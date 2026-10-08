//! The tunnel (PLAN.md §5.3): kernel WireGuard, and the nftables rules on both
//! of its ends.
//!
//! Each end owns one table, `inet homewarp`, and replaces it whole. Nothing
//! here edits a rule that another program made.

use std::{
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    process::{Command, Stdio},
    time::SystemTime,
};

use defguard_wireguard_rs::{
    InterfaceConfiguration, Kernel, WGApi, WireguardInterfaceApi, key::Key, net::IpAddrMask,
    peer::Peer,
};
use homewarp_proto::{Desired, Mode, NEW_PER_SECOND, Open, Protocol, Through};

/// What a Gate calls its tunnel's interface, and home its first tunnel's.
pub const INTERFACE: &str = "homewarp0";
/// How many tunnels a home can keep at once: one to each VPS, numbered from 0.
pub const TUNNELS: u8 = 8;
/// What home marks the connections of its first tunnel with, and the number of
/// the routing table that sends their replies back into it. Each further
/// tunnel has the next number of both.
const MARK: u32 = 0x4857;

/// What home calls the interface of one of its tunnels.
pub fn interface(tunnel: u8) -> String {
    format!("homewarp{tunnel}")
}

/// Whether an interface is one of home's tunnels, by its name.
fn is_tunnel(interface: &str) -> bool {
    interface
        .strip_prefix("homewarp")
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|c| c.is_ascii_digit()))
}
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

/// What an interface's name can be, which is also what is safe inside a rule.
fn named_well(interface: &str) -> bool {
    (1..=15).contains(&interface.len())
        && interface
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// A port on the VPS that is sent home for the length of a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeForward {
    pub public_port: u16,
    pub home_port: u16,
}

/// The Gate's whole table, as nft reads it.
///
/// A forwarded port is translated to the same port at `home` and sent into the
/// tunnel with its source left alone, so that home sees the player's address.
/// In NAT mode the source becomes the Gate's instead. Nothing else passes:
/// only forwarded ports go in, and nothing that home starts comes out.
///
/// New connections are held to so many a second from one address, counted in
/// a list of addresses that has an end. An address there was no room to count
/// by itself is counted with every other such: all of those together get what
/// one address gets, so that a flood from more addresses than the list holds
/// does not go home unlimited for having filled it.
///
/// The tunnel's own addresses are answered inside the tunnel only. A machine
/// answers for each of its addresses on every interface it has, and so would
/// answer a neighbour on the VPS's own network that asked the public
/// interface for the address this Gate's door is on: that is dropped.
///
/// `wan` is the interface players arrive on. It is written into the rules, so
/// it is held to what an interface's name can be.
///
/// `probe` is one more TCP port, sent to a port of home's choosing: there for
/// the few seconds in which home looks at how a connection through here arrives.
pub fn gate_ruleset(
    wan: &str,
    home: Ipv4Addr,
    desired: &Desired,
    probe: Option<ProbeForward>,
) -> Result<String, Error> {
    gate_rules(wan, home, desired, probe, true)
}

/// As [`gate_ruleset`], without the count of what passes through each port:
/// for a VPS whose nft is too old to keep one.
pub fn gate_ruleset_uncounted(
    wan: &str,
    home: Ipv4Addr,
    desired: &Desired,
    probe: Option<ProbeForward>,
) -> Result<String, Error> {
    gate_rules(wan, home, desired, probe, false)
}

fn gate_rules(
    wan: &str,
    home: Ipv4Addr,
    desired: &Desired,
    probe: Option<ProbeForward>,
    counted: bool,
) -> Result<String, Error> {
    if !named_well(wan) {
        return Err(Error::Interface(wan.to_owned()));
    }
    // The forwarded ports once more, as a set whose every element keeps a
    // count of what matched it: what went through each port, either way. The
    // probe's port is not among them, being no server's.
    let through = |protocol: Protocol| {
        let ports: Vec<String> = desired
            .forwards
            .iter()
            .filter(|forward| forward.protocol == protocol)
            .map(|forward| forward.port.to_string())
            .collect();
        match ports.is_empty() {
            true => String::new(),
            false => format!(" elements = {{ {} }}", ports.join(", ")),
        }
    };
    let (sets, to_home, from_home) = match counted {
        true => (
            format!(
                "\n  set through_tcp {{ type inet_service; counter;{} }}\n  set through_udp {{ type inet_service; counter;{} }}",
                through(Protocol::Tcp),
                through(Protocol::Udp)
            ),
            "\n    tcp dport @through_tcp\n    udp dport @through_udp",
            "\n    iifname \"homewarp0\" tcp sport @through_tcp\n    iifname \"homewarp0\" udp sport @through_udp",
        ),
        false => (String::new(), "", ""),
    };
    let map = |protocol: Protocol| {
        let mut ports: Vec<String> = desired
            .forwards
            .iter()
            .filter(|forward| forward.protocol == protocol)
            .map(|forward| format!("{0} : {home} . {0}", forward.port))
            .collect();
        if let (Protocol::Tcp, Some(probe)) = (protocol, probe) {
            ports.push(format!(
                "{} : {home} . {}",
                probe.public_port, probe.home_port
            ));
        }
        // nft has no way to write a list of nothing.
        match ports.is_empty() {
            true => String::new(),
            false => format!(" elements = {{ {} }}", ports.join(", ")),
        }
    };
    // Twice as many at once as in a second: a player who joins opens a few.
    let rate = desired.new_per_second.unwrap_or(NEW_PER_SECOND).max(1);
    let burst = rate.saturating_mul(2);
    let masquerade = match desired.mode {
        Mode::Transparent => "",
        Mode::Nat => "\n    oifname \"homewarp0\" masquerade",
    };
    // The tunnel's own addresses: four of them, of which home's is one.
    let tunnel = Ipv4Addr::from(u32::from(home) & !3);

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
  set newconn {{ type ipv4_addr; size 65535; flags dynamic,timeout; timeout 1m; }}{sets}

  chain prerouting {{
    type nat hook prerouting priority dstnat; policy accept;
    iifname "{wan}" dnat ip to tcp dport map @fwd_tcp
    iifname "{wan}" dnat ip to udp dport map @fwd_udp
  }}
  chain own {{
    type filter hook input priority filter - 10; policy accept;
    iifname "{wan}" ip daddr {tunnel}/30 counter drop
  }}
  chain nat_mode {{
    type nat hook postrouting priority srcnat; policy accept;{masquerade}
  }}
  chain forward {{
    type filter hook forward priority filter; policy accept;
    oifname "homewarp0" ct status dnat goto to_home
    oifname "homewarp0" counter drop
    iifname "homewarp0" ct state new counter drop{from_home}
  }}
  chain to_home {{
    ct state new add @newconn {{ ip saddr limit rate over {rate}/second burst {burst} packets }} counter drop
    ct state new ip saddr != @newconn limit rate over {rate}/second burst {burst} packets counter drop
    tcp flags syn tcp option maxseg size set rt mtu{to_home}
  }}
}}"#,
        tcp = map(Protocol::Tcp),
        udp = map(Protocol::Udp),
    )
    .expect("writing to a String does not fail");
    Ok(rules)
}

