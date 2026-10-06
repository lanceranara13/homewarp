//! The tunnel (PLAN.md §5.3): kernel WireGuard, and the nftables rules on both
//! of its ends.
//!
//! Each end owns one table, `inet homewarp`, and replaces it whole. Nothing
//! here edits a rule that another program made.

use std::{
    fmt::Write as _,
    io::{self, Write as _},
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    process::{Command, Stdio},
    time::SystemTime,
};

use defguard_wireguard_rs::{
    InterfaceConfiguration, Kernel, WGApi, WireguardInterfaceApi, key::Key, net::IpAddrMask,
    peer::Peer,
};
use homewarp_proto::{Desired, Mode, Protocol};

/// What both ends call the tunnel's interface.
pub const INTERFACE: &str = "homewarp0";
/// Small enough for WireGuard's own header to fit inside any ordinary link.
pub const MTU: u32 = 1380;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("`{0}` is not the name of a network interface")]
    Interface(String),
    #[error("WireGuard: {0}")]
    WireGuard(String),
    #[error("the way back could not be set up: {0}")]
    Ip(io::Error),
    #[error("the way back could not be set up: {0}")]
    Routing(String),
    #[error("nft could not be run: {0}")]
    Nft(#[from] io::Error),
    #[error("nft refused the rules: {0}")]
    Refused(String),
}

/// The Gate's whole table, as nft reads it.
///
/// A forwarded port is translated to the same port at `home` and sent into the
/// tunnel with its source left alone, so that home sees the player's address.
/// In NAT mode the source becomes the Gate's instead. Nothing else passes:
/// only forwarded ports go in, and nothing that home starts comes out.
///
/// `wan` is the interface players arrive on. It is written into the rules, so
/// it is held to what an interface's name can be.
pub fn gate_ruleset(wan: &str, home: Ipv4Addr, desired: &Desired) -> Result<String, Error> {
    let named_well = (1..=15).contains(&wan.len())
        && wan
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !named_well {
        return Err(Error::Interface(wan.to_owned()));
    }
    let map = |protocol: Protocol| {
        let ports: Vec<String> = desired
            .forwards
            .iter()
            .filter(|forward| forward.protocol == protocol)
            .map(|forward| format!("{0} : {home} . {0}", forward.port))
            .collect();
        // nft has no way to write a list of nothing.
        match ports.is_empty() {
            true => String::new(),
            false => format!(" elements = {{ {} }}", ports.join(", ")),
        }
    };
    let masquerade = match desired.mode {
        Mode::Transparent => "",
        Mode::Nat => "\n    oifname \"homewarp0\" masquerade",
    };

    let mut rules = String::new();
    // Made if it is not there, so that deleting it cannot fail; then made anew.
    // nft takes the whole of this or none of it.
    writeln!(
        rules,
        r#"table inet homewarp
delete table inet homewarp
table inet homewarp {{
  map fwd_tcp {{ type inet_service : ipv4_addr . inet_service;{tcp} }}
  map fwd_udp {{ type inet_service : ipv4_addr . inet_service;{udp} }}
  set newconn {{ type ipv4_addr; size 65535; flags dynamic,timeout; timeout 1m; }}

  chain prerouting {{
    type nat hook prerouting priority dstnat; policy accept;
    iifname "{wan}" dnat ip to tcp dport map @fwd_tcp
    iifname "{wan}" dnat ip to udp dport map @fwd_udp
  }}
  chain nat_mode {{
    type nat hook postrouting priority srcnat; policy accept;{masquerade}
  }}
  chain forward {{
    type filter hook forward priority filter; policy accept;
    oifname "homewarp0" ct status dnat goto to_home
    oifname "homewarp0" counter drop
    iifname "homewarp0" ct state new counter drop
  }}
  chain to_home {{
    ct state new add @newconn {{ ip saddr limit rate over 30/second burst 60 packets }} counter drop
    tcp flags syn tcp option maxseg size set rt mtu
  }}
}}"#,
        tcp = map(Protocol::Tcp),
        udp = map(Protocol::Udp),
    )
    .expect("writing to a String does not fail");
    Ok(rules)
}

/// One end of the tunnel, as the kernel is to have it.
pub struct Link {
    /// This end's own key, in base64 as WireGuard writes keys.
    pub private_key: String,
    /// Where WireGuard listens. 0 leaves the choice to the kernel, which is
    /// right for the end that only dials out.
    pub listen_port: u16,
    /// This end's address inside the tunnel. The two ends share a /30.
    pub address: Ipv4Addr,
    pub peer_public_key: String,
    pub preshared_key: String,
    /// What the other end may send from: at the Gate, home's tunnel address
    /// and nothing else; at home, anything, for players come from anywhere.
    pub peer_allowed: (Ipv4Addr, u8),
    /// Where to dial, for the end that dials: home.
    pub peer_endpoint: Option<SocketAddr>,
}

