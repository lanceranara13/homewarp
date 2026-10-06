//! The guard on the VPS itself (PLAN.md §6): "harden this VPS".
//!
//! What the Gate does by itself is pass on what arrives for the servers. What
//! arrives for the VPS itself is none of its business until its owner asks.
//! Then everything that arrives there from the internet is dropped, but for
//! what was listening when they asked, and new SSH connections from one
//! address are held to a few a minute.
//!
//! A rule like that can shut its owner out. So it is put in place on trial,
//! for a minute, and undone again by itself unless it is told within that
//! minute, through the tunnel, to stay: by which time its owner has seen that
//! they can still get in.

use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use homewarp_net::guard_dropped;
use homewarp_proto::{Guard, GuardState, Open};

use crate::{Gate, Refusal, allowed, failed, lock, read};

/// How long a guard is left in place that has not been told to stay.
const TRIAL: Duration = Duration::from_secs(60);
/// More ports than any machine has open, and few enough to refuse a mistake.
const MOST_PORTS: usize = 256;
/// Where a guard that was told to stay is written down, in the Gate's directory.
const KEPT: &str = "guard.json";

/// The guard as the Gate has it.
pub(crate) struct Guarding {
    /// What stays open on the VPS itself.
    pub(crate) open: Open,
    /// While it is on trial: when the trial ends, and what was kept before it.
    trial: Option<(Instant, Option<Open>)>,
}

/// The guard that was told to stay before this Gate was started, if one was.
pub(crate) fn kept(dir: &Path) -> anyhow::Result<Option<Guarding>> {
    Ok(read::<Open>(&dir.join(KEPT))?.map(|open| Guarding { open, trial: None }))
}

/// Whether an address, as the kernel's tables write one, is the machine's own
/// for itself: nothing from outside arrives there.
fn loopback(address: &str) -> bool {
    match address.len() {
        // IPv4, its bytes backwards: 127.x.y.z ends in 7F.
        8 => address.ends_with("7F"),
        // IPv6's ::1, and an IPv4 address of that kind written as an IPv6 one.
        32 => {
            address == "00000000000000000000000001000000"
                || (address.starts_with("0000000000000000FFFF0000") && address.ends_with("7F"))
        }
        _ => false,
    }
}

/// The ports, in one of the kernel's tables of sockets, that something
/// listens on where anyone can reach it.
fn listeners(table: &str, tcp: bool) -> Vec<u16> {
    let port_of = |line: &str| {
        let mut fields = line.split_whitespace().skip(1);
        let (local, remote, state) = (fields.next()?, fields.next()?, fields.next()?);
        let (address, port) = local.rsplit_once(':')?;
        let waiting = match tcp {
            // Listening, as the kernel numbers that.
            true => state == "0A",
            // Not tied to one other end: whoever sends to it is heard.
            false => remote.ends_with(":0000"),
        };
        match waiting && !loopback(address) {
            true => u16::from_str_radix(port, 16).ok().filter(|port| *port != 0),
            false => None,
        }
    };
    table.lines().skip(1).filter_map(port_of).collect()
}

/// What is listening on this machine now, where anyone can reach it.
fn listening() -> Open {
    let ports = |tables: [&str; 2], tcp: bool| {
        let mut all: Vec<u16> = tables
            .iter()
            .filter_map(|name| fs::read_to_string(format!("/proc/net/{name}")).ok())
            .flat_map(|table| listeners(&table, tcp))
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    };
    Open {
        tcp: ports(["tcp", "tcp6"], true),
        udp: ports(["udp", "udp6"], false),
    }
}

/// How the guard stands, and what is listening.
fn view(gate: &Gate) -> Guard {
    let (state, open, seconds_left) = match lock(&gate.guard).as_ref() {
        None => (GuardState::Off, Open::default(), None),
        Some(guarding) => match guarding.trial {
            Some((until, _)) => {
                let left = until.saturating_duration_since(Instant::now());
                (
                    GuardState::Trial,
                    guarding.open.clone(),
                    Some(left.as_secs()),
                )
            }
            None => (GuardState::Kept, guarding.open.clone(), None),
        },
    };
    Guard {
        dropped: match state {
            GuardState::Off => 0,
            _ => guard_dropped(),
        },
        state,
        open,
        seconds_left,
        listening: listening(),
    }
}

pub(crate) async fn get(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
) -> Result<Json<Guard>, Refusal> {
    allowed(&gate, &headers)?;
    Ok(Json(view(&gate)))
}

/// Puts a guard in place on trial: everything that arrives at the VPS itself
/// from the internet is dropped, but for these ports. In a minute it is
/// undone again, unless [`keep`] has said that it is to stay.
pub(crate) async fn put(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
    Json(mut open): Json<Open>,
) -> Result<Json<Guard>, Refusal> {
    allowed(&gate, &headers)?;
    let sound = |ports: &[u16]| ports.len() <= MOST_PORTS && !ports.contains(&0);
    if !sound(&open.tcp) || !sound(&open.udp) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Those are not ports a guard can keep open.".to_owned(),
        ));
    }
    // The tunnel's own port, whatever was asked for: without it this Gate
    // would be shut out of its own home, and could not be told anything more.
    open.udp.push(lock(&gate.config).listen_port);
    for ports in [&mut open.tcp, &mut open.udp] {
        ports.sort_unstable();
        ports.dedup();
    }
    let until = Instant::now() + TRIAL;
    let before = {
        let mut guard = lock(&gate.guard);
        // What a trial goes back to is what was kept, not another trial.
        let before = match guard.take() {
            Some(Guarding { open, trial: None }) => Some(open),
            Some(Guarding {
                trial: Some((_, before)),
                ..
            }) => before,
            None => None,
        };
        *guard = Some(Guarding {
            open,
            trial: Some((until, before.clone())),
        });
        before
    };
    if let Err(error) = gate.apply_again() {
        *lock(&gate.guard) = before.map(|open| Guarding { open, trial: None });
        let _ = gate.apply_again();
        return Err(failed(error));
    }
    tracing::info!("The VPS is guarded: for a minute, unless it is told to stay so.");
    let on_trial = Arc::clone(&gate);
    tokio::spawn(async move {
        tokio::time::sleep(TRIAL).await;
        end_trial(&on_trial, until);
    });
    Ok(Json(view(&gate)))
}