/// How many new SSH connections a minute one address may open to a guarded
/// VPS. A person opens a few; what guesses at passwords opens thousands.
const SSH_PER_MINUTE: u32 = 12;

/// The Gate's table with a guard on the VPS itself added to it (PLAN.md §6):
/// what arrives at the VPS itself from the internet is dropped, but for the
/// ports in `open`, and new SSH connections from one address are held to a
/// few a minute; addresses there was no room to count by themselves are held
/// to that together, as in [`gate_ruleset`]. `rules` is what it made.
///
/// Only what arrives on `wan` for the machine itself is looked at. What is
/// forwarded to servers never comes this way; nor does what arrives by the
/// tunnel, or by any other interface the VPS has. A connection that is open
/// stays open, the machine can still be pinged, and it can still be given an
/// address by whoever gives it one.
pub fn guarded(rules: &str, wan: &str, open: &Open) -> Result<String, Error> {
    if !named_well(wan) {
        return Err(Error::Interface(wan.to_owned()));
    }
    let elements = |ports: &[u16]| {
        let mut ports = ports.to_vec();
        ports.sort_unstable();
        ports.dedup();
        let ports: Vec<String> = ports.iter().map(u16::to_string).collect();
        // nft has no way to write a list of nothing.
        match ports.is_empty() {
            true => String::new(),
            false => format!(" elements = {{ {} }}", ports.join(", ")),
        }
    };
    let guard = format!(
        r#"
  set guard_tcp {{ type inet_service;{tcp} }}
  set guard_udp {{ type inet_service;{udp} }}
  set guard_ssh {{ type ipv4_addr; size 65535; flags dynamic,timeout; timeout 10m; }}
  set guard_ssh6 {{ type ipv6_addr; size 65535; flags dynamic,timeout; timeout 10m; }}
  counter guard_dropped {{ packets 0 bytes 0 }}

  chain guard {{
    type filter hook input priority filter - 5; policy accept;
    iifname != "{wan}" accept
    ct state established,related accept
    meta l4proto {{ icmp, ipv6-icmp }} accept
    udp dport {{ 68, 546 }} accept
    tcp dport 22 ct state new add @guard_ssh {{ ip saddr limit rate over {SSH_PER_MINUTE}/minute burst {SSH_PER_MINUTE} packets }} counter name "guard_dropped" drop
    tcp dport 22 ct state new ip saddr != @guard_ssh limit rate over {SSH_PER_MINUTE}/minute burst {SSH_PER_MINUTE} packets counter name "guard_dropped" drop
    tcp dport 22 ct state new add @guard_ssh6 {{ ip6 saddr limit rate over {SSH_PER_MINUTE}/minute burst {SSH_PER_MINUTE} packets }} counter name "guard_dropped" drop
    tcp dport 22 ct state new ip6 saddr != @guard_ssh6 limit rate over {SSH_PER_MINUTE}/minute burst {SSH_PER_MINUTE} packets counter name "guard_dropped" drop
    tcp dport @guard_tcp accept
    udp dport @guard_udp accept
    counter name "guard_dropped" drop
  }}
"#,
        tcp = elements(&open.tcp),
        udp = elements(&open.udp),
    );
    // Put in ahead of the brace that closes the table.
    let (table, _) = rules
        .trim_end()
        .rsplit_once('}')
        .ok_or_else(|| Error::Refused("there is no table to add a guard to".to_owned()))?;
    Ok(format!("{table}{guard}}}\n"))
}

/// How many packets the guard of [`guarded`] has dropped since the table was
/// last put in the kernel. None where there is no guard.
pub fn guard_dropped() -> u64 {
    counter("guard_dropped").unwrap_or(0)
}