/// What an end has heard of the other.
pub struct Heard {
    /// How long ago the two last shook hands. None if they never have.
    pub handshake_age_seconds: Option<u64>,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

fn wireguard(error: impl std::fmt::Display) -> Error {
    Error::WireGuard(error.to_string())
}

/// A new private key and the public key that goes with it, both in base64.
pub fn new_keypair() -> (String, String) {
    let private = Key::generate();
    (private.to_string(), private.public_key().to_string())
}

/// Makes the tunnel's interface if it is not there and sets it up as `link`
/// says: key, port, address, the one peer. Done again, it changes what differs.
pub fn bring_up(link: &Link) -> Result<(), Error> {
    let mut api = WGApi::<Kernel>::new(INTERFACE).map_err(wireguard)?;
    let allowed = IpAddrMask::new(link.peer_allowed.0.into(), link.peer_allowed.1);
    let key = |text: &str| Key::try_from(text).map_err(wireguard);
    let peer_key = key(&link.peer_public_key)?;
    let preshared = key(&link.preshared_key)?;

    if !Path::new("/sys/class/net").join(INTERFACE).exists() {
        api.create_interface().map_err(wireguard)?;
    } else if let Ok(now) = api.read_interface_data() {
        // Set up again, WireGuard forgets the session it has and where the
        // other end is, and whoever is playing waits for the next handshake.
        // So an interface that is already as wanted is left alone: this
        // program can then be restarted or replaced under a running tunnel.
        let as_wanted = now.private_key == Some(key(&link.private_key)?)
            && (link.listen_port == 0 || now.listen_port == link.listen_port)
            && now.peers.len() == 1
            && now.peers.get(&peer_key).is_some_and(|peer| {
                peer.preshared_key.as_ref() == Some(&preshared)
                    && peer.allowed_ips.len() == 1
                    && peer.allowed_ips[0].address == allowed.address
                    && peer.allowed_ips[0].cidr == allowed.cidr
                    // Home dials. If the Gate has moved, home has to be told where to.
                    && link.peer_endpoint.is_none_or(|at| peer.endpoint == Some(at))
            });
        if as_wanted {
            return Ok(());
        }
    }
    let mut peer = Peer::new(peer_key);
    peer.preshared_key = Some(preshared);
    peer.set_allowed_ips(vec![allowed]);
    if let Some(endpoint) = link.peer_endpoint {
        peer.endpoint = Some(endpoint);
        // The end that dials keeps the way open through whatever NAT it is behind.
        peer.persistent_keepalive_interval = Some(25);
    }
    api.configure_interface(&InterfaceConfiguration {
        name: INTERFACE.to_owned(),
        prvkey: link.private_key.clone(),
        addresses: vec![IpAddrMask::new(link.address.into(), 30)],
        port: link.listen_port,
        peers: vec![peer],
        mtu: Some(MTU),
        fwmark: None,
    })
    .map_err(wireguard)
}

/// What this end has heard of the other: when, and how much.
pub fn heard() -> Result<Heard, Error> {
    let host = WGApi::<Kernel>::new(INTERFACE)
        .and_then(|api| api.read_interface_data())
        .map_err(wireguard)?;
    let mut heard = Heard {
        handshake_age_seconds: None,
        received_bytes: 0,
        sent_bytes: 0,
    };
    for peer in host.peers.values() {
        heard.received_bytes += peer.rx_bytes;
        heard.sent_bytes += peer.tx_bytes;
        // The kernel gives the start of time for a peer never heard from.
        let age = peer
            .last_handshake
            .filter(|at| *at > SystemTime::UNIX_EPOCH)
            .and_then(|at| at.elapsed().ok())
            .map(|age| age.as_secs());
        heard.handshake_age_seconds = heard.handshake_age_seconds.or(age);
    }
    Ok(heard)
}

/// Home's whole table, as nft reads it.
///
/// A connection that comes in from the Gate is marked, and the mark is put on
/// its replies, which [`route_replies`] then sends back into the tunnel: that
/// is what lets a server see its players' own addresses. The rest is what
/// neither the Gate nor a game server may do. From the tunnel, only what
/// Docker published on `bridge` gets in; a server reaches neither this
/// machine nor the networks behind it.
pub fn home_ruleset(bridge: &str) -> Result<String, Error> {
    let named_well = (1..=15).contains(&bridge.len())
        && bridge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !named_well {
        return Err(Error::Interface(bridge.to_owned()));
    }
    Ok(format!(
        r#"table inet homewarp
delete table inet homewarp
table inet homewarp {{
  chain mark_in {{
    type filter hook prerouting priority mangle; policy accept;
    iifname "homewarp0" ct state new ct mark set 0x4857
    iifname != "homewarp0" ct mark 0x4857 meta mark set ct mark
  }}
  chain input {{
    type filter hook input priority filter; policy accept;
    iifname {{ "homewarp0", "{bridge}" }} ct state established,related accept
    iifname "homewarp0" counter drop
    iifname "{bridge}" counter drop
  }}
  chain forward {{
    type filter hook forward priority filter - 1; policy accept;
    iifname "homewarp0" oifname "{bridge}" ct status dnat accept
    iifname "homewarp0" counter drop
    iifname "{bridge}" ip daddr {{ 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 }} ct state new counter drop
    oifname "homewarp0" tcp flags syn tcp option maxseg size set rt mtu
  }}
}}
"#
    ))
}

fn ip(arguments: &[&str]) -> Result<(), Error> {
    let done = Command::new("ip")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(Error::Ip)?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(Error::Routing(said.trim().to_owned()));
    }
    Ok(())
}