/// Undoes a guard whose minute is over and that was not told to stay, and
/// puts back what was kept before it.
fn end_trial(gate: &Gate, until: Instant) {
    {
        let mut guard = lock(&gate.guard);
        let before = match guard.as_ref() {
            // A later trial has taken this one's place, and ends by itself.
            Some(Guarding {
                trial: Some((ends, before)),
                ..
            }) if *ends == until => before.clone(),
            _ => return,
        };
        *guard = before.map(|open| Guarding { open, trial: None });
    }
    tracing::warn!("The guard was not told to stay, and has been undone.");
    if let Err(error) = gate.apply_again() {
        tracing::error!("{error:#}");
    }
}

/// Tells a guard on trial to stay. It is written down, so that a restart of
/// this machine comes back with it.
pub(crate) async fn keep(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
) -> Result<Json<Guard>, Refusal> {
    allowed(&gate, &headers)?;
    let open = match lock(&gate.guard).as_mut() {
        Some(guarding) => {
            guarding.trial = None;
            guarding.open.clone()
        }
        None => {
            return Err((
                StatusCode::CONFLICT,
                "There is no guard to keep. Its minute was over, and it has been undone."
                    .to_owned(),
            ));
        }
    };
    // Written beside the file and moved over it, so that it is never half there.
    let beside = gate.dir.join("guard.json.new");
    fs::write(&beside, serde_json::to_vec(&open).map_err(failed)?).map_err(failed)?;
    fs::rename(&beside, gate.dir.join(KEPT)).map_err(failed)?;
    tracing::info!("The guard stays.");
    Ok(Json(view(&gate)))
}

/// Takes the guard away, kept or on trial.
pub(crate) async fn remove(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
) -> Result<Json<Guard>, Refusal> {
    allowed(&gate, &headers)?;
    *lock(&gate.guard) = None;
    match fs::remove_file(gate.dir.join(KEPT)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(failed(error)),
        _ => {}
    }
    gate.apply_again().map_err(failed)?;
    tracing::info!("The VPS is no longer guarded.");
    Ok(Json(view(&gate)))
}

#[cfg(test)]
mod tests {
    use super::{listeners, loopback};

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1034 1 0000000000000000 100 0 0 10 0
   1: 0100007F:0277 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2211 1 0000000000000000 100 0 0 10 0
   2: 3500007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   101        0 987 1 0000000000000000 100 0 0 10 0
   3: 014DD50A:12F9 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5120 1 0000000000000000 100 0 0 10 0
   4: D62A262D:0016 327100CB:C350 01 00000000:00000000 02:000A1B2C 00000000     0        0 7781 4 0000000000000000 20 4 30 10 -1
";
    const TCP6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:0050 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1500 1 0000000000000000 100 0 0 10 0
   1: 00000000000000000000000001000000:0019 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1600 1 0000000000000000 100 0 0 10 0
   2: 0000000000000000FFFF00000100007F:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1700 1 0000000000000000 100 0 0 10 0
";
    const UDP: &str = "   sk       local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  412: 00000000:CA6C 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 0 2 0000000000000000 0
  530: 3500007F:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000   101        0 985 2 0000000000000000 0
  611: D62A262D:D431 08080808:0035 01 00000000:00000000 00:00000000 00000000   101        0 9001 2 0000000000000000 0
  700: 00000000:0044 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 801 2 0000000000000000 0
";

    #[test]
    fn what_listens_where_anyone_can_reach_it_is_found_in_the_kernels_tables() {
        // SSH on every address and the Gate's own port on the tunnel's. Not
        // what listens on the machine for itself, nor a connection that is open.
        assert_eq!(listeners(TCP, true), [22, 4857]);
        assert_eq!(listeners(TCP6, true), [80]);
        // The tunnel and the address-asking client. Not the resolver that
        // listens on the machine for itself, nor a socket tied to one other end.
        assert_eq!(listeners(UDP, false), [51820, 68]);
        assert!(listeners("", true).is_empty());
        assert!(listeners("a heading\nnot a line of the table\n", true).is_empty());
    }

    #[test]
    fn the_machines_own_addresses_for_itself_are_told_apart() {
        for own in [
            "0100007F",
            "3500007F",
            "00000000000000000000000001000000",
            "0000000000000000FFFF00000100007F",
        ] {
            assert!(loopback(own), "{own}");
        }
        for reachable in [
            "00000000",
            "014DD50A",
            "D62A262D",
            "00000000000000000000000000000000",
            "0000000000000000FFFF0000D62A262D",
            "",
        ] {
            assert!(!loopback(reachable), "{reachable}");
        }
    }
}