/// One end of the tunnel, as the kernel is to have it.
pub struct Link {
    /// The interface it is: [`INTERFACE`] at a Gate, one of [`interface`] at home.
    pub interface: String,
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
    /// Where the other end's packets last came from.
    pub endpoint: Option<IpAddr>,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

fn wireguard(error: impl std::fmt::Display) -> Error {
    Error::WireGuard(error.to_string())
}

/// A private key as the kernel keeps it. Three bits of a key have no say in
/// what it does, and the kernel sets them its own way when it is given one. A
/// key kept as it was made would then never look like the one the kernel has.
fn clamped(key: &Key) -> Key {
    let mut bytes = key.as_array();
    bytes[0] &= 248;
    bytes[31] = (bytes[31] & 127) | 64;
    Key::new(bytes)
}

/// A new private key and the public key that goes with it, both in base64.
pub fn new_keypair() -> (String, String) {
    let private = clamped(&Key::generate());
    (private.to_string(), private.public_key().to_string())
}

/// A new preshared key, in base64: 32 bytes of nothing but chance.
pub fn new_preshared_key() -> String {
    Key::generate().to_string()
}

/// Makes the tunnel's interface if it is not there and sets it up as `link`
/// says: key, port, address, the one peer. Done again, it changes what differs.
pub fn bring_up(link: &Link) -> Result<(), Error> {
    if !named_well(&link.interface) {
        return Err(Error::Interface(link.interface.clone()));
    }
    let mut api = WGApi::<Kernel>::new(link.interface.as_str()).map_err(wireguard)?;
    let allowed = IpAddrMask::new(link.peer_allowed.0.into(), link.peer_allowed.1);
    let key = |text: &str| Key::try_from(text).map_err(wireguard);
    let peer_key = key(&link.peer_public_key)?;
    let preshared = key(&link.preshared_key)?;

    if !Path::new("/sys/class/net").join(&link.interface).exists() {
        api.create_interface().map_err(wireguard)?;
    } else if let Ok(now) = api.read_interface_data() {
        // Set up again, WireGuard forgets the session it has and where the
        // other end is, and whoever is playing waits for the next handshake.
        // So an interface that is already as wanted is left alone: this
        // program can then be restarted or replaced under a running tunnel.
        let as_wanted = now.private_key == Some(clamped(&key(&link.private_key)?))
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
        name: link.interface.clone(),
        prvkey: link.private_key.clone(),
        addresses: vec![IpAddrMask::new(link.address.into(), 30)],
        port: link.listen_port,
        peers: vec![peer],
        mtu: Some(MTU),
        fwmark: None,
    })
    .map_err(wireguard)
}

/// What has come in by one of home's tunnels and what has gone out by it, in
/// bytes, counted by the kernel from when the interface was made. None where
/// there is no such interface.
pub fn passed(tunnel: u8) -> Option<(u64, u64)> {
    let count = |which: &str| {
        let path = format!(
            "/sys/class/net/{}/statistics/{which}_bytes",
            interface(tunnel)
        );
        fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
    };
    Some((count("rx")?, count("tx")?))
}