/// Home's way back: replies that carry the mark leave by the tunnel and not by
/// the home's own line, and the tunnel's interface alone lets in packets from
/// addresses that route elsewhere, which is what every player's is.
pub fn route_replies() -> Result<(), Error> {
    // Taken away first, so that doing this twice leaves one rule and not two.
    let _ = ip(&["rule", "del", "fwmark", "0x4857", "lookup", "4857"]);
    ip(&["rule", "add", "fwmark", "0x4857", "lookup", "4857"])?;
    ip(&[
        "route", "replace", "default", "dev", INTERFACE, "table", "4857",
    ])?;
    std::fs::write(
        format!("/proc/sys/net/ipv4/conf/{INTERFACE}/rp_filter"),
        "2",
    )
    .map_err(Error::Ip)
}

/// Removes everything this crate makes, on either end: the table, the way
/// back and the interface. What is not there is passed over.
pub fn take_down() {
    let _ = ip(&["rule", "del", "fwmark", "0x4857", "lookup", "4857"]);
    let _ = ip(&["link", "del", INTERFACE]);
    let _ = apply("table inet homewarp\ndelete table inet homewarp\n");
}

/// Hands a ruleset to nft, which makes all of it take effect or none of it.
pub fn apply(ruleset: &str) -> Result<(), Error> {
    let mut nft = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    nft.stdin
        .take()
        .expect("stdin was asked for")
        .write_all(ruleset.as_bytes())?;
    let done = nft.wait_with_output()?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(Error::Refused(said.trim().to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use homewarp_proto::{Desired, Forward, Mode, Protocol};

    use super::{Error, gate_ruleset};

    const HOME: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 2);

    fn desired(mode: Mode, forwards: &[(u16, Protocol)]) -> Desired {
        Desired {
            generation: 1,
            mode,
            forwards: forwards
                .iter()
                .map(|&(port, protocol)| Forward { port, protocol })
                .collect(),
        }
    }

    #[test]
    fn forwards_each_port_to_the_same_port_at_home() {
        let asked = [
            (25565, Protocol::Tcp),
            (25565, Protocol::Udp),
            (27015, Protocol::Udp),
        ];
        let rules = gate_ruleset("eth0", HOME, &desired(Mode::Transparent, &asked)).unwrap();
        assert!(rules.contains(
            "map fwd_tcp { type inet_service : ipv4_addr . inet_service; elements = { 25565 : 10.213.77.2 . 25565 } }"
        ));
        assert!(
            rules.contains(
                "elements = { 25565 : 10.213.77.2 . 25565, 27015 : 10.213.77.2 . 27015 }"
            )
        );
        assert!(rules.contains("iifname \"eth0\" dnat ip to tcp dport map @fwd_tcp"));
        // The player's address goes home untouched.
        assert!(!rules.contains("masquerade"));
        // The table is replaced whole.
        assert!(rules.starts_with("table inet homewarp\ndelete table inet homewarp\n"));
    }

    #[test]
    fn with_nothing_to_forward_the_maps_are_empty_and_still_there() {
        let rules = gate_ruleset("ens3", HOME, &Desired::default()).unwrap();
        assert!(rules.contains("map fwd_tcp { type inet_service : ipv4_addr . inet_service; }"));
        assert!(!rules.contains("elements"));
    }

    #[test]
    fn nat_mode_is_one_rule_more() {
        let nat = gate_ruleset("eth0", HOME, &desired(Mode::Nat, &[])).unwrap();
        assert!(nat.contains("oifname \"homewarp0\" masquerade"));
    }

    #[test]
    fn home_marks_what_comes_from_the_gate_and_lets_in_only_what_docker_published() {
        let rules = super::home_ruleset("homewarp-br").unwrap();
        // The mark goes on a connection as it arrives, and onto its replies only.
        assert!(rules.contains("iifname \"homewarp0\" ct state new ct mark set 0x4857"));
        assert!(rules.contains("iifname != \"homewarp0\" ct mark 0x4857 meta mark set ct mark"));
        assert!(
            rules.contains("iifname \"homewarp0\" oifname \"homewarp-br\" ct status dnat accept")
        );
        assert!(rules.contains(
            "iifname { \"homewarp0\", \"homewarp-br\" } ct state established,related accept"
        ));
        assert!(matches!(
            super::home_ruleset("br\" accept; #"),
            Err(Error::Interface(_))
        ));
    }

    #[test]
    fn an_interface_name_cannot_carry_rules_in_with_it() {
        for odd in [
            "",
            "eth0\" accept; #",
            "an-interface-name-too-long",
            "eth 0",
        ] {
            assert!(matches!(
                gate_ruleset(odd, HOME, &Desired::default()),
                Err(Error::Interface(_))
            ));
        }
    }
}