/// What a Gate has heard of home: when, and how much.
pub fn heard() -> Result<Heard, Error> {
    let host = WGApi::<Kernel>::new(INTERFACE)
        .and_then(|api| api.read_interface_data())
        .map_err(wireguard)?;
    let mut heard = Heard {
        handshake_age_seconds: None,
        endpoint: None,
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
        heard.endpoint = heard.endpoint.or(peer.endpoint.map(|from| from.ip()));
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
///
/// `probe` is a TCP port that Core has published for a moment as a server's is,
/// to see how a connection through the Gate arrives. Nothing is opened for it:
/// it comes in as a player's connection does. What arrives for it from the
/// tunnel is counted, before anything on this machine can drop it, so that
/// "it never got here" can be told from "it got here, and no reply got back".
///
/// `panel` is the TCP port the panel is reached on over TLS, once it has been
/// given a name. Its door is a container that is no server and sits on no
/// bridge of Homewarp's, so it is let in by what it came for: a connection
/// from the tunnel that was to that port and that Docker passed on. The panel's
/// other port, the one without TLS, stays shut to the tunnel.
///
/// `tunnels` is every tunnel home keeps, one to each VPS. Each has a mark of
/// its own, so that a reply leaves by the tunnel its connection came in by and
/// a player is answered from the address they connected to. `probe` names the
/// tunnel whose VPS is being looked at, with the port.
pub fn home_ruleset(
    bridge: &str,
    tunnels: &[u8],
    probe: Option<(u8, u16)>,
    panel: Option<u16>,
) -> Result<String, Error> {
    if !named_well(bridge) {
        return Err(Error::Interface(bridge.to_owned()));
    }
    let mut tunnels = tunnels.to_vec();
    tunnels.sort_unstable();
    tunnels.dedup();
    if tunnels.is_empty() || tunnels.iter().any(|tunnel| *tunnel >= TUNNELS) {
        return Err(Error::Routing(
            "there is no such tunnel to make rules for".to_owned(),
        ));
    }
    let list = |each: &dyn Fn(u8) -> String| {
        let all: Vec<String> = tunnels.iter().map(|tunnel| each(*tunnel)).collect();
        all.join(", ")
    };
    // Every tunnel's interface, and every tunnel's mark, as nft writes a list.
    let from = list(&|tunnel| format!("\"{}\"", interface(tunnel)));
    let marks = list(&|tunnel| format!("{:#x}", MARK + u32::from(tunnel)));
    let marked: String = tunnels
        .iter()
        .map(|tunnel| {
            format!(
                "\n    iifname \"{}\" ct state new ct mark set {:#x}",
                interface(*tunnel),
                MARK + u32::from(*tunnel)
            )
        })
        .collect();
    let panel = match panel {
        None => String::new(),
        Some(port) => format!(
            "\n    iifname {{ {from} }} meta l4proto tcp ct original proto-dst {port} ct status dnat accept"
        ),
    };
    let (counter, counted) = match probe {
        None => Default::default(),
        Some((tunnel, port)) => (
            "\n  counter probe { packets 0 bytes 0 }".to_owned(),
            format!(
                "\n    iifname \"{}\" tcp dport {port} counter name \"probe\"",
                interface(tunnel)
            ),
        ),
    };
    Ok(format!(
        r#"table inet homewarp
delete table inet homewarp
table inet homewarp {{{counter}
  chain mark_in {{
    type filter hook prerouting priority mangle; policy accept;{counted}{marked}
    iifname != {{ {from} }} ct mark {{ {marks} }} meta mark set ct mark
  }}
  chain input {{
    type filter hook input priority filter; policy accept;
    iifname {{ {from}, "{bridge}" }} ct state established,related accept
    iifname {{ {from} }} counter drop
    iifname "{bridge}" counter drop
  }}
  chain forward {{
    type filter hook forward priority filter - 1; policy accept;
    iifname {{ {from} }} oifname "{bridge}" ct status dnat accept{panel}
    iifname {{ {from} }} counter drop
    iifname "{bridge}" ip daddr {{ 10.0.0.0/8, 100.64.0.0/10, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }} ct state new counter drop
    oifname {{ {from} }} tcp flags syn tcp option maxseg size set rt mtu
  }}
}}
"#
    ))
}

/// Runs `ip` and returns what it printed.
fn ip(arguments: &[&str]) -> Result<String, Error> {
    let done = Command::new("ip")
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(Error::Ip)?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(Error::Routing(said.trim().to_owned()));
    }
    Ok(String::from_utf8_lossy(&done.stdout).into_owned())
}

/// Home's way back: replies that carry the mark leave by the tunnel and not by
/// the home's own line, and the tunnel's interface alone lets in packets from
/// addresses that route elsewhere, which is what every player's is.
///
/// Done again it changes nothing, and at no moment is the way back missing:
/// it is done again every few seconds, under whoever is playing.
pub fn route_replies(tunnel: u8) -> Result<(), Error> {
    let interface = interface(tunnel);
    let interface = interface.as_str();
    let (mark, table) = way_back(tunnel);
    let rules = ip(&["rule", "show"])?;
    let there = rules.lines().any(|rule| {
        let mut words = rule.split_whitespace();
        words.any(|word| word == mark) && rule.trim_end().ends_with(&format!("lookup {table}"))
    });
    if !there {
        ip(&["rule", "add", "fwmark", &mark, "lookup", &table])?;
    }
    // The kernel drops this route whenever the interface loses its address,
    // which setting WireGuard up again makes it do for a moment.
    ip(&[
        "route", "replace", "default", "dev", interface, "table", &table,
    ])?;
    let filter = |of: &str| {
        fs::read_to_string(format!("/proc/sys/net/ipv4/conf/{of}/rp_filter"))
            .ok()
            .and_then(|value| value.trim().parse::<u8>().ok())
    };
    // The kernel goes by the greater of an interface's own setting and the one
    // for all of them, where 1 is strict and 2 is loose.
    let strict = || filter(interface).max(filter("all")) == Some(1);
    if !strict() {
        return Ok(());
    }
    // A container is often not allowed to change this, even with the right to
    // change the network. Then players' packets are dropped as they arrive,
    // which Core's probe finds out, and the Gate stands in for them instead.
    let _ = fs::write(
        format!("/proc/sys/net/ipv4/conf/{interface}/rp_filter"),
        "2",
    );
    Ok(())
}

/// The mark of a tunnel's connections and the number of the routing table that
/// sends their replies back into it, as `ip` writes and reads them.
fn way_back(tunnel: u8) -> (String, String) {
    // The first table's number is the mark's digits, read as a decimal number.
    (
        format!("{:#x}", MARK + u32::from(tunnel)),
        (4857 + u32::from(tunnel)).to_string(),
    )
}

/// Whether this end's table is in the kernel. Something that empties the
/// machine's whole ruleset, a firewall restarting for one, takes it along.
pub fn has_table() -> bool {
    Command::new("nft")
        .args(["list", "table", "inet", "homewarp"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What keeps servers from the home they run in, whether or not there is a
/// tunnel: a table of its own, so that it is there before a VPS is connected
/// and stays when one is disconnected. A server reaches neither this machine
/// nor the networks behind it; what Docker published is reached as before.
///
/// [`home_ruleset`] says the same of `bridge` while the tunnel is up. The two
/// agree, and either alone is enough.
pub fn keep_ruleset(bridge: &str) -> Result<String, Error> {
    if !named_well(bridge) {
        return Err(Error::Interface(bridge.to_owned()));
    }
    Ok(format!(
        r#"table inet homewarp_keep
delete table inet homewarp_keep
table inet homewarp_keep {{
  chain input {{
    type filter hook input priority filter; policy accept;
    iifname "{bridge}" ct state established,related accept
    iifname "{bridge}" counter drop
  }}
  chain forward {{
    type filter hook forward priority filter - 1; policy accept;
    iifname "{bridge}" ip daddr {{ 10.0.0.0/8, 100.64.0.0/10, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }} ct state new counter drop
  }}
}}
"#
    ))
}

/// Whether the table of [`keep_ruleset`] is in the kernel.
pub fn has_keep_table() -> bool {
    Command::new("nft")
        .args(["list", "table", "inet", "homewarp_keep"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// How many packets have arrived from the tunnel for the port of the probe
/// that [`home_ruleset`] was last given.
pub fn probe_packets() -> Result<u64, Error> {
    counter("probe")
}

/// How many packets a counter of this end's table has counted.
fn counter(name: &str) -> Result<u64, Error> {
    let done = Command::new("nft")
        .args(["list", "counter", "inet", "homewarp", name])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !done.status.success() {
        let said = String::from_utf8_lossy(&done.stderr);
        return Err(Error::Refused(said.trim().to_owned()));
    }
    let said = String::from_utf8_lossy(&done.stdout);
    let mut words = said.split_whitespace();
    words
        .by_ref()
        .find(|word| *word == "packets")
        .and_then(|_| words.next())
        .and_then(|count| count.parse().ok())
        .ok_or_else(|| Error::Refused(format!("nft said of the counter: {}", said.trim())))
}

/// What has gone through each forwarded port since the table was last put in
/// the kernel, as the sets of [`gate_ruleset`] have counted it. Nothing where
/// the table was made without them.
pub fn counted() -> Vec<Through> {
    let mut all = Vec::new();
    for (set, protocol) in [
        ("through_tcp", Protocol::Tcp),
        ("through_udp", Protocol::Udp),
    ] {
        let listed = Command::new("nft")
            .args(["list", "set", "inet", "homewarp", set])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let Ok(listed) = listed else { continue };
        if listed.status.success() {
            let listing = String::from_utf8_lossy(&listed.stdout);
            all.extend(through(&listing).into_iter().map(|(port, bytes)| Through {
                port,
                protocol,
                bytes,
            }));
        }
    }
    all
}

/// Reads the ports and their counts off what nft lists of such a set:
/// `elements = { 25565 counter packets 12 bytes 3400, 27015 counter packets 0 bytes 0 }`.
fn through(listing: &str) -> Vec<(u16, u64)> {
    let Some((_, elements)) = listing.split_once("elements = {") else {
        return Vec::new();
    };
    let elements = elements.split('}').next().unwrap_or_default();
    elements
        .split(',')
        .filter_map(|element| {
            let mut words = element.split_whitespace();
            let port = words.next()?.parse().ok()?;
            let bytes = words
                .skip_while(|word| *word != "bytes")
                .nth(1)
                .and_then(|count| count.parse().ok())
                .unwrap_or(0);
            Some((port, bytes))
        })
        .collect()
}

/// The interface this machine's default route leaves by: on a VPS, the one
/// players arrive on. None if there is no default route.
pub fn default_interface() -> Option<String> {
    default_route(&fs::read_to_string("/proc/net/route").ok()?)
}

/// Reads the kernel's table of routes: a line of headings, then for each route
/// its interface, its destination in hexadecimal and, further on, its metric
/// and its mask.
fn default_route(routes: &str) -> Option<String> {
    routes
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let metric: u32 = fields.get(6)?.parse().ok()?;
            let interface = (*fields.first()?).to_owned();
            (fields.get(1) == Some(&"00000000") && fields.get(7) == Some(&"00000000"))
                .then_some((metric, interface))
        })
        .min()
        .map(|(_, interface)| interface)
}

/// A network this machine is already on that has some of the same addresses
/// as `network`, as `eth0 10.213.77.0/24`: one that the tunnel, given those
/// addresses, would take from the machine or lose to it. The tunnel's own
/// interface is not counted. None if there is none, or if it cannot be told.
pub fn network_in_the_way(network: (Ipv4Addr, u8)) -> Option<String> {
    in_the_way(&fs::read_to_string("/proc/net/route").ok()?, network)
}

/// Reads the same table of routes for one that shares addresses with `network`.
fn in_the_way(routes: &str, (network, length): (Ipv4Addr, u8)) -> Option<String> {
    let wanted = u32::from(network);
    let wanted_mask = u32::MAX.checked_shl(32 - u32::from(length)).unwrap_or(0);
    routes.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let interface = *fields.first()?;
        // The kernel writes an address here with its bytes backwards.
        let read = |field: &&str| u32::from_str_radix(field, 16).ok().map(u32::swap_bytes);
        let (destination, mask) = (read(fields.get(1)?)?, read(fields.get(7)?)?);
        // The default route covers every address and is in nobody's way.
        if mask == 0 || is_tunnel(interface) {
            return None;
        }
        // Two networks share addresses when they agree as far as the shorter
        // of their two prefixes goes.
        let both = mask & wanted_mask;
        (destination & both == wanted & both).then(|| {
            format!(
                "{interface} {}/{}",
                Ipv4Addr::from(destination),
                mask.count_ones()
            )
        })
    })
}

/// Removes everything this crate makes at a Gate, and at a home with the one
/// tunnel: the table, the way back and the interface. What is not there is
/// passed over.
pub fn take_down() {
    take_tunnel_down(0);
    take_table_down();
}

/// Removes one of home's tunnels: its way back and its interface. The table
/// is home's to make anew for the tunnels that are left, or to take down.
pub fn take_tunnel_down(tunnel: u8) {
    let (mark, table) = way_back(tunnel);
    let _ = ip(&["rule", "del", "fwmark", &mark, "lookup", &table]);
    let _ = ip(&["link", "del", &interface(tunnel)]);
}

/// Removes this end's table.
pub fn take_table_down() {
    let _ = apply("table inet homewarp\ndelete table inet homewarp\n");
}

/// Which of home's tunnels have an interface in the kernel now.
pub fn tunnels_up() -> Vec<u8> {
    (0..TUNNELS)
        .filter(|tunnel| {
            Path::new("/sys/class/net")
                .join(interface(*tunnel))
                .exists()
        })
        .collect()
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

    use super::{
        Error, ProbeForward, default_route, gate_ruleset, gate_ruleset_uncounted, through,
    };

    const HOME: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 2);

    fn desired(mode: Mode, forwards: &[(u16, Protocol)]) -> Desired {
        Desired {
            generation: 1,
            mode,
            forwards: forwards
                .iter()
                .map(|&(port, protocol)| Forward { port, protocol })
                .collect(),
            new_per_second: None,
        }
    }

    #[test]
    fn a_guard_shuts_the_vps_itself_but_for_what_is_open_and_touches_nothing_else() {
        use homewarp_proto::Open;

        let plain = gate_ruleset(
            "eth0",
            HOME,
            &desired(Mode::Transparent, &[(25565, Protocol::Tcp)]),
            None,
        )
        .unwrap();
        let open = Open {
            tcp: vec![443, 22, 80, 80],
            udp: vec![51820],
        };
        let guarded = super::guarded(&plain, "eth0", &open).unwrap();
        // Everything that was there is there still, and the table is whole.
        assert!(guarded.starts_with(plain.trim_end().strip_suffix('}').unwrap()));
        assert_eq!(guarded.matches('{').count(), guarded.matches('}').count());
        assert!(guarded.ends_with("}\n"));
        assert!(
            guarded.contains("set guard_tcp { type inet_service; elements = { 22, 80, 443 } }")
        );
        assert!(guarded.contains("set guard_udp { type inet_service; elements = { 51820 } }"));
        // Only what arrives on the public interface for the machine itself.
        let chain = guarded.split_once("chain guard {").unwrap().1;
        let order = [
            "type filter hook input priority filter - 5; policy accept;",
            "iifname != \"eth0\" accept",
            "ct state established,related accept",
            "meta l4proto { icmp, ipv6-icmp } accept",
            "tcp dport 22 ct state new add @guard_ssh { ip saddr limit rate over 12/minute burst 12 packets }",
            // Where the list of addresses is full, the rest share one allowance.
            "tcp dport 22 ct state new ip saddr != @guard_ssh limit rate over 12/minute burst 12 packets counter name \"guard_dropped\" drop",
            "tcp dport 22 ct state new ip6 saddr != @guard_ssh6 limit rate over 12/minute burst 12 packets counter name \"guard_dropped\" drop",
            "tcp dport @guard_tcp accept",
            "udp dport @guard_udp accept",
            "counter name \"guard_dropped\" drop\n  }",
        ];
        let mut rest = chain;
        for rule in order {
            let (_, after) = rest
                .split_once(rule)
                .unwrap_or_else(|| panic!("{rule} is missing or out of order"));
            rest = after;
        }
        // Nothing open at all is still a table nft reads.
        let shut = super::guarded(&plain, "eth0", &Open::default()).unwrap();
        assert!(shut.contains("set guard_tcp { type inet_service; }"));
        assert!(matches!(
            super::guarded(&plain, "eth0\" accept; #", &open),
            Err(Error::Interface(_))
        ));
        assert!(super::guarded("no table here", "eth0", &open).is_err());
    }

    #[test]
    fn the_limit_on_new_connections_is_the_one_asked_for_or_thirty_a_second() {
        let limit = |asked: Option<u32>| {
            let mut desired = desired(Mode::Transparent, &[(25565, Protocol::Tcp)]);
            desired.new_per_second = asked;
            let rules = gate_ruleset("eth0", HOME, &desired, None).unwrap();
            let (_, after) = rules.split_once("limit rate over ").unwrap();
            after.split(" }").next().unwrap().to_owned()
        };
        assert_eq!(limit(None), "30/second burst 60 packets");
        // An address the list had no room for is held to the same, with every other such.
        let rules = gate_ruleset(
            "eth0",
            HOME,
            &desired(Mode::Transparent, &[(25565, Protocol::Tcp)]),
            None,
        )
        .unwrap();
        let chain = rules.split_once("chain to_home {").unwrap().1;
        let (counted, shared) = chain
            .split_once("ct state new add @newconn { ip saddr limit rate over 30/second burst 60 packets } counter drop\n")
            .unwrap();
        assert!(!counted.contains("drop"));
        assert!(shared.trim_start().starts_with(
            "ct state new ip saddr != @newconn limit rate over 30/second burst 60 packets counter drop\n"
        ));
        assert_eq!(limit(Some(5)), "5/second burst 10 packets");
        assert_eq!(limit(Some(2000)), "2000/second burst 4000 packets");
        // None at all would be no server at all, and is not what is written.
        assert_eq!(limit(Some(0)), "1/second burst 2 packets");
        assert_eq!(
            limit(Some(u32::MAX)),
            format!("{0}/second burst {0} packets", u32::MAX)
        );
    }

    #[test]
    fn forwards_each_port_to_the_same_port_at_home() {
        let asked = [
            (25565, Protocol::Tcp),
            (25565, Protocol::Udp),
            (27015, Protocol::Udp),
        ];
        let rules = gate_ruleset("eth0", HOME, &desired(Mode::Transparent, &asked), None).unwrap();
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
        // The tunnel's own addresses are not answered on the public interface.
        assert!(rules.contains(
            "chain own {\n    type filter hook input priority filter - 10; policy accept;\n    iifname \"eth0\" ip daddr 10.213.77.0/30 counter drop\n  }"
        ));
    }

    #[test]
    fn counts_what_goes_through_each_port_where_nft_can() {
        let asked = [
            (25565, Protocol::Tcp),
            (25565, Protocol::Udp),
            (27015, Protocol::Udp),
        ];
        let wanted = desired(Mode::Transparent, &asked);
        let rules = gate_ruleset("eth0", HOME, &wanted, None).unwrap();
        assert!(
            rules.contains("set through_tcp { type inet_service; counter; elements = { 25565 } }")
        );
        assert!(rules.contains(
            "set through_udp { type inet_service; counter; elements = { 25565, 27015 } }"
        ));
        // To home once it has been let through, and from home whatever answers.
        let to_home = rules.find("chain to_home").unwrap();
        assert!(rules[to_home..].contains("\n    udp dport @through_udp\n"));
        assert!(rules[..to_home].contains("iifname \"homewarp0\" tcp sport @through_tcp\n"));
        // With nothing forwarded the sets are there, and empty.
        let none = gate_ruleset("eth0", HOME, &Desired::default(), None).unwrap();
        assert!(none.contains("set through_tcp { type inet_service; counter; }"));

        // For an nft that keeps no such counts, the same table without them.
        let plain = gate_ruleset_uncounted("eth0", HOME, &wanted, None).unwrap();
        assert!(!plain.contains("through_"));
        assert!(plain.contains("iifname \"eth0\" dnat ip to tcp dport map @fwd_tcp"));
        let without: String = rules
            .lines()
            .filter(|line| !line.contains("through_"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(plain, without);
    }

    #[test]
    fn reads_the_counts_off_what_nft_lists() {
        let listing = "table inet homewarp {\n\tset through_udp {\n\t\ttype inet_service\n\t\tcounter\n\t\telements = { 25565 counter packets 12 bytes 3400,\n\t\t\t     27015 counter packets 0 bytes 0 }\n\t}\n}\n";
        assert_eq!(through(listing), [(25565, 3400), (27015, 0)]);
        // Nothing forwarded, and an nft that listed no counts.
        assert_eq!(
            through(
                "table inet homewarp {\n\tset through_tcp {\n\t\ttype inet_service\n\t\tcounter\n\t}\n}\n"
            ),
            []
        );
        assert_eq!(
            through("elements = { 25565, 27015 }"),
            [(25565, 0), (27015, 0)]
        );
    }

    #[test]
    fn with_nothing_to_forward_the_maps_are_empty_and_still_there() {
        let rules = gate_ruleset("ens3", HOME, &Desired::default(), None).unwrap();
        assert!(rules.contains("map fwd_tcp { type inet_service : ipv4_addr . inet_service; }"));
        assert!(!rules.contains("elements"));
    }

    #[test]
    fn nat_mode_is_one_rule_more() {
        let nat = gate_ruleset("eth0", HOME, &desired(Mode::Nat, &[]), None).unwrap();
        assert!(nat.contains("oifname \"homewarp0\" masquerade"));
    }

    #[test]
    fn a_probe_is_one_more_tcp_port_sent_to_the_port_home_chose() {
        let probe = ProbeForward {
            public_port: 40001,
            home_port: 50002,
        };
        let asked = [(25565, Protocol::Tcp)];
        let rules = gate_ruleset(
            "eth0",
            HOME,
            &desired(Mode::Transparent, &asked),
            Some(probe),
        )
        .unwrap();
        assert!(
            rules.contains(
                "elements = { 25565 : 10.213.77.2 . 25565, 40001 : 10.213.77.2 . 50002 }"
            )
        );
        assert!(rules.contains("map fwd_udp { type inet_service : ipv4_addr . inet_service; }"));
    }

    #[test]
    fn home_counts_what_arrives_for_a_probe_and_opens_nothing_for_it() {
        let plain = super::home_ruleset("homewarp-br", &[0], None, None).unwrap();
        assert!(!plain.contains("probe"));
        let probing = super::home_ruleset("homewarp-br", &[0], Some((0, 50002)), None).unwrap();
        assert!(probing.contains("counter probe { packets 0 bytes 0 }"));
        // Counted where it arrives, before anything has had the chance to drop it.
        let counted = probing
            .find("iifname \"homewarp0\" tcp dport 50002 counter name \"probe\"")
            .unwrap();
        assert!(counted < probing.find("ct state new ct mark set").unwrap());
        // And that is the only difference: a probe comes in as a player does.
        let without: String = probing
            .lines()
            .filter(|line| !line.contains("probe"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(without, plain);
    }

    #[test]
    fn a_new_key_is_kept_as_the_kernel_will_report_it() {
        use defguard_wireguard_rs::key::Key;

        for _ in 0..32 {
            let (private, public) = super::new_keypair();
            let key = Key::try_from(private.as_str()).unwrap();
            assert_eq!(super::clamped(&key), key);
            assert_eq!(key.public_key().to_string(), public);
        }
        // The three bits the kernel sets its own way, whatever it is given.
        let clamped = super::clamped(&Key::new([0xff; 32])).as_array();
        assert_eq!((clamped[0], clamped[31]), (0xf8, 0x7f));
        let clamped = super::clamped(&Key::new([0; 32])).as_array();
        assert_eq!((clamped[0], clamped[31]), (0, 0x40));
    }

    #[test]
    fn a_network_with_the_tunnels_addresses_is_found_to_be_in_the_way() {
        use super::in_the_way;

        let tunnel = (Ipv4Addr::new(10, 213, 77, 0), 30);
        let table = |routes: &str| {
            format!(
                "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n{routes}"
            )
        };
        // An ordinary home: a default route, the LAN, Docker's bridge, the
        // servers' own bridge, and the tunnel itself once it is up.
        let ordinary = table(
            "eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
             eth0\t0001A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
             docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n\
             homewarp-br\t0050D50A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
             homewarp0\t004DD50A\t00000000\t0001\t0\t0\t0\tFCFFFFFF\t0\t0\t0\n",
        );
        assert_eq!(in_the_way(&ordinary, tunnel), None);
        // A LAN that happens to be 10.213.77.0/24, a wider one that holds it,
        // and a single address inside it.
        for (route, said) in [
            (
                "eth0\t004DD50A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n",
                "eth0 10.213.77.0/24",
            ),
            (
                "tun1\t0000D50A\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n",
                "tun1 10.213.0.0/16",
            ),
            (
                "wg9\t0000000A\t00000000\t0001\t0\t0\t0\t000000FF\t0\t0\t0\n",
                "wg9 10.0.0.0/8",
            ),
            (
                "eth1\t024DD50A\t00000000\t0005\t0\t0\t0\tFFFFFFFF\t0\t0\t0\n",
                "eth1 10.213.77.2/32",
            ),
        ] {
            assert_eq!(in_the_way(&table(route), tunnel).as_deref(), Some(said));
        }
        // Next door is not in the way.
        let beside = table("eth0\t044DD50A\t00000000\t0001\t0\t0\t100\tFCFFFFFF\t0\t0\t0\n");
        assert_eq!(in_the_way(&beside, tunnel), None);
        assert_eq!(in_the_way("", tunnel), None);
    }

    #[test]
    fn finds_the_interface_of_the_default_route() {
        let routes = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
            docker0\t000011AC\t00000000\t0001\t0\t0\t0\t0000FFFF\t0\t0\t0\n\
            wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
            eth0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n";
        assert_eq!(default_route(routes).as_deref(), Some("eth0"));
        assert_eq!(default_route("Iface\tDestination\n"), None);
    }

    #[test]
    fn servers_are_kept_from_home_with_or_without_a_tunnel() {
        let rules = super::keep_ruleset("homewarp-br").unwrap();
        // A table of its own, replaced whole, that says nothing of the tunnel.
        assert!(rules.starts_with("table inet homewarp_keep\ndelete table inet homewarp_keep\n"));
        assert!(!rules.contains("homewarp0"));
        assert!(rules.contains(
            "iifname \"homewarp-br\" ip daddr { 10.0.0.0/8, 100.64.0.0/10, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 } ct state new counter drop"
        ));
        assert!(rules.contains("iifname \"homewarp-br\" ct state established,related accept"));
        assert!(rules.contains("iifname \"homewarp-br\" counter drop"));
        assert!(super::keep_ruleset("br0; flush ruleset").is_err());
    }

    #[test]
    fn home_marks_what_comes_from_the_gate_and_lets_in_only_what_docker_published() {
        let rules = super::home_ruleset("homewarp-br", &[0], None, None).unwrap();
        // The mark goes on a connection as it arrives, and onto its replies only.
        assert!(rules.contains("iifname \"homewarp0\" ct state new ct mark set 0x4857"));
        assert!(
            rules.contains("iifname != { \"homewarp0\" } ct mark { 0x4857 } meta mark set ct mark")
        );
        assert!(
            rules.contains(
                "iifname { \"homewarp0\" } oifname \"homewarp-br\" ct status dnat accept"
            )
        );
        assert!(rules.contains(
            "iifname { \"homewarp0\", \"homewarp-br\" } ct state established,related accept"
        ));
        assert!(matches!(
            super::home_ruleset("br\" accept; #", &[0], None, None),
            Err(Error::Interface(_))
        ));
    }

    #[test]
    fn each_tunnel_has_a_mark_of_its_own_so_that_a_reply_leaves_by_the_one_it_came_in_by() {
        // Given in any order, and one of them twice.
        let rules =
            super::home_ruleset("homewarp-br", &[2, 0, 2], Some((2, 50002)), Some(8443)).unwrap();
        assert!(rules.contains("iifname \"homewarp0\" ct state new ct mark set 0x4857"));
        assert!(rules.contains("iifname \"homewarp2\" ct state new ct mark set 0x4859"));
        assert!(!rules.contains("homewarp1"));
        assert!(rules.contains(
            "iifname != { \"homewarp0\", \"homewarp2\" } ct mark { 0x4857, 0x4859 } meta mark set ct mark"
        ));
        // What a tunnel may not do, no tunnel may.
        assert!(rules.contains("iifname { \"homewarp0\", \"homewarp2\" } counter drop"));
        assert!(rules.contains(
            "iifname { \"homewarp0\", \"homewarp2\" } meta l4proto tcp ct original proto-dst 8443 ct status dnat accept"
        ));
        // The probe is counted on the tunnel of the VPS that is looked at.
        assert!(rules.contains("iifname \"homewarp2\" tcp dport 50002 counter name \"probe\""));
        assert_eq!(super::way_back(0), ("0x4857".to_owned(), "4857".to_owned()));
        assert_eq!(super::way_back(2), ("0x4859".to_owned(), "4859".to_owned()));
        // No tunnel at all is no table, and there are only so many.
        assert!(super::home_ruleset("homewarp-br", &[], None, None).is_err());
        assert!(super::home_ruleset("homewarp-br", &[super::TUNNELS], None, None).is_err());
        assert!(super::is_tunnel("homewarp7") && !super::is_tunnel("homewarp-br"));
    }

    #[test]
    fn home_lets_the_tunnel_in_to_the_panels_tls_port_once_it_has_a_name_and_to_no_other() {
        let named = super::home_ruleset("homewarp-br", &[0], None, Some(8443)).unwrap();
        // Among what passes through this machine, ahead of the rule that drops
        // whatever else comes from the tunnel. What comes to the machine itself
        // is dropped as before.
        let (to_here, through) = named.split_once("chain forward").unwrap();
        let let_in = through
            .find("iifname { \"homewarp0\" } meta l4proto tcp ct original proto-dst 8443 ct status dnat accept")
            .unwrap();
        assert!(
            let_in
                < through
                    .find("iifname { \"homewarp0\" } counter drop")
                    .unwrap()
        );
        assert!(!to_here.contains("8443"));
        // And that is the only difference a name makes.
        let without: String = named
            .lines()
            .filter(|line| !line.contains("proto-dst"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(
            without,
            super::home_ruleset("homewarp-br", &[0], None, None).unwrap()
        );
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
                gate_ruleset(odd, HOME, &Desired::default(), None),
                Err(Error::Interface(_))
            ));
        }
    }
}
