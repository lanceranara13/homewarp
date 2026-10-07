# Homewarp — project plan

> Name decided 2026-10-05 (see §2). Status: Phases 0 to 5 are done (§11), each but for
> what is said beside it. The tunnel is proven on a real VPS and against a real Docker
> daemon, eggs install and run from the panel, a VPS is connected with one command, and a
> server's files, backups, schedules and further accounts are in the panel, which is
> staged on the homelab at port 3600. The panel can be given a name and reached from
> anywhere over TLS that is ended at home (tried on the owner's VPS with
> `homewarp.apixels.net`); sign-ins are limited and take a second step or a passkey; the
> VPS itself can be hardened on trial. §6 has the security model held against the code.
> Phase 6 is done but for updating by itself: 1.0.0 was released on 2026-10-07, signed
> with the owner's key and served from `https://lanceranara13.github.io/homewarp` (the
> licence is AGPL-3.0-or-later since the same day). Of Phase 7, "Later", five things
> are built and tried (who is on a Minecraft server, sleep and wake, notices, mods from
> Modrinth, a store elsewhere for backups) and three are not, each with its reason.
> Phase 8, the same day: several VPSes at once, each server reached through one of
> them; traffic drawn as it happens; and updates from the panel, with betas for those
> who ask. Tried in the lab and beside the owner's own Homewarp; not yet released.

## 1. What it is

A self-hosted game server panel where the servers run **at home** and the public
entrance is **one cheap VPS**.

```
 players ──► VPS public IP ══ encrypted tunnel ══► home server ──► game containers
             (homewarp-gate)                           (homewarp core + panel)
```

- The VPS is a thin, disposable relay. It holds no game data, no database, no panel.
- The home server runs everything that matters: the panel, the API, Docker, the worlds.
- Home needs **no port forwarding**, works behind CGNAT, and its IP is never published.
- Games are defined by templates that are import-compatible with Pterodactyl/Pelican
  "eggs", so the existing catalogue of hundreds of games and apps works from day one.

Target experience: **two commands** (one at home, one on the VPS), a three-step wizard,
and a joinable Minecraft server in under five minutes.

## 2. Name

**Homewarp** — chosen by the owner on 2026-10-05.

`/home` and `/warp` are the commands players already use to get somewhere fast. The
name says what the product does without explanation: friends warp to the server in
your house. It works for any game, not only Minecraft.

What was checked on 2026-10-05:

| Check | Result |
|---|---|
| crates.io | No crate with this name. |
| Domains | `.dev` and `.app` are not registered (registry RDAP lookup, verified against a nonsense control name). Not registered does not rule out premium pricing. |
| GitHub | One unrelated repository with one star. |

**Not checked:** trademarks, `.com` / `.gg`, GitHub organisation name. Do that before
the first public push.

**Found later the same day — similar name in the same niche:** a hosted tunnel service
for game servers called **Portwarp** (`portwarp.com`, free tier plus a $2.99/month plan)
exists. It is a different kind of product (a hosted service, not self-hosted software),
but the name is close and the audience is the same. The original check looked only for
exact matches. Decide whether this is acceptable before the name goes public.

Names considered and passed over: Adit (the first working name — free as a crate, but
it needs explaining every time), Hearthgate, Sethome (collides with Minecraft plugins of
the same name), Spawnpoint, Homefield, Homerun, Hideout; and, with the crate name already
taken, Petrel, Burrow, Wombat, Bilby, Roost, Sett, Homeport and Porthole.

How to describe it:

- **One sentence:** Host game servers on your own PC at home; friends join through a
  small VPS, so your home address stays hidden and your router needs no setup.
- **Six words:** Home-hosted game servers, one VPS front door.
- **For people who know the space:** Pelican with the tunnel built in.
- **For anyone:** Your PC at home is the game room. The VPS is a PO box: everyone sends
  to the PO box, nobody learns your street address.

Vocabulary used in the rest of this document:

| Term | Meaning |
|---|---|
| **Core** | The home-side binary `homewarp`: panel, API, Docker runtime, tunnel client. |
| **Gate** | The VPS-side binary `homewarp-gate`: tunnel endpoint + port forwarder. |
| **Template** | A game/app definition. Imported from eggs or written natively. |
| **Forward** | One public port on the Gate mapped to one server port at home. |

## 3. Goals and non-goals

**Goals**

1. Easy: no PHP, no MySQL, no Redis, no reverse-proxy config. One binary + one SQLite
   file at home; one static binary on the VPS.
2. Secure: a compromised VPS must not mean a compromised home network.
3. Fast: kernel-path forwarding, no TCP-over-TCP, no userspace copy per packet.
4. One VPS is enough, and the cheapest tier at that: it must run on 1 vCPU, with a
   512 MB floor and the owner's ceiling of 2 GB (§7.1).
5. Many games, not only Minecraft; TCP **and** UDP as first-class.
6. Real client IPs reach the game server (bans, logs, anti-cheat keep working).

**Non-goals for v1**

- Multi-node clusters, reselling/billing, multiple organisations.
- Windows or macOS as the home host (Linux + Docker only).
- Hosting the game containers on the VPS itself.
- A plugin system.

## 4. Prior art and where this fits

| Project | What it is | Gap this project fills |
|---|---|---|
| Pterodactyl / Pelican | PHP panel + Go "Wings" daemon. Pelican needs PHP 8.3–8.5 with ten extensions, a web server, Composer, a database, plus Wings. | Heavy install; no answer for "my server is at home behind NAT". |
| Calagopus | Rust (Axum) panel + Rust Wings, React front end, PostgreSQL, egg-compatible, MIT. | Still a panel+daemon+database stack; no tunnel. Proves the Rust + egg-compat approach is viable. |
| Pangolin | Self-hosted tunnelled reverse proxy (WireGuard, Traefik, site connector). Raw TCP/UDP resources supported. | HTTP-first; not a game panel. Today people pair it with Pelican — two products, two installs. |
| playit.gg / frp / rathole | Tunnels only. | No panel; client IP usually lost. |
| Crafty / MCSManager / AMP | Panels, mostly Minecraft-centric or commercial. | No tunnel. |

No project found combines **panel + tunnel** in one install. That is the niche.

## 5. Architecture

### 5.1 Topology decision: brain at home, VPS as a dumb pipe

Pterodactyl puts the panel on the public machine and the daemon on the node. Homewarp
inverts that: **the panel lives at home**, and the VPS only forwards packets.

Why:

- The VPS is the exposed, least-trusted box. Keeping it stateless means compromising
  it yields a tunnel endpoint and nothing else.
- Re-creating or moving the VPS is one command; no backups or migrations.
- The panel sits next to Docker and the files — file manager, backups and console need
  no network hop.
- Two components instead of four (panel, daemon, database, cache).
- If home is down the servers are down anyway, so nothing is lost by the panel being there.

Commands flow **Core → Gate only**. The Gate never issues commands to home.

### 5.2 Components

**`homewarp` (Core, home)** — one process:

- HTTP API + embedded web UI
- SQLite database
- Docker runtime driver (create/start/stop, console attach, stats)
- Template engine (egg import, variables, config-file patching, install scripts)
- File manager, backups, scheduler
- Tunnel client: WireGuard interface, home-side firewall and routing rules
- Gate controller: pushes desired state, polls health

**`homewarp-gate` (Gate, VPS)** — one small static process:

- WireGuard endpoint
- Applies a declarative set of forwards as nftables rules
- Reports health and per-port counters
- Nothing else. No database, no Docker, no web UI.

### 5.3 Tunnel: kernel WireGuard + nftables DNAT

Chosen over a userspace reverse proxy (frp/rathole style) because:

| | Kernel WG + DNAT | Userspace relay |
|---|---|---|
| UDP games (Bedrock, Valheim, Palworld, Factorio, CS2…) | Native | Needs session tracking per game |
| Real client IP at the game server | Yes, every protocol | Only with PROXY protocol, which most games lack |
| Throughput / CPU | Kernel path, no copies | Copy per packet, context switches |
| TCP behaviour | End-to-end between player and server | TCP terminated at VPS, or TCP-over-TCP |
| Root needed | Yes (both sides) | No |

Root is already required at home (Docker) and is available on any VPS, so the one
downside does not cost anything here. If a VPS lacks the WireGuard module (old
OpenVZ), fall back to a userspace WireGuard implementation behind the same config.

**Path of a packet (transparent mode, the default):**

1. Player → `VPS_IP:25565`.
2. Gate: `nft` DNAT → `HOME_TUNNEL_IP:25565`. **No masquerade**, so the source stays
   the player's real IP.
3. WireGuard carries it home.
4. Home: Docker's published port hands it to the container.
5. Reply: container → host. Conntrack restores the connection mark, and a
   policy-routing rule sends marked traffic back into the tunnel rather than out
   through the home ISP.
6. Gate reverses the DNAT and replies from `VPS_IP`.

Sketch — Gate side:

```nft
table inet homewarp {
  map fwd_tcp { type inet_service : ipv4_addr . inet_service; }
  map fwd_udp { type inet_service : ipv4_addr . inet_service; }
  set newconn { type ipv4_addr; size 65535; flags dynamic,timeout; timeout 1m; }

  chain prerouting {
    type nat hook prerouting priority dstnat;
    iifname $WAN dnat ip to tcp dport map @fwd_tcp
    iifname $WAN dnat ip to udp dport map @fwd_udp
  }
  chain forward {
    type filter hook forward priority filter;
    oifname "homewarp0" ct status dnat goto to_home
    oifname "homewarp0" drop                 # only forwarded ports enter the tunnel
    iifname "homewarp0" ct state new drop    # home does not use the Gate as an exit
  }
  chain to_home {
    ct state new add @newconn { ip saddr limit rate over 30/second burst 60 packets } drop   # per source
    tcp flags syn tcp option maxseg size set rt mtu
  }
}
```

Sketch — home side:

```nft
table inet homewarp {
  chain mark_in {
    type filter hook prerouting priority mangle;
    iifname "homewarp0" ct state new ct mark set 0x4857           # remember: this flow came from the Gate
    iifname != "homewarp0" ct mark 0x4857 meta mark set ct mark   # replies, and only replies, get the routing mark
  }
  chain input {
    type filter hook input priority filter;
    iifname { "homewarp0", "homewarp-br" } ct state established,related accept
    iifname "homewarp0" drop                              # the Gate may not reach host services
    iifname "homewarp-br" drop                            # nor may a game container
  }
  chain forward {
    type filter hook forward priority filter - 1;
    iifname "homewarp0" oifname "homewarp-br" ct status dnat accept   # from the tunnel: published game ports
    iifname "homewarp0" drop                                          # and nothing else
    iifname "homewarp-br" ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 } ct state new drop
    oifname "homewarp0" tcp flags syn tcp option maxseg size set rt mtu
  }
}
# ip rule add fwmark 0x4857 lookup 4857
# ip route add default dev homewarp0 table 4857
# sysctl net.ipv4.conf.homewarp0.rp_filter=2
```

These are sketches, not final rulesets. Both were loaded and exercised on the real VPS
on 2026-10-05 (§10): the Gate side as first written, on nft 1.0.2; the home side after one
correction. As first written, the second `mark_in` rule had no `iifname !=` and so marked
the inbound packet as well; policy routing then sent it straight back into the tunnel
and nothing arrived. The per-source limit loads, but its limiting was not exercised.

The lab (§10) then ran them against a real Docker daemon, and the sketches above are as
the lab runs them. What that added:

- **A game container could reach the home host itself.** The first sketch only stopped
  forwards to private ranges. A container talking to the host's own addresses (its
  bridge gateway, its LAN address) is input, not forward, and was not covered: SSH and
  the panel were reachable. The `homewarp-br` input rule closes that.
- **From the tunnel, only what Docker published.** `ct status dnat` accepts a flow only
  if a published-port rule translated it, instead of trusting Docker alone to drop the
  rest. (Docker 28 and later does drop it too, in its raw table.)
- **MSS is clamped on both sides**, each for what it sends into the tunnel.
- **The Gate forwards nothing but its forwards**, and nothing that starts at home.
- **Reverse-path filtering.** Players arrive on `homewarp0` from addresses that route
  elsewhere, so a host that filters strictly drops them. Loosening that one interface is
  enough, even with `all.rp_filter=1`.

Both sides own exactly one nftables table (`inet homewarp`) and replace it atomically.
They never edit Docker's rules.

**Host firewalls (found on the real VPS).** An accept in Homewarp's table cannot undo a
drop in another table on the same hook — a packet has to pass every base chain. A VPS
running ufw drops inbound by default, so the WireGuard port stays closed until ufw
itself allows it, and with ufw's stock "deny routed" the forwarded game traffic would be
dropped too. Owning one table is therefore not enough on its own: the Gate installer
must detect ufw, firewalld or any default-drop input chain, then either add the opening
through that tool or stop and print the exact command. The spike used one runtime rule
in `ufw-user-input`.

**NAT mode (fallback).** If the return path cannot be made to work on a given host
(exotic firewall, rootless Docker), the Gate masquerades instead. Everything works,
but servers see the Gate's tunnel address instead of player IPs. The UI states this
plainly on the Network page.

**Self-probe.** After the tunnel comes up, Core asks the Gate to open a temporary
port, then connects to `VPS_IP:port` over its normal internet route and checks that
the connection arrives through the tunnel with the home's own public IP as source.
Pass → transparent mode. Fail → NAT mode with an explanation. No guessing.

As built (2026-10-06), it has three answers and not two, because there are two ways
to fail and NAT mode mends only one of them. The probe has to come the way a player's
connection comes, or it says nothing about players: so the listener is Core's own
program, run once in a container on the servers' bridge with a port published as a
server's is (`homewarp probe-listen`, from the image Core itself runs from, which is the
one image sure to be there). What arrives from the tunnel for that port is counted
before anything on the machine can drop it. Then Core connects to the port the Gate
opened:

| What happened | What it means | What Core does |
|---|---|---|
| The connection arrived, from an address that is not the Gate's | Players' addresses reach servers, and replies get back | Transparent mode. *Preserved.* |
| Its first packet was counted, and it never completed | Packets arrive and the replies do not get back: the way back cannot be made to work on this machine | NAT mode, probed again to see that it works. *Hidden.* |
| Nothing was counted | The test never came through: a firewall in front of the VPS, most likely, shut for the port tried | Transparent mode, and it says that it could not check. *Not checked.* |

The page has a button to check again. Known gap: a home machine that has its public
address on itself, with no router between, cannot probe itself this way, since the
connection would arrive from one of its own addresses.

The first probe listened on the home machine itself, on the tunnel's address, and
passed in the lab. On the homelab it found "not checked" while a player was getting
through with their own address: the homelab runs ufw, which drops what arrives for the
machine itself unless it was told otherwise, and an accept in Homewarp's table cannot
undo that. A server's traffic is passed on, not received, and Docker's rules let it
by. It is the *Host firewalls* lesson above, met a second time at the other end.

**Addressing.** Tunnel `10.213.77.0/30` (and the next seven such blocks for further
VPSes, since Phase 8), game bridge `10.213.80.0/24`, both checked for
collisions at setup (the homelab already runs several VPN containers in `10.x`) and
configurable. WireGuard MTU 1380 with MSS clamping; IPv4 only in v1.

**Home initiates.** Core dials out to the Gate's UDP port with a 25-second keepalive,
so no inbound rule or port forward is ever needed at home, and a changing home IP
heals itself.

### 5.4 Control channel

The Gate's API listens **only on its tunnel address** — no extra public port, and
WireGuard has already authenticated the peer. A bearer token is required on top.

- `PUT /v1/state` — full desired state (forwards, rate limits, mode) with a generation
  number. Idempotent; the Gate reconciles and persists the last applied state so it
  survives a reboot before home reconnects.
- `GET /v1/status` — handshake age, per-forward packet/byte counters, conntrack count, load.
- `POST /v1/probe` — temporary port for the self-probe.
- `POST /v1/rotate` and `POST /v1/rotate/commit` — the change of keys on first contact (§5.5).

HTTP + JSON. No gRPC, no message broker.

As built (2026-10-06), `status` says the Gate's version, which state it carries out,
when it last heard from home and from what address, and the bytes through the tunnel.
Counters for each forward, the connection count and the load are not in it yet: they
belong with the traffic column of the Network page, which is not built either.

Both ends put back what something else takes away. Core does its whole setup again
every ten seconds, each step of which changes nothing when nothing is missing; the
Gate looks every thirty. A firewall restarting on the VPS empties the whole ruleset,
the Gate's table with it, and that is the case this is for.

### 5.5 Enrolling a VPS

1. Panel → Network → **Connect a VPS**. User types the VPS IP.
2. Panel shows one command: `curl -fsSL <installer> | sudo sh -s -- <JOIN_TOKEN>`.
   The token (valid 15 minutes) carries a *bootstrap* WireGuard key, Core's public key,
   a preshared key and the tunnel addresses.
3. The installer drops the static binary, writes a systemd unit, brings WireGuard up.
4. Core dials in. On first contact the Gate **generates a fresh keypair locally** and
   both sides switch to it; the bootstrap key and token become worthless.
5. Self-probe runs, mode is chosen, the wizard turns green.

The user never edits a config file and never copies a key by hand.

As built (2026-10-06):

- **The command is `homewarp-gate join <token>`**, run as root on a VPS that has the
  program already. There are no releases to download before Phase 6, so the one line
  that fetches the program waits until then; `scripts/dev.sh vps` copies it over.
  `join` writes the Gate's `config.json`, asks ufw for the three openings it needs
  where ufw is active (the tunnel's UDP port; the Gate's API on the tunnel's interface;
  routed traffic from the public interface into the tunnel), installs a systemd unit
  that is held to the network and its own directory, and starts it. Where firewalld runs
  instead, it is asked for the tunnel's port in the zone players arrive in, and for a
  zone of the Gate's own (`homewarp`) for the tunnel's interface, which lets in the
  Gate's API and nothing else; what a forward sends on needs no opening there, because
  firewalld passes what another table has redirected (Phase 6). `homewarp-gate leave`
  takes all of it away.
- **The token** is JSON in URL-safe base64, about 400 characters: the Gate's first
  private key, home's public key, a preshared key, the token for the Gate's API, the
  two ports, the two tunnel addresses, and when it runs out. Core stops dialling with
  what it carried the moment it runs out, which is what makes a leaked one worthless.
- **The change of keys is in two steps**, so that a lost answer cannot leave the two
  ends with different keys for good. `rotate`: the Gate makes a key that never leaves
  the VPS and a new API token, and takes a new preshared key from Core; it answers with
  the public half and the token, and goes on with the old ones. Core writes the new
  ones down. `commit`: the Gate writes its new config, answers, and switches half a
  second later. If that answer is lost, Core tries the new keys before giving up on the
  old; if the Gate was restarted in between and has forgotten, it says so and Core
  starts over. Afterwards nothing the token carried is in use but home's public key.
- **One Gate**, when this was written. Since Phase 8 a home has up to eight, each
  enrolled this way, one at a time.

### 5.6 Server runtime (egg-compatible)

Core reproduces the contract Wings gives to a container, so unmodified eggs and
"yolk" images work:

- Server directory bind-mounted at `/home/container`; install scripts run in the
  egg's install image with the same directory at `/mnt/server`.
- Environment: `STARTUP`, `SERVER_MEMORY`, `SERVER_IP`, `SERVER_PORT`, `TZ`,
  `P_SERVER_UUID`, `P_SERVER_LOCATION`, plus every template variable.
- Startup command with `{{VAR}}` substitution.
- Config-file patching before start: `properties`, `yaml`, `json`, `ini`, `xml`, `file`
  parsers, including wildcards and `{{server.allocations.default.port}}`-style lookups.
  (All but `xml` are written: §11, Phase 2.)
- "Started" detection from the `done` string; stop via command or `^C` / `^SIGTERM` etc.
- Variable validation: a Rust port of the Laravel rule subset eggs actually use
  (`required`, `nullable`, `string`, `numeric`, `integer`, `boolean`, `max`, `min`,
  `between`, `in`, `regex`, and the eight more that the egg corpus of §10 turned up).
- Feature flags such as `eula` (prompt to accept when the console asks).

Import formats: Pterodactyl `PTDL_v1` / `PTDL_v2` (JSON) and Pelican `PLCN_v1`–`v3`
(JSON and YAML). All are normalised into one internal schema that adds what eggs
lack — notably **protocol per port** (tcp / udp / both), which the Gate needs.

Lifecycle state machine: `installing → offline → starting → running → stopping`, with
`crashed` and automatic restart with back-off.

Container hardening (applied to every server): non-root user, `cap_drop: ALL`,
`no-new-privileges`, read-only root filesystem with tmpfs `/tmp`, PID limit, memory and
CPU limits, dedicated bridge with inter-container traffic off, **no route to the home LAN**.

**Names are looked up at `1.1.1.1` and `1.0.0.1`** (2026-10-06), as Wings has its servers
do. Left to Docker a server asks the home's own resolver, which is as a rule the router,
and "no route to the home LAN" means the router too: with the tunnel's rules in place a
server could look nothing up. To become a setting. Found on the homelab, whose resolver
is `192.168.1.1`, before it bit; the lab is offline and could not have shown it.

**Ports (2026-10-06).** A server has its first port, which is what `SERVER_PORT` and
`server.allocations.default.port` stand for, and up to sixteen further ones. Each is
open for TCP, UDP or both, published by Docker at home under its own number and
forwarded by the Gate for the same protocols. A port number belongs to one server. An
egg does not say which protocol its port speaks, so a port starts as both.

### 5.7 Data model (SQLite)

`users`, `sessions`, `api_keys`, `templates`, `template_variables`, `servers`,
`server_variables`, `allocations`, `gates`, `backups`, `schedules`, `schedule_tasks`,
`subusers`, `audit_log`, `settings`.

One file, WAL mode, embedded migrations. No external database.

A template is one row: the whole document the importer made, as JSON, with the egg it
came from beside it. Nothing is ever asked of a part of a template, and a better
importer can read the egg again. So there is no `template_variables` table; a server's
own values can be kept by variable name.

As built in Phase 4: the accounts let into a server are in `server_users`, each with
what it may do there; a schedule's steps are kept in the schedule's own row, for the
reason a template's variables are, so there is no `schedule_tasks`; and `traffic`, which
the list above lacks, holds what the Gate counted through each port, an hour to a row.
There are no API keys yet.

### 5.8 API and UI

- REST under `/api/v1`, OpenAPI generated from the Rust types, TypeScript client
  generated from OpenAPI — front end and back end cannot drift.
- One WebSocket per open server page: console output, stats, state changes.
- The web UI is a static bundle embedded in the `homewarp` binary.
- First paint uses a single request per page; stats, player counts and tunnel latency
  arrive over the socket afterwards and never block rendering.

See `DESIGN.md` for the interface.

### 5.9 Reaching the panel

| Mode | How | Default |
|---|---|---|
| LAN only | `http://<home-ip>:3600` | Yes — owner's choice, 2026-10-05 |
| Tailscale | Panel bound to the tailnet (or published with `tailscale serve`). Reachable from the owner's devices anywhere, invisible to the internet. | Opt-in |
| Public via Gate, with domain | Gate forwards the panel's port raw; **Core terminates TLS**. The VPS never sees plaintext. | Opt-in |
| Public via Gate, no domain | Same, using a Let's Encrypt IP-address certificate (GA since January 2026, 160-hour lifetime, auto-renewed). | Not built |

*As built (Phase 5), with a domain:* the panel's port is one of its own, 8443 unless the
deployment says otherwise, the same number on the VPS and at home. 443 was the plan, and on
a VPS with a web server 443 is that server's; so is 80, which is why the certificate is
asked for over HTTP-01 and not TLS-ALPN-01: Core has the answer to the authority's
question, the Gate puts it in a file on the VPS, and whatever has port 80 there serves the
file (the Gate itself, for as long as the asking takes, where nothing else has it). The
answer is no secret, and it is all the VPS is handed. At home the port is a third door's,
and the tunnel is let in to that door and to nothing else of the panel.

### 5.10 Tailscale (explored 2026-10-05)

Question: could Tailscale replace the built-in tunnel and make the VPS ↔ home link simpler?

| | Built-in WireGuard | Tailscale |
|---|---|---|
| Setup on the VPS | One command from the panel | Install + log in on both machines, then the same Homewarp command |
| Accounts / third parties | None | Tailscale account and coordination service, or self-hosted Headscale |
| Client IP at the game server | Preserved, every protocol | Lost: the standard recipe is DNAT + masquerade, so every player appears as the VPS's tailnet address |
| Data path | Kernel on both ends | Userspace `tailscaled` on both ends; fast when direct, slow through a DERP relay |
| Build effort | Higher (keys, routing, firewall) | Lower (forward to a `100.x` address) |

Findings:

- **Funnel cannot carry game traffic.** It listens only on 443, 8443 and 10000, TLS only, no UDP. A VPS is still required.
- **No documented way was found to keep real client IPs over Tailscale.** Every guide found forwards with masquerade. Real IPs would then depend on PROXY protocol per game (Paper/Velocity built in, Fabric/Forge via mods, most other games not at all). Whether an exit-node arrangement could avoid this is untested.
- **Embedding is not possible yet.** The official `tailscale-rs` library is an experimental preview (April 2026): relay-only data path, no NAT traversal, not audited. Tailscale would be a separate install on both machines.
- Tagged nodes have key expiry disabled by default, so server nodes do not silently drop off.
- The homelab does not run Tailscale today (no binary, interface or container).
- The owner's VPS already does (1.102.2). Its tailnet has a node named `home`, but that
  is another device on the home LAN acting as a subnet router for `192.168.1.0/24`, not
  the homelab; the VPS does not accept its routes.
- Until 2026-10-05 the VPS carried the forwards of an earlier Pelican setup: DNAT plus
  masquerade to a tailnet address. That is the recipe above, and it is why those servers
  never saw player addresses.

Decision (owner, 2026-10-05): **built-in WireGuard, with Tailscale as an option.** That is:

1. Built-in WireGuard stays the default for game traffic — it is the only option that preserves client IPs for every game and needs no account.
2. Add an **external-link mode**: the Gate forwards to any address the user supplies (a tailnet IP, an existing WireGuard peer, ZeroTier…) in NAT mode. Cheap to build, and it covers Tailscale and Headscale users.
3. Offer Tailscale as a **panel access** mode. This is where it is clearly the better tool.

Not yet tested on the real machines: the external-link mode over Tailscale. It needs the
homelab on the tailnet, or the VPS told to accept the subnet router's routes — both are
the owner's to grant.

## 6. Security model

| Threat | Mitigation |
|---|---|
| VPS is compromised | It holds no data or panel credentials. Home firewall lets tunnel traffic reach **only** published game ports on the game bridge — never the host, never the LAN. Worst case equals what the internet could already reach. |
| VPS is compromised, and the panel has a name that leads to it | Whoever reads the VPS's traffic reads ciphertext: TLS is ended at home, and the key never leaves it. Whoever *has* the VPS has the address the name leads to, and so could ask an authority for a certificate of their own for that name and stand between a browser and the panel. Two things are in the way. The session cookie is marked `Secure`, so it is not sent to the VPS's own web server over plain HTTP. And a **CAA record** for the name, which the Network page writes out with this Homewarp's account at the authority in it, has the authority refuse everybody else; that record is the owner's to add at their DNS host, and until it is there this row is only half true. A certificate got that way would also show in the public logs of certificates. |
| Game server is exploited (plugin RCE, Log4Shell-class bug) | Unprivileged, capability-less container; read-only rootfs; resource limits; blocked from RFC1918 ranges, so it cannot pivot into the home network. |
| Malicious template / install script | Runs only inside the install container under the same limits; never on the host. Import shows the script and images before confirming. |
| Panel account takeover | Argon2id, rate-limited login, TOTP (then passkeys), `HttpOnly`+`SameSite=Strict` sessions, CSRF protection, hashed API keys, audit log. Panel is LAN-only unless the user opts in. |
| File-manager path traversal | All file operations resolved beneath the server root with `openat2(RESOLVE_BENEATH)` semantics (`cap-std`), never string-joined paths. |
| DDoS | Only the VPS is hit; home IP stays private. Per-source new-connection rate limits in nftables on the Gate; provider-level protection sits in front. |
| Tampered binaries / updates | Signed releases; installer and self-updater verify before executing. |
| Gate firewall change locks the user out of SSH | Homewarp only owns its own table. The optional "harden this VPS" toggle uses commit-confirm: auto-revert after 60 s unless confirmed through the tunnel. |
| Join token leaks | 15-minute lifetime, and keys are rotated on first contact. |
| Home IP leaks through the game server | Optional: route game containers' outbound traffic through the Gate as well. |

Explicitly accepted: Core needs the Docker socket, which is root-equivalent on the
home host. This is inherent to every panel of this kind (Wings included).

### Held against what was built (2026-10-07)

The table above was written before any of it was built. Here it is row by row, read
against the code and against what the lab and the tests check. Four things were changed
because of the reading; they are marked *changed*.

| Row | As built | What it leaves |
|---|---|---|
| VPS is compromised | Holds. Home's rules let the tunnel in to what Docker published on the servers' bridge and, while the panel has a name, to the door of the panel over TLS, and to nothing else; the lab takes the Gate over, widens what it may send, and tries. What a Gate answers is read with a limit on its size and its time. *Changed:* what a Gate says went through a port is kept only for ports this Core asked it to forward. A Gate that named every port there is would have been given a row for each, every hour. | It sees what players send and can change it, as any relay can, and it knows the home's address. |
| VPS is compromised, and the panel has a name | As the row says. The CAA record is the owner's to add; the Network page writes it out. | Until that record is there, whoever has the VPS can get a certificate for the name. |
| Game server is exploited | Holds. Not root, no capabilities, no new privileges, read-only root, memory and process limits; kept from the machine it runs on and from the home network whether or not a VPS is connected (the lab asks both ways). *Changed:* kept as well from `100.64.0.0/10`, where Tailscale and carriers' own networks are, and from `169.254.0.0/16`. | Servers of one Homewarp reach each other on their bridge, which a proxy in front of several of them needs. There is no limit on what a server writes to the disk it is on. IPv6 is off on the bridge and nothing filters it if someone turns it on. |
| Malicious template / install script | Partly. The script runs in a container of its own with the server's folder and nothing else of the machine, kept from the home network as servers are, with memory and process limits. It is root there with most of what Docker gives a container, because install scripts install packages: "the same limits" was not true. *Changed:* it may no longer write raw packets, with which it could pass itself off as a neighbour on the servers' network, nor make device files. The template page shows the script and the images. | Whoever imports an egg runs its script, in that container. |
| Panel account takeover | Holds. Argon2id, with an unknown name costing the same work as a wrong password; sign-in limits by address and by account; a second step, and passkeys; a cookie that is `HttpOnly`, `SameSite=Strict` and over TLS `Secure`; the same-site check on every change and on the console's socket; the audit log. Adding a passkey, changing the password and turning the second step off each take the password. | There are no API keys, so none are hashed. The limits are kept in memory and a restart forgets them. A session lasts thirty days and is ended early only by signing out or by a change of password. |
| File-manager path traversal | Holds, and is tried 1,400 ways with every test run (§11, Phase 5). | |
| DDoS | Holds for floods of connections: the limit by address is a setting now. *Changed:* the panel's own port has limits too, 256 handshakes at once, 1,024 connections open and 64 from one address. | Packets from many addresses to a forwarded UDP port are passed home as fast as the VPS's line carries them: the limit is by address, and there is none for all of them together. |
| Tampered binaries / updates | Not built. There are no releases yet (Phase 6). | |
| Gate firewall change locks the user out of SSH | Built in Phase 5: the guard is put in place for a minute and undone by the Gate itself unless it is kept. | It is a list of what was listening when it was asked for. What begins to listen later is shut until the VPS is hardened again, and the panel says so. |
| Join token leaks | Holds. The lab checks that the key and the token the command carried open nothing afterwards. | |
| Home IP leaks through the game server | Not built. What a server sends out leaves by the home's own line, which the lab shows. A game that announces itself to a public list announces the home's address there. | Phase 7: servers' own traffic out through the Gate. |

## 7. Performance

- Forwarding is in-kernel on both ends; the Rust processes are control plane only and
  sit idle while players are connected.
- TCP stays end-to-end between player and game server — no double congestion control.
- The Gate has a hard resource budget; see §7.1.
- Added latency is the detour through the VPS; choose a VPS region close to home.
  The wizard measures and shows home ↔ Gate round-trip time.
- Measured, not assumed: Phase 0 records iperf3 TCP/UDP throughput and latency with
  and without the tunnel; those numbers gate the design.

### 7.1 VPS resource budget (owner's requirement, 2026-10-05)

The VPS must be the cheapest tier available: 1 vCPU, and no more than 2 GB of RAM.
The design aims well below that. These are budgets to be proven by measurement, not
measurements.

| Item | Budget |
|---|---|
| Smallest supported VPS | 1 vCPU, 512 MB RAM, 1 GB free disk, one IPv4 address |
| `homewarp-gate` memory | ≤ 20 MB resident, idle or busy |
| `homewarp-gate` CPU | About zero when idle; forwarding is kernel work and does not pass through it |
| Binary | ≤ 10 MB, static, no runtime dependencies |
| Disk | One small state file, rewritten only when forwards change; logs to journald with a size cap |
| Other software on the VPS | None — no Docker, database, web server or TLS stack |

How the design keeps to it:

- **Packets never enter the Gate process.** Kernel WireGuard and nftables forward them;
  the Gate only configures the kernel and answers status requests.
- **No TLS library in the Gate.** Its control API is plain HTTP inside the WireGuard
  tunnel, which already encrypts and authenticates.
- **Single-threaded runtime**, no worker pool, no background jobs beyond a slow status tick.
- **No database.** Desired state is one JSON file.
- **Connection tracking is the kernel's**, at a few hundred bytes per connection.
- **Userspace extras stay opt-in** (hostname routing, PROXY protocol, wake-on-connect).
  Each gets its own budget and uses zero-copy forwarding where the kernel offers it.
- **Outbound traffic from game containers does not go through the VPS** unless switched
  on, so modpack and update downloads do not eat its traffic allowance.
- The Tailscale link mode is heavier on the VPS — an extra daemon and userspace
  encryption — which is one more reason the built-in tunnel is the default.

Enforced by tests, so it cannot drift:

- The lab runs the Gate container limited to **1 CPU and 128 MB**. The suite fails if it
  is killed for memory or its resident size passes 20 MB.
- Gate CPU use is recorded while iperf3 pushes 10 and 100 Mbit/s through the tunnel.
- The ARM64 Gate build ships with the first Gate release, because the cheapest tiers
  are often ARM.

What actually decides which VPS to buy — the size of the CPU and RAM do not:

| Matters | Why |
|---|---|
| Region | Every packet detours through it. Close to home and to the players. |
| A vCPU that is really yours | Measured on the owner's VPS: 17 % of the core is taken by the host at rest and 53–65 % under load, and that caps the tunnel at about 40–60 Mbit/s (§10). Check `st` in `vmstat` on a trial before committing. |
| Traffic allowance | Budget about 100 MB per player per hour for Minecraft, and count it twice in case the provider bills both directions. Ten players for four hours a day is roughly 120 GB a month, 240 GB counted twice — inside the 1–3 TB that budget tiers include. |
| KVM virtualisation | Needed for kernel WireGuard. |
| A dedicated IPv4, UDP not filtered, game servers allowed | Check the provider's terms. |

For scale: one third-party benchmark puts kernel WireGuard at about 15 % of one core
at 500 Mbit/s on a $5 VPS. The owner's VPS did not come close — about 40–60 Mbit/s with
the core saturated, most of it stolen by the host — so treat that benchmark as a best
case. Ten Minecraft players peak under 10 Mbit/s, which leaves room even so.

Price check on 2026-10-05, from comparison sites (verify before buying): 1 vCPU / 1 GB
tiers at about $2 a month (IONOS) or about $22 a year (RackNerd, 3 TB of traffic).

## 8. Technology

| Area | Choice | Why |
|---|---|---|
| Language | Rust (stable), one Cargo workspace | Small static binaries, low memory, safe parsers for untrusted eggs and file paths. |
| Async / HTTP | `tokio`, `axum`, `tower-http` | De facto standard. |
| Database | SQLite via `sqlx` | Zero setup; one file to back up. |
| Docker | `bollard` | Native Docker Engine API client. |
| WireGuard | Kernel WireGuard driven over netlink (`defguard_wireguard_rs`, which also covers the userspace fallback) | No `wg-quick`, no extra packages. |
| Firewall | `nft` with a generated ruleset, atomic table replace (`nftables` crate or direct invocation with a fixed argv) | Debuggable, transactional. Netlink-native crates are too immature today. |
| Auth | `argon2`, `totp-rs`, later `webauthn-rs` | |
| TLS / ACME | `rustls`, `rustls-acme` | No OpenSSL; in-process certificates. |
| Files / archives | `cap-std`, `tar`, `zstd` | Sandboxed paths; fast backups. |
| SFTP (later) | `russh` + `russh-sftp` | Same accounts as the panel. |
| API schema | `utoipa` → OpenAPI → generated TS client | |
| Front end | React + Vite + TypeScript, Tailwind CSS, Radix primitives, TanStack Query/Router, xterm.js, CodeMirror 6, uPlot | Matches existing TypeScript experience; all static, embedded in the binary. Node is a build-stage dependency only. |
| Packaging | Core: Docker image **and** static binary + systemd. Gate: static musl binary (x86_64 + aarch64) + systemd. | The VPS needs no Docker. |

Confirmed by use in Phase 0 (2026-10-05, Rust 1.99): `bollard` 0.21 with only its
local-socket transport, `cap-std` 4, and `serde-saphyr` 1 for YAML eggs (the long-standing
`serde_yaml` is deprecated; eggs are untrusted input, so the parser should be a maintained
one). Phase 1 added `axum` 0.8, `sqlx` 0.9, `argon2` 0.6, `utoipa` 6 with `utoipa-axum`
0.3, and `rust-embed` 8. Still unused, so still to confirm: the WireGuard, nftables, ACME
and SFTP crates.

The front end is React 19.3, Vite 8, Tailwind 4.3, TanStack Router and Query, Radix and
ESLint 10. **TypeScript is held at 5.9.** TypeScript 7 is current, but `typescript-eslint`
(`<6.1.0`) and `openapi-typescript` (`^5`) do not accept it yet; lift the pin when both do.

## 9. Repository layout

```
homewarp/
├─ crates/
│  ├─ homewarp-core/        # binary: API, runtime, scheduler, tunnel client
│  ├─ homewarp-gate/        # binary: VPS endpoint, minimal dependencies
│  ├─ homewarp-proto/       # shared types: gate desired-state, status, join token
│  ├─ homewarp-net/         # WireGuard + nftables ruleset generation (both sides)
│  ├─ homewarp-template/    # egg import, validation rules, config-file parsers
│  └─ homewarp-runtime/     # Docker driver, lifecycle state machine, console, stats
├─ web/                 # React app → built into homewarp-core
├─ templates/           # bundled starter templates
├─ lab/                 # simulated VPS + internet (see §10); real-vps-spike.sh for a real one
├─ deploy/              # Dockerfiles, compose, systemd units, installers
├─ scripts/             # dev.sh (sync + check/test/lab/deploy over ssh)
├─ assets/              # brand files: heart.svg (the donation heart)
├─ README.md
├─ PLAN.md
└─ DESIGN.md
```

`homewarp-gate` depends only on `homewarp-proto` and `homewarp-net`, keeping the internet-facing
binary small and auditable.

## 10. Development and testing — nothing installed locally

The workstation only edits files. Every build, test and deploy runs on the homelab
over `ssh home`, inside containers.

### What the homelab looks like (probed 2026-10-05)

| Fact | Consequence |
|---|---|
| Ubuntu 24.04, kernel 6.8, x86_64 | Kernel WireGuard is present and already loaded. |
| `nft` 1.0.9, `ip_forward=1`, `rp_filter=2` | Transparent mode prerequisites already met. |
| Docker 29.2.1, Compose 5.1, iptables backend | Core runs as a Compose project like the other services. |
| No Rust, Node or Go on `PATH` (an `nvm` install exists under the home directory) | Toolchains live in builder containers only. |
| `lance` is in the `docker` group; **no passwordless sudo** | Core deploys as a container with `network_mode: host` + `NET_ADMIN`; no host packages, no systemd units, no sudo. |
| **ufw is active: what arrives for the machine itself is dropped unless allowed** (found 2026-10-06; it was not looked for on the 5th) | A port that Docker publishes is passed on, not received, and so is not shut: that is how every service here is reached. A program in the host's own network is behind ufw. Moving Core there shut the panel's port to the home network, which a check made from the homelab itself did not show. So Core listens on a socket in its data directory, and a second container of the same image, with the port published, passes connections to it: the *door* of `deploy/compose.yml`. Nothing on the host is changed for it. |
| **2 vCPU** | Cold release builds will take several minutes. Iterate with `cargo check` in a warm builder container; cap the builder below 2 CPUs so running services are not starved. |
| **Disk 89 % full, 28 GB free** (was 91 % / 24 GB before the Docker cleanup on 2026-10-05) | Hard budget: ~8 GB for build caches, pruned by script. Worlds and backups compete for the rest — free space or add a volume before running real servers. |
| Where the 205 GB goes: `media-server` 138 GB, Docker ≈ 28 GB, `file-browser` 17 GB, Suwayomi 5.5 GB, `.vscode-server` 4.3 GB | Docker has little left to give (≈ 1.9 GB of old Pelican images, 0.6 GB of unattached volumes). The media library is the lever. |
| A stopped Pelican Wings container and two stopped server containers exist, data under `/var/lib/pelican` | Real material for testing an "import from Pelican" path later. Not touched by this project. |
| 13 GiB RAM, ~8.5 GiB available | Enough for the build plus one or two Minecraft servers; not for several heavy SteamCMD games at once. |
| Ports 80/443/81 and 3100–3500 in use | Panel goes on **3600**, the next free port in the existing series. |

### What the VPS looks like (probed 2026-10-05)

The owner's VPS, reachable as `ssh server1` (root). It is a working machine with other
jobs, not a blank box.

| Fact | Consequence |
|---|---|
| Hong Kong, KVM, 1 vCPU, 2 GB RAM, 19 GB disk; about 47 ms from home | Inside the §7.1 ceiling. |
| Ubuntu 22.04, kernel 5.15, `nft` 1.0.2, iptables 1.8.7 (nf_tables) | Older userland than the homelab: rulesets must stay within what nft 1.0.2 accepts. |
| WireGuard module shipped but not loaded; no `wg`, no `iperf3` | The Gate must configure WireGuard itself and depend on no package. |
| ufw active: inbound default drop, routed default allow | See *Host firewalls* in §5.3. |
| nginx on 80/443 serving several sites, Docker with one container, Tailscale | The Gate must coexist. Port 443 is taken, so "public via Gate" (§5.9) cannot use it here. |
| Old Pelican forwards (DNAT + masquerade of TCP 25565–25579, 30000–30050, 40000–40050, 50000–50010 and matching UDP ranges, to a tailnet address that no longer exists) — **removed 2026-10-05**, with the ufw opening for 25565 and the dead `pelican` nginx site | 25565 is free. Copies of what was removed are in `/root/backup-2026-10-05/` on the VPS. The old panel's database is still there in two small Docker volumes: material for the import path of Phase 7. |
| Disk 34 % used, 12 GB free (it was 100 % and had stopped logging on 4 October) | An abandoned app that crash-looped 1.5 million times had written most of 12 GB of logs, and logrotate was missing. On 2026-10-05 the app was removed, the owner cleared the logs, and logrotate was installed with Ubuntu's stock settings. |
| SSH accepts root and passwords, no fail2ban, constant login attempts | Makes the "harden this VPS" toggle (§6) worth having. Owner's call. |
| CPU steal 17 % at rest | See the spike results below. |

### Tunnel spike on the real VPS (2026-10-05)

`lab/real-vps-spike.sh` — `up`, `test`, `bench`, `down`. Kernel WireGuard between a
throwaway container at home and the VPS, two test ports forwarded with DNAT and no
masquerade. Inside the container, its own namespace played the home host and a nested
namespace played a game container behind a published port. Everything on the VPS was
runtime-only and was removed afterwards; the homelab's own network namespace was not
touched.

| Check | Result |
|---|---|
| Kernel WireGuard on the VPS | Works. |
| Handshake | First attempt. Home dialled out; nothing was opened at home. |
| Client address at the game side | **Preserved, TCP and UDP**: the game side saw the client's public address, not the Gate's tunnel address. |
| Gate ruleset of §5.3 | Loads as written on nft 1.0.2, including the `port : address . port` maps. |
| Home ruleset of §5.3 | One bug, found and fixed (see §5.3). With the fix, replies return through the tunnel. |
| VPS reaching home | Could not ping the home tunnel address. Acting as a taken-over VPS — its own allowed-ips widened, a LAN address routed into the tunnel — it still could not reach the homelab's SSH port; the home forward rule counted the drops. |
| Game side reaching out | Reached the internet directly, not through the VPS. Could not reach a private address. |
| Latency | 48.5 ms direct, 47.0 ms through the tunnel (10 pings each): no measurable cost. 50 pings each way, none lost. |
| TCP, home → VPS | Direct 15–78 Mbit/s between runs; tunnel 37–50. |
| TCP, VPS → home | Direct 218–241 Mbit/s; tunnel 53–63, with the VPS core saturated: 53–65 % steal, 16–27 % idle. |
| UDP at 10 Mbit/s, home → VPS | 6.8 % lost direct, 6.3 % in the tunnel — the line, not the tunnel. None lost at 2 Mbit/s either way. |
| UDP at 10 Mbit/s, VPS → home | None lost direct, 0.4 % in the tunnel. |

What this does and does not show:

- The tunnel adds no latency and no loss of its own, and it keeps client addresses. The
  central risk of §12 is retired for the Gate side and for the routing at home.
- Throughput through this VPS is limited by its CPU, not by the design. It is several
  times what a handful of Minecraft servers need, and far below the line's capacity.
- The home → VPS direction was noisy and lossy with or without the tunnel. The test ran
  at about 22:00 local time; repeat it off-peak before drawing conclusions about the line.
- **Not shown:** Docker's own published-port rules on a real host (a hand-made namespace
  stood in for them), a client on a third network (the client was at home, as in the
  self-probe), the per-source rate limit under load, recovery after a reboot or an
  address change, and the Tailscale link mode. The first of these is the lab's job.

### Dev loop

`scripts/dev.sh <cmd>` tars the working tree to `/home/lance/homewarp` and runs:

| Command | Runs on the homelab |
|---|---|
| `check` | `cargo check` + `clippy` + `cargo fmt --check` in the builder container (named volumes for registry and `target`) |
| `test` | unit tests (`cargo nextest`) + `tsc --noEmit` + ESLint in a Node container |
| `gen` | rewrite `web/openapi.json` from the handlers; a test fails while that copy is stale |
| `npm …` | npm in `web/`, in a Node container; `package.json` and its lock come back |
| `lab` | full end-to-end suite in the simulated network below |
| `paper` | the Phase 0 runtime spike: the Paper egg end to end on the homelab's Docker |
| `build` | build the web interface, then Core with it inside, then the image `homewarp:dev` |
| `deploy` | `build`, then `docker compose up -d`; check `:3600` answers. Servers it runs stay running |
| `scratch` | a throwaway copy of the last build on `:3601`, with data of its own, the same Docker daemon and the same network as the deployment, for trying what needs an account without touching staging's. `scratch down` removes it, its servers' containers, its data and its end of a tunnel |
| `gate` | the Gate as a VPS runs it, one static binary each for x86_64 and ARM64, and Core built the same way for the lab. The ARM64 one is run once under an emulator, to see that it runs at all |
| `vps` | `gate`, then the x86_64 binary copied to the VPS (`ssh server1`) by way of the workstation. `vps leave` runs `homewarp-gate leave` there |

The tree goes to `/home/lance/homewarp/src`, replaced whole on every sync; `data/` beside
it belongs to the server. The builder runs as the homelab user, capped at 1.5 CPUs, and
`Cargo.lock` is copied back so it is committed from the workstation.

Measured on 2026-10-05 (Rust 1.99.0, 2 vCPU):

| What | Result |
|---|---|
| Edit to verdict, warm: sync + `check` + `clippy` + `fmt --check` | 2.7 s |
| `cargo check`, cold, Core with axum alone | 19 s |
| `cargo check`, cold, adding the template and runtime crates (bollard, cap-std, serde-saphyr) | 32 s |
| Unit tests, first build | 38 s |
| `cargo check`, cold, Core's whole Phase 1 stack (sqlx with bundled SQLite, argon2, utoipa) | 42 s |
| Release build of Core, cold / after a one-file change | 2 min 33 s / 18 s |
| Web build (2141 modules) | under 1 s |
| `deploy`, cold, start to answering on `:3600` | 2 min 46 s |
| Core as deployed | 6.7 MB binary, 1.4 MB resident when idle |

Cold builds are well below the "several minutes" feared for 2 vCPUs.

Disk after Phase 1, against the ~8 GB budget: caches 3.9 GB (`target` 2.7 GB, crate
registry 0.5 GB, `node_modules` and npm's cache 0.7 GB) and images 3.5 GB (builder 1.34 GB,
`yolks:java_25` 0.78 GB, the two lab images 0.5 GB each, Node 0.24 GB, Core 0.12 GB). That
is 7.4 GB: the budget is spent, and the homelab is at 91 % with 22 GB free. `dev.sh prune`
drops `target`; the open question about disk at home (§13) is now pressing.

### The lab: a VPS without a VPS

A Compose project on the homelab simulating all three parties in isolated network
namespaces, so the host's real networking is never touched:

```
 client (203.0.113.50) ── "internet" bridge ── gate (203.0.113.10)
                                                   ║ WireGuard
                                               home (docker-in-docker, runs homewarp core)
                                                   └─ game containers
```

Assertions:

- A Minecraft status ping and a UDP echo from `client` reach a server inside `home`.
- The server sees source `203.0.113.50` (transparent mode proven for TCP and UDP).
- iperf3 throughput and latency overhead are recorded.
- Killing the tunnel, restarting the Gate, and changing the home's address all recover.
- Nothing but the configured ports is reachable from `client` or from `gate`.

#### Lab results (2026-10-05)

`lab/run.sh`, started with `scripts/dev.sh lab`. As built it differs from the drawing in
two ways: both networks are internal, so the lab can reach neither the homelab's LAN nor
the internet, and home also sits on a simulated LAN (`192.168.50.0/24`) with a NAS that
nothing from outside may touch. The Gate did not exist yet, so `wg` and `nft` were driven
by the script; since Phase 3 the simulated VPS runs the Gate program itself (§11). Home ran Docker 29.8.2; every check passed on **both** of Docker's firewall
backends, iptables and nftables.

| Check | Result |
|---|---|
| Client address through a real Docker published port | **Preserved, TCP and UDP.** The game container saw `203.0.113.50`. |
| Control: the reply mark removed | No answer. The policy-routed return path is what carries it. |
| NAT mode: one masquerade rule on the Gate | Works; the server sees the Gate's tunnel address. |
| Strict reverse-path filtering at home (`all.rp_filter=1`) | Works with `homewarp0` alone set loose. |
| A taken-over Gate, its allowed-ips widened and home's networks routed into the tunnel | Cannot reach a service on home's tunnel address or LAN address, the NAS, an unpublished port of the game container, or the published port by the container's own address. |
| The game container, hardened as in §5.6 | Reaches the internet from home's own address, not through the Gate. Cannot reach the NAS, nor a service on the home host by its bridge or LAN address. |
| Throughput to the game container | About 1.05–1.16 Gbit/s through the tunnel against 21–23 Gbit/s straight to the published port. One 2-vCPU machine plays every party, so this shows only that nothing in the path is slow in itself. |
| UDP at 10 Mbit/s through the tunnel | None lost. |

- The remaining network risk of §12, Docker's own rules fighting the return path, is
  retired for Docker 29 on both backends.
- The Gate container's limits (1 CPU, 128 MB) are in place but prove nothing yet: with no
  Gate process there is nothing in it to measure (0.6 MB), and the kernel's forwarding
  work is not charged to it.
- **Not shown:** home behind NAT (the real VPS showed that), recovery after a restart or
  an address change (that needs the Gate and Core), the rate limit under load, and Core
  doing all this from inside a container on a real host rather than a script in a
  privileged one.

#### The lab since Phase 3 (2026-10-06)

Both programs in it are the real ones: Core in `home`, and the Gate on the simulated
VPS, enrolled with a join token as a VPS is. Nothing is typed into either kernel by the
script any more, except to break things.

- **Home is behind a router now**, a container that hides the home LAN behind one
  public address as a home's router does. Without it the self-probe cannot be tried: a
  connection home makes to the VPS has to come back from an address that is not home's
  own. It also gives the lab a home address to change.
- **The home LAN is an internal macvlan network, not a bridge.** On an internal bridge
  Docker drops every packet whose address is outside the bridge's subnet, on the
  homelab's own firewall, and what home sends to the internet by way of the router is
  just that. The first run with the router passed nothing, for that reason and no
  other. The macvlan network is a wire between its containers, with no firewall of the
  homelab's on it.
- **What it checks**, on both of Docker's firewall backends: enrolment and the change
  of keys; the self-probe and both of its verdicts that can be made to happen there;
  each of a server's ports for the protocols it was given; the containment checks of
  Phase 0, with the panel added to what a taken-over Gate must not reach; the §7.1
  budget; and that the tunnel comes back by itself when the Gate's table is emptied,
  when the VPS loses its end as in a reboot, when home loses its end while Core runs,
  when Core is restarted with home's end gone, and when the home's address changes.
  It ends by disconnecting the VPS and seeing that home's kernel is as it was.

### Runtime spike: the Paper egg (2026-10-05)

`scripts/dev.sh paper` runs `crates/homewarp-runtime/examples/paper.rs` on the homelab's
own Docker (29.2.1): the current Paper egg from the Pelican repository (`PLCN_v3`, YAML),
unmodified.

| Step | Result |
|---|---|
| Import | 6 variables, 6 images; the first, `yolks:java_25`, is the default. |
| Install container (`installers:alpine`, script at `/mnt/install`, files at `/mnt/server`) | 4.4 s; Paper 26.3 build 152. |
| Hand the files to the server's user (`chown` in a container that sees only that directory) | 0.3 s; everything owned by `4857:4857`. |
| Patch `server.properties` (`properties` parser) | `server-port`, `query.port`, `server-ip` set. |
| Start in the hardened container: not root, no capabilities, no new privileges, read-only root, memory and process limits | 15.5 s to `Done`, 10.8 s by the server's own count. |
| Status ping, as a Minecraft client sends it, on the published port | Answered: Paper 26.3, 0/20 players. |
| Console | `list` answered; `stop` ended it with exit code 0 in 1.1 s. |
| Images pulled | `yolks:java_25` 781 MB (15.5 s), `installers:alpine` 38 MB (5.6 s). |

Found on the way:

- The egg has a variable, `USER_AGENT`, that is required and has no default: Paper's
  download service wants to know who is calling. The create-server wizard has to fill it.
- An empty list arrives as `{}` (`rules: {  }`): the panel that exports eggs is PHP. The
  importer accepts it, along with the Pterodactyl spellings (rules joined with `|`,
  `config` values as strings of JSON).
- Docker's `local` log driver refuses a single file with compression, which is its default.
- The memory limit is what was asked for plus Wings' headroom (15 % up to 2 GB, 10 % up to
  4 GB, 5 % above), because eggs size the JVM heap from `SERVER_MEMORY`.
- The spike agreed to Mojang's EULA with a flag. In the product that is the user's click
  (the `eula` feature, §5.6).
- **Not shown:** crash detection and restart, statistics, the other config-file parsers,
  variable validation, a SteamCMD game. Those are Phase 2.

### Egg corpus (2026-10-06)

The importer was run, through Core's API, over 116 eggs as their authors publish them:
an even sample of `pelican-eggs/minecraft` (39), `pelican-eggs/games-steamcmd` (39) and
`pelican-eggs/generic` (24), and all 14 that ship with Pterodactyl's panel. Both
families of format are in there, as YAML and as JSON.

| | Result |
|---|---|
| Read | 115 of 116 on the first run, and all 116 since replacing by pattern was written. They make 101 templates: 15 are a second export of an egg already read, and are turned away by name. |
| Refused | On the first run 1: BungeeCord, whose `config.yml` replaces by pattern (`servers.*.address`). The importer said so rather than guess. |
| Config files | 50 eggs patch none, 38 only `properties` files, and 28 need another parser: `file` 9, `ini` 7, `json` 6, `yaml` 4, `xml` 1. |
| Stop | 54 by a console command, 46 by an interrupt. Nine of the 116 spell the interrupt `^^C`. Wings does not know that spelling and sends SIGKILL (`environment/docker/power.go`); Homewarp reads it as the interrupt that was meant. Until this run it kept a signal named `^C`, which would have failed at the first stop. |
| Install | All 100 have an install script and a "started" string. |
| Images | 64 offer one image; 32 offer four or more (Java versions). |
| Features | `steam_disk_space` 38, `eula` 32, `java_version` 32, `pid_limit` 32, `gsl_token` 3. |

Validation rules, by how many of the 100 templates use each: `required` 97, `string`
95, `max` 65, `nullable` 63, `boolean` 48, `in` 37, `between` 24, `regex` 24, `numeric`
22, `integer` 19, `alpha_dash` 10, `min` 8, `digits_between` 6, `alpha_num` 6, `size` 5,
`gt` 2, `url` 2, `ends_with` 1, `not_in` 1, and one `int` where `integer` was meant. The
eleven first listed in §5.6 are not enough: the validator needs the other eight, and
has to name a rule it does not know when the egg is imported, not when a server starts.
Two eggs carry an empty entry in a list of rules, which the importer kept as a rule;
it drops them now.

This was one run with a throwaway script, against a throwaway copy of Core. The
standing corpus of §12 is still to build.

### Real-world tests

- Staging Core on the homelab itself (Compose project at `/home/lance/homewarp`, port 3600).
- Browser end-to-end tests driven against that deployment, never a local server.
- The owner's VPS (`ssh server1`) for tunnel tests against a real machine, with
  `lab/real-vps-spike.sh` until the Gate exists.

## 11. Phases

Each phase ends with something that works on the homelab.

**Phase 0 — Spikes (de-risk before committing to the design)** — *done 2026-10-05; results in §10*
- Dev loop: builder container, sync script, measured `cargo check` time, disk budget.
- Tunnel: lab with kernel WireGuard, DNAT, transparent return path through a Docker
  published port; iperf3 numbers, with the simulated VPS limited to 1 CPU and 128 MB.
  *Done on the real VPS and in the lab.*
- Runtime: run the Paper egg end-to-end (install container → yolk image → console) with `bollard`.
- *Exit:* client IP preserved for TCP and UDP in the lab; Paper starts; numbers written down.
  If the transparent return path proves fragile, this is where the design changes.
  *Met. The design stands; §5.3 lists the five things the lab added.*
- *Left for Phase 3, with the owner's go-ahead:* Core setting up the tunnel from inside a
  container in the homelab's own network namespace, where the other services live.

**Phase 1 — Core skeleton** — *done 2026-10-06*
- Workspace, config, SQLite + migrations, first-run setup code, login, sessions.
- OpenAPI + generated client; embedded UI shell (login, empty Servers page), with the
  donation heart in its corner from the first build.
- *Exit:* `http://192.168.1.250:3600` shows the panel; admin account can be created.
  *Met.* Driven in a browser against the deployment: a wrong setup code is refused, the
  right one creates the account, sign-out and sign-in work, a signed-in visit to `/setup`
  goes to the panel, and the shell was looked at 1250, 900 and 390 px wide in the dark
  theme and at 1250 px in the light one. A page load makes one API request. The test
  account was then deleted: the first account is the owner's to create, with the code in
  `docker logs homewarp`.
- What it is: one 6.7 MB binary and one SQLite file. Argon2id passwords; the browser
  holds a random token in an `HttpOnly`, `SameSite=Strict` cookie and the database holds
  only its hash; the first account must quote a code that Core writes to its log. The
  page allows scripts, styles and fonts from its own origin and nowhere else.
- Found by that policy: the bundler had inlined one small font as a `data:` URL, which
  the policy blocked. Inlining is now off.
- **Not in it, by plan (Phase 5):** login rate limiting, TOTP, TLS and the `Secure`
  cookie flag. Fine on the home network; all four are needed before the panel faces the
  internet (§13).
- The donation popover has no links yet, and says so. The four destinations besides
  Servers are shown dimmed and lead nowhere.

**Phase 2 — Servers** — *done 2026-10-06, but for Paper past its EULA, which is the owner's to agree to*
- Template import (all egg formats), validation rules, config-file parsers.
- Install flow, lifecycle state machine, console WebSocket, stats, limits, crash recovery.
- UI: template gallery, create-server wizard, server page with console.
- *Exit:* Paper, one SteamCMD game and one non-game template install and run on the LAN.
- *Done so far: templates.* Core keeps them (§5.7) behind four endpoints: list, import,
  read one, remove. The panel has a Templates page: a gallery, an import page that
  takes an egg as a file or pasted, and a page for each template that lays out all the
  egg tells Homewarp to do (images, startup command, variables with their rules, the
  install script), with removal behind a question.
- Driven in a browser against a throwaway copy of the staging build, so that staging's
  own database was left without an account: the published Paper egg and Pterodactyl's
  Rust egg imported from files, BungeeCord and a second Paper refused in words, a
  template removed, and a session ended behind the page's back, which leads to sign-in.
  Looked at 390, 900 and 1250 px wide in the dark theme and at 1250 px in the light one.
- Measured: a page opened cold makes two requests, the session and its own read, and
  they leave in the same millisecond. Reached from inside the panel it makes one. The
  page an import lands on makes none.
- Found by the page policy, again: the component library's modal layers lock scrolling
  by writing a `<style>` element into the page, which the policy refuses. The account
  menu had been doing so since Phase 1, unnoticed. The menu is no longer modal and the
  one dialog is the browser's own `<dialog>`. The policy is as it was.
- Found by real eggs: *Egg corpus* in §10.
- *Done so far: a server's first life.* A server is made from a template in the panel:
  a name, memory, a port, the template's variables, a processor limit, and for a game
  that has one the box that agrees to its EULA. The variables are checked against their
  rules, all nineteen that the corpus uses (§10); an egg with a rule or a pattern Core
  cannot follow is refused when it is imported, and none of the 116 is. Core installs
  the server, starts it and watches its console. It can be stopped, killed when it will
  not stop, started again, and removed with its files. Each server has a task of its
  own in Core, which keeps the last 500 lines of its console for a page that opens
  later.
- Core holds the Docker socket now (§13). It runs as root inside its container, with a
  read-only root and two capabilities, CHOWN and DAC_OVERRIDE: what it takes to keep
  files that belong to the servers' user. It sees its data directory at the path the
  host has it at, because Docker reads the paths it is handed on the host.
- Tried on the homelab against the real daemon, through the API and then in a browser,
  in a throwaway copy (`dev.sh scratch`). A test egg installed and was running in five
  seconds: as uid 4857, read-only root, no capabilities, its memory and processor
  limits set, its port published for TCP and UDP. It was stopped by its console
  command, started, left running while Core was restarted and found running by the new
  Core, killed, and removed, which left no container and no file. Refused in words:
  removing a server that runs, removing a template a server is made from, a name or a
  port another server has, starting what is running.
- Then the published Paper egg, through the wizard: installed (Paper 26.3), started,
  `server.properties` patched, and stopped by Paper itself, asking for its EULA. That
  was not agreed to on the owner's behalf. The page showed Offline, with Paper's own
  words in the console.
- Nothing else on the homelab was touched: every other container it runs was still
  running afterwards. Looked at 390 and 1250 px wide, in the dark theme.
- *Done so far: the socket of §5.8.* A server's page opens one WebSocket and is sent,
  as it happens, each line of the console, each change of state, and about once a
  second what the server uses of the processor and of its memory. The first paint
  still comes from one request; the socket takes over once it has said where things
  stand, and a page that loses it goes back to asking until it has it again. A line
  typed under the console goes to the server, with the up and down arrows for what was
  typed before. A socket is opened with a GET, which the check on requests from other
  sites lets pass, so this endpoint makes that check itself, as well as asking for a
  session.
- Tried in a browser against a throwaway copy: with the socket open a page made no
  request after its first two; a line typed in came back answered by the server; `stop`
  typed in took it to Offline and locked the line; Core was restarted under the open
  page, which came back by itself to a server that had never stopped, and typed into
  it again. The page policy lets a page open a socket to its own origin, in Chromium
  at least: `'self'` is all it says, and Safari before 15.4 did not read that as
  covering WebSockets.
- *Done: the config-file parsers.* `file`, `ini`, `json` and `yaml` join `properties`,
  written to do what Wings does, whose `parser/parser.go` was read for it, with three
  differences made on purpose. An INI file keeps its comments and its layout, where
  Wings writes it afresh. A JSON file keeps the order of its keys, where Wings sorts
  them. And a setting that is made only where a certain value is there already, plain
  or by pattern, is made when it is there: Wings has both of those tests the wrong way
  round, so in Wings they never pass. With replacing by pattern, BungeeCord's egg
  imports, and all 116 are read. A file that cannot be set up is said in the console
  and passed over, as is a setting with a placeholder Core has nothing for. A config
  file may sit in a directory the game has yet to make; it is made, and given to the
  server's user. Only XML is missing: one egg of the 116, Space Engineers, which is
  refused when a server is made from it and not after it has installed.
- *Done: crash recovery.* A server that ends by itself with an error is started again
  after 5 seconds, then 15, then 45, and after a fourth crash in a row is left alone,
  with why in its console. One that had stayed up a minute starts the count afresh.
  Tried: a server made to exit with code 3 was back in 6 seconds, and after a second
  crash waited its 15.
- *Done: changing a server.* Its name, image, memory, processor limit, port, variables
  and EULA box, on a Settings page that is the wizard's form again. It has to be
  stopped, and is as it was changed from its next start; its files are not touched.
  Tried against the daemon: refused while running, held to the template's rules, and
  after the change started with the new greeting, 384 MB in place of 256, and its port
  published at the new number.
- *Exit, run on the homelab from published eggs, each in a throwaway copy.* **Gitea**,
  which is not a game: installed in 41 seconds, its page answered over the LAN, its
  `app.ini` set by the `file` parser, stopped by a `^^C` read as an interrupt, removed.
  **Barotrauma**, through SteamCMD: 549 MB installed and running in 116 seconds,
  listening on UDP 27015 on the home machine, stopped, removed. **Paper**: installed
  and started through the wizard as far as its EULA (above); the same engine ran it to
  "Done" and answered a status ping in Phase 0. Agreeing to the EULA is the owner's to
  do, with the box in the wizard.
- **Moved out of this phase**, each to where it belongs. Importing from a URL, and a
  catalogue to pick from, to Phase 6 with the catalogue browser: both need Core to
  fetch from the internet, which it does not do and should not start doing without the
  care that takes. Protocol per port to Phase 3, with allocations. A server's disk use
  to Phase 4, with its files. Players to Phase 7. The XML parser to whenever an egg
  that matters needs it. Small and still open: how long a server has been up; usage on
  the Servers page, which asks every four seconds; free memory and disk shown where a
  server is made.

**Phase 3 — Gate and tunnel** — *done 2026-10-06, but for a game's own log of a real player, which waits on the Minecraft EULA, and a reboot of either real machine, which is the owner's to do*
- `homewarp-gate`, enrollment with key rotation, declarative forwards, self-probe,
  transparent + NAT modes, allocations with a protocol for each port (§5.6), Network
  page with live health.
- x86_64 and ARM64 Gate binaries; the §7.1 resource budget asserted in the lab.
- *Exit:* lab suite green; a player joins through a real VPS IP and the server logs
  their real address; VPS reboot, home reboot and home IP change all self-heal.
- *Done so far: the Gate program, in the lab (2026-10-06).* `homewarp-gate` is one static
  binary of 3.1 MB that asks nothing of a VPS but `nft`. From two files in its
  directory it makes the WireGuard interface, over netlink, and its one nftables table,
  and then answers Core on its tunnel address only, to whoever shows its token.
  `PUT /v1/state` takes all that it is to forward and in which mode; `GET /v1/status`
  says what it is doing and when home was last heard from. What it is told it keeps, so
  that it comes back from a reboot doing the same. The types the two ends share are in
  `homewarp-proto`; the rules and the kernel work are in `homewarp-net`.
- The lab's simulated VPS runs it now, in place of commands typed by the script, and
  home tells it what to forward through the tunnel. 28 checks pass on both of Docker's
  firewall backends: those of Phase 0 as they were, NAT mode asked for through the API
  and taken back, and for the program itself that it answers nobody without its token
  and nobody on the public address, that it holds 2 MB of memory against the 20 MB of
  §7.1, that players still get through while it is stopped and when it is started
  again over the tunnel it left, and that with the tunnel wiped from the kernel, as a
  reboot of the VPS wipes it, it is back to what it was last told.
- Found: set up a second time, a WireGuard interface forgets its session and where the
  other end is. Restarting the Gate cut players off until home shook hands again. It
  now leaves alone an interface that is already as wanted.
- Found: after a reboot of the VPS, home notices only when something it sent goes
  unanswered, which takes about 15 seconds, or at its next re-keying, which can take
  two minutes. A keepalive alone does not do it. Core's regular asking after the Gate
  is what makes recovery quick, and has to exist for that reason as well.
- Found: the lab's own stand-in servers lost about one answer in a hundred (3 of 267),
  by a reset overtaking it. Twice it looked like a fault of the tunnel. The lab's
  clients now only listen; after that, 3024 connections through the Gate in four
  minutes, across two re-keyings, and none unanswered.
- Found: `defguard_wireguard_rs` always links its userspace fallback, and with it
  `ring`, part of which is C. The binary is small all the same, but the Gate is 151
  crates, which is not the short list §9 had in mind. To weigh before a first release:
  driving netlink directly, with the three small crates that library is built on.
- *Done in the lab (2026-10-06): Core's end, and the two ends together.* The lab's
  `home` runs Core itself now, built static as the Gate is. Through its own API it is
  given an account, two lab eggs and two servers, and then told where its Gate is. It
  makes its end of the tunnel, tells the Gate to forward its servers' ports, and asks
  after it every ten seconds. All 28 checks pass on both of Docker's firewall
  backends with nothing typed into either kernel by the script: NAT mode is asked of
  Core and reaches the Gate within Core's ten seconds, and after the tunnel is wiped
  on the simulated VPS it is Core's asking that brings it back. The WireGuard library
  adds no route for home's allowed addresses, which was the thing to watch. (The lab
  has changed since, and so has the count: see below and §10.)
- *How the rest of the phase is to be done* (decided 2026-10-06, the owner having said
  "dont stop until finished with phase 3" and to commit as it goes):
  - *Getting the Gate onto a VPS.* There are no releases to download until Phase 6, so
    the one-line installer waits for them. Until then the binary is copied over
    (`scripts/dev.sh` does it for the owner's VPS) and installs itself:
    `homewarp-gate join <token>` writes its two files and a systemd unit, opens what a
    host firewall such as ufw needs, and starts. The wizard shows that command.
  - *Enrolment.* Core makes the token: a bootstrap key for the Gate, home's public
    key, a preshared key, the Gate's API token and the ports. On first contact Core
    asks the Gate to make a key of its own (`POST /v1/rotate`), and both switch to it.
  - *Self-probe.* The Gate opens a port for a moment (`POST /v1/probe`); Core connects
    to the VPS's public address from home and sees whether it comes back through the
    tunnel with home's own address. If not, NAT mode, said plainly on the Network page.
  - *Real machines.* `server1` and the homelab are working machines. Neither is
    rebooted for a test: the reboot checks are the lab's, and on the real pair the
    Gate's service and Core's container are restarted and the tunnel's interface
    removed, which is what a reboot does to them.
- *Done in the lab (2026-10-06): the rest of what the phase lists.*
  - *Enrolment.* Core makes the command, the VPS runs it, and within a few seconds the
    Gate has keys of its own (§5.5). The lab checks that the Gate's key is not the one
    the command carried, that home knows the Gate by the new one, that the preshared key
    and the Gate's token are new as well, that the token the command carried opens
    nothing, and that the command is gone from Core once it has been used.
  - *The self-probe* and the choice of mode (§5.3). The lab makes both verdicts happen:
    with the way back open, *preserved*; with a rule put ahead of Homewarp's that sends
    marked replies out by the home's own line, Core finds that nothing gets back, has
    the Gate stand in for players, probes again, and says *hidden*. Taken away, it finds
    *preserved* again.
  - *Ports.* A server's first port and its further ones, each for TCP, UDP or both
    (§5.6), from the form through Docker's published ports to the Gate's two maps.
  - *Both ends heal themselves* (§5.4). In the lab: the Gate's table emptied by something
    else; the VPS's end wiped as a reboot wipes it; home's end wiped while Core runs;
    Core restarted with home's end gone; and the home's address changed, to which the
    Gate's status then answers with the new one.
  - *The Network page and the wizard* (DESIGN.md), and the Gate's mark in the sidebar.
    Address chips say the VPS's address once one is connected. The mark and the chips
    are wanted and not needed: nothing but the two Network pages waits for the Gate, and
    every page shares the one request. Measured on a first load of the Servers page: its
    two reads leave together at 21 ms, the page is painted at 40 ms, and the Gate was
    asked for at 32 ms, once the sidebar was there to want it. On the Network page the
    Gate and the session leave together.
  - *ARM64.* `dev.sh gate` builds both, 3.4 MB for x86_64 and 3.1 MB for ARM64, and runs
    the ARM64 one once under an emulator. That it forwards on an ARM machine is not
    shown: there is none here.
  - *The budget of §7.1*, asserted: 3 MB resident against 20; one file under 10 MB for
    each architecture; never killed for memory in a container held to 1 CPU and 128 MB.
    Recorded, not asserted: while iperf3 pushed about 950 Mbit/s and then 100 Mbit/s
    through the tunnel, the Gate program used no processor time that its clock could
    count. Packets do not pass through it.
- Found: **a key kept as it was made never looks like the one the kernel has.** The
  kernel sets three bits of a private key its own way. Each end compares the interface
  with what it wants before touching it, found the key different every time, and set the
  interface up again: at home every ten seconds, on the VPS every thirty. Each time the
  session was lost, and on the VPS the knowledge of where home is. The first full run
  showed it as six failures that came and went. Keys are now made the kernel's way, and
  compared that way whoever made them. Keys from `wg genkey`, which the lab used until
  enrolment existed, are made that way already, which is why nothing showed it before.
- Found: setting WireGuard up again takes the interface's address away for a moment,
  and the kernel drops every route through an interface that has no address. So home's
  route for replies is put back every round, not only when the tunnel is first made.
- Found: a server asked the home's router for names, and the tunnel's rules keep a
  server from the home network, the router with it (§5.6).
- Found: the lab's own network was in the way (§10), and a request whose sender hangs
  up at once is dropped by the panel unanswered, which looked like a rule blocking it.
- Found: the Gate program undoes what the lab does to it. The lab plays a taken-over
  VPS by widening the Gate's WireGuard settings by hand; when the Gate's look at the
  kernel fell inside those seconds it put the interface back as it should be, which is
  right, and costs the session, which failed the four checks that came next. Whoever has
  taken a VPS over is not running the Gate, so the lab stops it for that part.
- *Done on the real machines (2026-10-06):* the homelab and the owner's VPS, a working
  machine each, 45 ms apart.
  - *Core's deployment* is in the homelab's own network namespace with `NET_ADMIN`, behind
    its door (§10). The thirty other containers there were running before and after, and
    with no VPS connected Core makes nothing in the machine's network at all.
  - *The trial* was made from the scratch copy, so that staging's database stayed empty
    for the owner. `dev.sh vps` put the Gate on the VPS; the panel made the command; run
    there, it opened ufw, installed the service and started it, and the tunnel was up
    within the seconds it takes to look. The self-probe found *preserved*.
  - *A player joined through the VPS's public address*, from the workstation, and the
    server wrote down the workstation's own public address, not the Gate's. The server
    was a stand-in that says who joined: a Minecraft server needs its EULA agreed to,
    which is the owner's to do, so a game's own log of a real player is still to come.
  - With the tunnel's rules in place on the homelab, a server still looked names up, and
    could not reach the home's router. From the VPS, neither the homelab's SSH nor the
    panel could be reached through the tunnel. The Gate held 3 MB on the VPS.
  - *It came back by itself*: the Gate's service restarted, and no connection was
    refused; the tunnel wiped from the VPS's kernel as a reboot wipes it, back in 29 s;
    home's end wiped under a running Core, back in 7 s; Core restarted with its end gone,
    back at once. Neither machine was rebooted, and the home's address was not changed:
    those two are the lab's.
  - *Disconnected from the page*, home's kernel was as before, and the Gate had been told
    to forward nothing. `homewarp-gate leave` then took the Gate off the VPS, which is as
    it was found: on the last run its ufw went from ten rules to sixteen and back to ten.
    The homelab has one container more than it had, the door.
- Found on the real machines, each of them invisible in the lab:
  - the homelab's ufw shut the panel's port once Core was in the host's network, and a
    check made from the homelab itself said all was well (§10: the door);
  - the same ufw dropped the self-probe's first design, which listened on the home
    machine itself (§5.3);
  - `leave` left the VPS two of its three openings in ufw: the one for routed traffic is
    taken away with `ufw route delete`, not `ufw delete route`. They were removed by hand
    and the program put right.
- Left for the owner: an account on staging (its setup code is in `docker logs homewarp`),
  then `bash scripts/dev.sh vps` and Network → Connect a VPS to connect the VPS for good.
- Moved: traffic for each forwarded port, and with it the traffic column of the
  Network page, to Phase 4. Rate limits as a setting to Phase 5, where the other limits
  are. firewalld set up by `join` itself, and not only described, to Phase 6. Checking
  the tunnel's addresses against the home machine's own networks before the first
  connection, to Phase 6 as well.

**Phase 4 — Day-two features** — *done 2026-10-06, but for traffic counted on the real VPS, which has no Gate on it now, and an SFTP program with windows, which is the owner's to try*
- File manager (browse, edit, upload, archive/extract), backups (tar.zst, restore,
  retention), schedules, audit log, sub-users and permissions, SFTP.
- From Phase 3: traffic for each forwarded port, counted by the Gate and shown on the
  Network page; which resolvers servers ask, as a setting.
- *A server's page is now a frame with tabs* (DESIGN.md, Inside a server): Console,
  Files, Backups, Schedules, Users, Settings. The frame holds the name, the state, the
  address and the power controls, and the socket: a console left for another tab is as
  it was on coming back. An account sees the tabs it may use and no others.
- *Files.* Every path a request names is opened through `cap-std` (§6), in
  `homewarp-runtime`: listing, reading text up to 4 MB, writing, making folders, moving,
  deleting, packing into a `tar.gz` and unpacking a zip, a tar, a `tar.gz` or a
  `tar.zst`. A path that says `..` is refused as written; a link that leads out is
  refused by the kernel's own resolution, whatever it is named.
  - A file that is written, by an upload or by the editor, is written beside its place
    and moved into it once it is whole, and keeps the mode of the file it replaces: a
    start script that is edited can still be run.
  - A download is answered as `application/octet-stream`, as an attachment, with
    `nosniff`: a page that a plugin wrote among a server's files never opens as a page
    of the panel's.
  - An archive is somebody else's word for where files go. An entry named `../x` or
    `/x` is passed over; one that would be written through a link is passed over; a
    file takes the place of what is there by its name and is not written into it. What
    is uploaded, packed or unpacked stops short of the last gigabyte of the disk, which
    is also what stops an archive that unpacks to more than it should.
  - The page: a table of the folder with a menu on each row, upload by a button or by
    dropping files on it, with how far each has got; an editor that is a plain text
    box, saves on Ctrl-S, keeps Windows line ends where it found them, and asks before
    unsaved text is left. What the files take of the disk, and what is left of it, is
    asked for after the folder is painted.
- *Accounts.* The account made at setup owns the Homewarp and makes the others
  (Settings). Another account sees no server until the owner lets it into one, on that
  server's Users tab; being let in is being let look, and what it may do besides is one
  of six things: console, power, files, backups, schedules, settings. To an account
  that has not been let into a server, the server is not there (404, not 403). Making
  and removing servers, templates, the VPS and the accounts are the owner's. An account
  changes its own password; the owner can give it another, which signs it out.
- *Activity.* What is done through the panel is written down once it has been done:
  who, when, to which server, and a line of detail such as a path or the command that
  was typed. The owner reads it on the Activity page, by server and by account. A
  sign-in with the wrong password is written down against the account it was for, and
  what was typed where no account is, is not. Lines are kept half a year.
- *Backups.* One `tar.zst` of everything in a server's files, kept in `backups/` beside
  the servers' files and not among them, so that a server cannot reach its own. Made in
  the background while the server runs; the newest three are kept unless the Backups
  tab says another number. Putting one back needs the server stopped, shows it as
  *Restoring* so that nothing starts it half way, reads the backup through to its end
  before it touches a file, and says in the console how it went.
- *Schedules.* Up to ten steps, each a command, a start, a stop, a restart, a kill or a
  backup, with a wait before it, at times written as cron writes them. The times are
  read on the clock of whoever set them, as so many minutes from UTC: a fixed distance,
  so where clocks change in summer a schedule runs an hour off for half the year.
  What came due while Homewarp was not running is passed over, not run late. Each run
  says what it came to, and is written down under Homewarp's own name.
- *SFTP.* The same files through the same `cap-std` directory, for a program made to
  move many of them. A sign-in is an account and a server at once, `sam.3`, with the
  account's own password and nothing else, and is let in if the account may use the
  Files tab there. There is no shell and no command: either is refused at once. It is
  reached through a door of its own on port 2022 (§10: the door), and its host key is
  made on first start and kept beside the database.
- *Traffic.* The Gate's table has the forwarded ports once more, as two sets whose
  every element counts what matched it; Core adds what is new to the hour each time it
  hears from the Gate, and the Network page shows the last day. A VPS whose `nft` is
  too old for such sets refuses that table, and is given the one without them.
- *Resolvers* are a setting now (Settings, the owner's): one to three addresses on the
  internet. One at home is refused in words, for the reason §5.6 gives.
- *Exit, driven against a throwaway copy on the homelab's own Docker.* A server made
  from a one-line egg whose console is a shell:
  - files uploaded, moved, packed, deleted, unpacked and edited through the page, in
    both themes and at a phone's width; every file belonged to the server's user;
  - a backup made while it ran, a restore refused until it was stopped, then done:
    the changed file was as it had been and the added one was gone;
  - a schedule set for every minute ran by itself within the minute and typed its line
    into the console; one set off by hand made its backup, and another stopped the
    server, waited, and started it;
  - a second account saw nothing; let in with `files` alone it had two tabs, no power
    controls and no line to type into;
  - OpenSSH's `sftp` listed, uploaded, downloaded the same bytes back, renamed and
    deleted, and `get ../../../homewarp.db` found no such file; the wrong password, the
    account that had not been let in, a server that is not there and `ssh … id` were
    all refused;
  - the lab (§10), whole, with one section more: the Gate counted through the ports
    just used, and home added it to the hour.
- Found on the throwaway copy, and invisible to the tests, which run as an ordinary
  user: Core is root with three of root's powers (deploy/compose.yml), and changing
  the mode of a file is not one of them once the file has been given to the server's
  user. Unpacking wrote every file and then reported each as passed over. A file's mode
  is now set before it is given away, and a file that is unpacked over another takes
  its place.
- Not driven: dropping files onto the page, an upload large enough to watch, a backup
  of a real world, the traffic column with a real VPS behind it, and an SFTP program
  other than OpenSSH's.
- Left for the owner, on staging, which has all of it and still no account: the tabs of
  a server's page, Activity and Settings in the sidebar, and
  `sftp -P 2022 <account>.<server's id>@192.168.1.250`, or the same in a program with
  windows. Staging publishes one port more than it did: 2022.
- Not in it, each said where it shows: the editor colours nothing; packing makes a
  `tar.gz` and no zip; a folder is not uploaded as a folder; a file moved by SFTP is not
  written down one by one, only the sign-in; `setstat` over SFTP is answered and not
  carried out, for the reason above; a server has no limit on its disk; the web bundle
  has passed 500 kB and is not yet split; and sign-ins are not rate-limited, here or in
  the panel, until Phase 5.

**Phase 5 — Hardening and public panel**
- TLS/ACME (domain and IP certificates), TOTP, passkeys, rate limits.
- LAN-egress block, Gate rate limits, "harden VPS" with commit-confirm.
- Fuzz the egg and config-file parsers; path-traversal test suite; threat-model review.
- From Phase 3: the door hides from Core who is asking, which rate limits will need
  (PROXY protocol on the socket, or the door's own count).
- *Begun 2026-10-06. Done so far, in the working tree and not yet on staging:*
  - *Who is asking.* A door sends one line ahead of each connection, the first line of
    the PROXY protocol, and Core reads it off its socket: the panel's and SFTP's. Tried
    on a throwaway copy from another machine on the LAN: the Activity page had that
    machine's address, not the door's.
  - *Sign-in limits.* Five wrong tries from one address, or twenty at one account from
    anywhere, and the next is refused unread for what is left of five minutes (429, with
    `Retry-After`), the right password with it. The panel's sign-in, the setup code and
    SFTP keep one count between them. Counted in memory: a restart forgets it. Tried on
    the same copy: the sixth try was refused, and so was the right password after it.
  - *Two-step sign-in.* A code from an authenticator app (RFC 6238, checked against the
    RFC's own table), taken once and from a clock up to half a minute off; eight
    recovery codes, kept as hashes, each good once. Turned on in Settings by typing a
    code back, off with the password, or by the owner for another account. The sign-in
    page asks for the code once the password is found right. Over SFTP the code is typed
    straight after the password. *Not yet looked at in a browser*, and the secret is
    shown as text and a link, with no picture of it to scan.
- *The panel online (§13 item 5).* The owner, 2026-10-06, asked which domain or whether
  the home network only: "continue with phase 5 the vps is on ssh server1 ip". Read as:
  through the VPS, at its address and no domain. That is the last row of §5.9, and what
  it takes on this VPS, looked at that day and not touched (45.38.42.214, Ubuntu 22.04,
  nginx 1.18 on 80 and 443 for four sites of `apixels.net`, no stream module, no Gate):
  - the panel on a port of its own there, 8443, which the Gate forwards as it forwards
    a server's and nginx never sees; home's end is one more door, and Core ends TLS;
  - a certificate for the address from Let's Encrypt, which proves an address over
    port 80 or 443 and no other. Both are nginx's. So one location on nginx's port 80,
    `/.well-known/acme-challenge/`, is passed through the tunnel to Core. That is a
    change to the VPS that stays, and is the owner's to allow; nothing else of nginx's
    is touched, and nginx sees the challenge and never the panel;
  - such certificates last six days, so Core renews by itself, and says on the Network
    page when it last did;
  - passkeys are bound to a name and not to an address, so there are none without a
    domain. Two-step sign-in is what stands in their place.
  - *Later the same day* the owner added: "maybe ill create homewarp.apixels.net and
    point it on server1 IP". With a name the certificate is an ordinary one of ninety
    days, and passkeys can be had. What does not change is that nginx has ports 80 and
    443 there, which leaves two ways, and the choice is the owner's:
    - *nginx ends TLS*, as it does for the four sites beside it, and passes the panel
      home through the tunnel. The address is `https://homewarp.apixels.net`, and the
      VPS reads everything: passwords, sessions, files. The first row of §6 stops being
      true.
    - *Core ends TLS at home*, and the Gate forwards a port of the panel's own, 8443.
      The address is `https://homewarp.apixels.net:8443`, nginx gets one small server
      block that passes the certificate's challenge through the tunnel and sends the
      rest to that address, and the VPS never sees the panel. This is the one §5.9 and
      §6 were written for, and the one recommended.
  - **Decided by the owner, 2026-10-06: Core ends TLS at home**, with the nginx server
    block that goes with it allowed. The name was made that day and answers with the
    VPS's address. How it was to be built (it is built now: see "The panel online,
    built" further down):
    - *The challenge without a way in.* A taken-over VPS must not reach the panel
      through the tunnel (§6, and the lab checks it), so nginx cannot pass the
      certificate's challenge home. Instead Core, which runs the ACME client and keeps
      every key, asks the Gate over the control channel to put the challenge's answer in
      a file, and nginx serves that file: `server_name homewarp.apixels.net` on port 80,
      `/.well-known/acme-challenge/` from a directory of the Gate's, everything else
      sent to `https://$host:8443`. The answer is no secret. The Gate gains two
      requests, to put such a file and to take it away, and takes only names made of
      the letters a token is made of.
    - *The port.* 8443 is forwarded by the Gate as a server's port is, to a third door
      at home, which passes to a socket where Core ends TLS (rustls) and then serves
      the same panel. Each handshake is done off the accept loop.
    - *The certificate.* Asked of Let's Encrypt's staging first, then of the real one;
      kept beside the database; renewed a month before it ends; swapped in without a
      restart. The Network page says what it is for and when it ends.
    - *With it:* the cookie's `Secure` flag and HSTS on what came in over TLS; the
      panel's own address in Settings, public or not; then passkeys, which the name
      makes possible.
  - *A way back in.* `homewarp two-steps-off <username>`, run on the machine itself
    (`docker exec homewarp homewarp two-steps-off <username>`), for an account that has
    lost both its app and its recovery codes. Whoever can run it can read the database,
    so it asks for nothing, and it is written down under Homewarp's own name.
  - *Kept from home with or without a VPS.* What keeps a server from the machine it
    runs on and the networks behind it was part of the tunnel's table, and so was there
    only while a VPS was connected. It is a table of its own now, `homewarp_keep`, put
    in place whenever Core runs servers and put back each round if a firewall's restart
    takes it. The lab asks for it after the VPS is disconnected.
  - *The panel online, built (2026-10-07).* The Network page has "Panel address": the
    owner gives the panel a name that leads to the VPS, agrees to the authority's terms,
    and Core asks for a certificate at once and keeps it renewed (a look every hour, a
    new one with a third of the old one's time left). What was built for it:
    - *Gate:* two requests, to put the answer to an authority's question in
      `/run/homewarp-gate/challenges` and to take it away. Names are held to the letters
      a token is made of. Where nothing on the VPS has port 80 the Gate serves the
      answers there itself, for as long as there are any, and lets go of the port after;
      where a web server has it, the reply says so and the panel prints what that
      server is to be told. The service may take port 80, and has a directory under
      `/run` that is gone when it stops.
    - *Core:* TLS ended by rustls on a socket behind a third door (`HOMEWARP_TLS`,
      `HOMEWARP_TLS_PORT`); each handshake in a task of its own, 256 at once at the
      most; HTTP/2 and HTTP/1.1. The certificate is asked for with `instant-acme` over
      HTTP-01, of Let's Encrypt unless `HOMEWARP_ACME` names another, and kept with its
      key and the account in three rows of `settings`. Before the authority is told to
      ask, Core asks the VPS's port 80 itself and stops with words if the wrong thing
      comes back: an authority counts failures. A certificate's dates are read off it
      by a few lines of DER, tested against a certificate made on the spot.
    - *The tunnel:* with a name, the Gate forwards the panel's port as one more, and
      home's rules let the tunnel in to that door alone: a connection that was to that
      port and that Docker passed on. Without a name neither is so. A server cannot be
      given the port.
    - *What a browser gets over TLS:* a cookie marked `Secure`. Not HSTS, unless the
      port is 443: it would be said of the whole name, and on a VPS with a web server
      the name's port 443 is not the panel. And a request over HTTP/2 names its site in
      its address and carries no `Host` header, which the same-site check reads; the
      header is filled in from the address. That was found by reading, before any
      browser met it: every change a browser made there would have been refused. The
      lab now sends what a browser sends.
    - *Sign-in limits where the Gate stands in:* in NAT mode everybody on the internet
      arrives from the Gate's one address, so that address is not counted, or anyone
      could shut the rest out. The count by account still is.
    - *The lab* has an authority of its own (Pebble) and the name `panel.lab`: from
      "nothing answers on the VPS's port 80" through a certificate, a browser on the
      internet that trusts it, the cookie, the browser's own address in Activity, to
      the port closed again with the name taken away.
    - *Not done here:* SFTP is not forwarded, and a page opened by the name says so in
      place of an address that would not work. A VPS with a firewall and no web server
      needs port 80 opened by hand (Phase 6). No certificate for an address alone.
  - *The panel online, on the real machines (2026-10-07).* A throwaway copy on the
    homelab, the Gate on the owner's VPS, the name `homewarp.apixels.net`, and Let's
    Encrypt's staging authority, so that nothing was used up of what the real one allows
    a name in a week. The VPS's nginx was given the one server block the owner allowed
    (port 80 for that name: the answers from the Gate's directory, everything else sent
    to `https://$host:8443`). What was seen:
    - The certificate came at the first asking, nineteen seconds after the name was
      given: the authority asked from two places, nginx answered both with the Gate's
      file, and the file was gone again afterwards. It is for that name and no other,
      and the dates Core read off it are the dates `openssl` reads.
    - From a browser, through the VPS: port 80 leads to `:8443`; signing in sets a
      cookie that is `Secure`, `HttpOnly` and `SameSite=Strict`; a change made from the
      page goes through over HTTP/2; the console's socket opens as `wss://` and frames
      arrive; nothing was refused. Activity had the browser's own address.
    - The Gate on the VPS: 1.2 MB of memory, its directory under `/run` readable by
      nginx, port 8443 forwarded as the one port there was.
    - What it showed that the lab could not: nothing. What the lab had shown first, the
      missing `Host` header among it, would each have been a failed evening there.
    - The staging certificate is one no browser trusts, so the browser was told to go
      on regardless; a passkey cannot be made on such a page. That is left for the
      owner's own deployment, which asks the real authority.
  - *Passkeys.* Made in Settings with the account's password, on the panel's own name
    over TLS or on the machine itself (`localhost`); signed in with from the sign-in
    page, nothing typed and nobody named. The device has to have checked who holds it,
    so no second step is asked for. What a browser sends is read by hand: the few kinds
    of CBOR an authenticator writes, a P-256 key, a signature checked with `ring`; no
    library for it. A challenge counts once and for five minutes; a key whose device
    has counted fewer uses than are known here is taken for a copy. Tried in Chrome
    with its built-in stand-in for a device, at `localhost` through an SSH tunnel:
    made, used, shown in Activity as "with a passkey", removed, refused afterwards.
  - *Two-step sign-in, looked at in a browser:* a wrong code refused in words, the
    right one turning it on, eight recovery codes, the sign-in page asking for the code
    after the password, a code refused the second time, a recovery code taken once.
    Over SFTP the password alone and the password with a wrong code are both refused,
    and with the code after it a file goes up. `homewarp two-steps-off` turns it off
    from the machine and is written down under Homewarp's own name.
  - *A path-traversal suite* (`crates/homewarp-runtime/tests/traversal.rs`): a server's
    folder with links that lead out every way a link can, and every thing the file
    layer does tried with every path that could lead out, 1,400 tries, each in a world
    of its own. What is held to is that the outside is afterwards exactly as it was and
    that nothing that came back was the outside's. The same for archives and for a
    backup put back. Its first catch was itself: one check read a moved link the
    trusting way.
  - *Fuzzing* (`crates/homewarp-template/tests/fuzz.rs`): real eggs and config files
    changed at random and read, 4,000 times with every test run and as many as are
    asked for with `HOMEWARP_FUZZ`; and by name, a YAML file that says ten thousand
    million things in eleven lines, and files nested a hundred thousand deep. A config
    file is the server's own, and Core reads it as root at every start.
  - *The Gate's limit on new connections is a setting* (Settings, New connections):
    one number, sent with everything else the Gate is told, thirty a second where
    nothing is set.
  - *"Harden this VPS"* (Network, The VPS itself). What arrives at the VPS itself from
    the internet is dropped, but for what was listening there when the owner asked, and
    new SSH connections from one address are held to twelve a minute. Only what arrives
    on the public interface for the machine itself is looked at: what is forwarded to
    servers never comes that way, nor what arrives by the tunnel or by an interface of
    something else's (Tailscale, Docker). It is one chain in the Gate's own table, and
    no rule of anyone else's is touched.
    - *On trial.* It is put in place for a minute, and the Gate undoes it by itself
      unless it is told within that minute, through the tunnel, to keep it. The panel
      shows the seconds, and says what to do with them: open a new SSH session. Kept, it
      is written down on the VPS (`guard.json`) and comes back after a restart.
    - *What is open* is what was listening, as the kernel's own tables of sockets have
      it, and the tunnel's port whatever was asked. What begins to listen afterwards is
      shut until the VPS is hardened again, and the panel names those ports. While the
      Gate answers a certificate authority's question itself, port 80 is open for that
      and shut again after.
    - *The lab* runs a service on its VPS, hardens it, starts another, lets the minute
      run out, hardens again and keeps it, restarts the Gate, asks a certificate's
      question of it, and takes the guard off: 26 checks.
    - *On the owner's VPS* (2026-10-07, from the throwaway copy, and taken off again
      afterwards). What the Gate found listening was what `ss` lists there: SSH, nginx
      on 80 and 443, Tailscale's ports, the address-asking client, the tunnel; nothing
      of what listens for the machine alone. Hardened on trial: a new SSH session got
      in, the websites answered, the panel answered through the VPS, and 21 packets
      from the internet had been dropped within ten seconds. Not kept: undone by the
      Gate sixty seconds to the second after it was put in place, with nothing written
      down. Hardened again and kept: written down, back after the Gate was restarted,
      SSH and the sites as before. Then from the page in a browser: the seconds
      counting down, kept, undone. (A port nothing listens on is dropped there with or
      without the guard: the VPS has a firewall of its own.)
  - *The review of §6* is in §6, under "Held against what was built". Four things were
    changed because of it.
- *Left of Phase 5:* nothing that was planned. What it showed and did not do is in §6:
  no limit on a flood from many addresses, none on what a server writes to disk, and
  servers' own traffic still leaves by the home's line.

**Phase 6 — Packaging and onboarding**
- One-line installers, signed releases, self-update, ARM64 builds of Core, docs.
- Template catalogue browser, and importing an egg from a URL (from Phase 2: both have
  Core fetch from the internet); first-run wizard polish; five-minute target measured.
- From Phase 3: the Gate's one line, which fetches the program before `join` runs;
  firewalld set up by `join` and not only described; the tunnel's addresses checked
  against the home machine's own networks; a home machine with a firewall of its own
  and no Docker to publish the panel's port for it.
- *Begun 2026-10-07. Done:*
  - *An egg from its address, and a catalogue* (Templates, Import). The import page
    takes an address as well as a file or pasted text, and lists the eggs the Pelican
    community publishes: 326 in ten collections that day, all under the MIT licence.
    Homewarp ships none of them. One is fetched from its authors when it is picked and
    put where a pasted egg would be, to be read before it is imported. Core fetches on
    its owner's word from inside a home network, so what it fetches is held to `https`,
    the usual port and a name; the name has to lead to addresses on the internet and to
    none inside a private network, and the address that was looked at is the one
    connected to. A megabyte and twenty seconds at the most, and a redirection is held
    to the same again. Tried against the internet from a throwaway copy: the list, an
    egg, an egg by way of a redirection, and four public names that lead to this
    machine, a home network and a cloud's own address, each refused.
  - *The tunnel's addresses held against the home machine's own networks.* Connecting a
    VPS looks at the machine's routes first, and stops with the network named if one
    already has `10.213.77.0/30` in it.
  - *Core for ARM64.* It did not link: its C parts were compiled against glibc's
    headers and linked with musl, which for the Gate had happened to work. The build
    container has a compiler for ARM64 that reads musl's headers and the kernel's now.
    Core is one static file of 21 MB there, and under an emulator makes its database
    and answers a question of it.
  - *Releases, and one line on each machine.* `scripts/dev.sh release` builds both
    programs for both processors and puts a release together: the four files, a list of
    their checksums, that list signed with an Ed25519 key, and two install scripts with
    the release's address and the key's public half written into them.
    - *At home:* `curl -fsSL <releases>/install.sh | sh` fetches Core, checks the
      signature and then the checksum, builds an image from it and Alpine's `nft` and
      `ip`, and starts it with its three doors. Run again it is the update, and leaves
      what was made alone.
    - *On the VPS:* the panel gives the line, `curl -fsSL <releases>/install-gate.sh |
      sh -s -- <token>`, where it knows where its releases are; it fetches the Gate,
      checks it the same way and runs `join`.
    - *Trusted for the signature, not for where it came from.* The lab makes a release
      with a key of its own with every run, has its VPS install from it by the panel's
      line, and then changes the program, the list and the signature in turn: each is
      refused, and nothing is installed.
    - *On the real machines* (2026-10-07, a throwaway key, each machine serving the
      release to itself): the home line took seven seconds on the homelab, as a second
      instance beside staging; its panel gave the line for the VPS; the VPS, with
      nothing of Homewarp on it, fetched, checked with its own `openssl` 3.0.2,
      installed and was connected, 48 ms away with players' addresses preserved. The
      home line run a second time kept the account and the VPS. Everything was taken
      off both machines again.
    - Both scripts are one function called on their last line, so that one cut off on
      the way does nothing at all.
  - *Docs:* `docs/installing.md` and `docs/releasing.md`.
  - *firewalld, set up by `join` and no longer only described.* No machine here runs
    firewalld, so the lab was given one: `GATE_FW=firewalld bash scripts/dev.sh lab`
    makes the simulated VPS an AlmaLinux 9 with firewalld 1.3 running as it does when
    first installed, and the whole lab is then run behind it.
    - *What `join` asks for:* the tunnel's UDP port in the zone players arrive in, and a
      zone of the Gate's own, `homewarp`, for the tunnel's interface, with the Gate's API
      port in it and nothing else. Both go into what firewalld keeps, and one reload
      takes them up. A VPS connected a second time has the zone already, which is not
      an error. `leave` takes both away again and leaves what the owner had opened.
    - *What it does not need:* an opening for what players send. firewalld passes on
      what another table has redirected, and a forward of the Gate's is that. Seen in
      its rules (`ct status dnat accept`) and then in the lab, where a player's own
      address arrives at the server with firewalld in front.
    - *Narrower than what was described before.* The advice `join` used to print put
      the tunnel's interface in firewalld's `trusted` zone, which would have let home
      reach everything on the VPS. In its own zone, home reaches the Gate's API and a
      service beside it is refused, which the lab tries.
    - *Port 80 is the owner's to open,* as with ufw, and only when the panel is given a
      name. The lab asks for a certificate with it shut: there is none, and Core says
      why and gives the two commands, where before it passed on the authority's
      "no route to host" and nothing more. That needed the authority's refusal to be
      read where it arrives, which is as an error and not as a state of the order.
  - *The first server, from a panel with nothing in it.* "New server" on a new panel
    used to stop at an empty list. It says what an egg is now and leads to the
    catalogue; the import page, come to that way, goes on to the server's form once the
    egg is in, and the list of games has a way to more beneath it. Walked in a browser
    on a throwaway copy: setup code, account, "Find a game", the list fetched, Valheim
    picked and imported, and the form for the server, in nine seconds of a script's
    clicking.
- *The five-minute target, by the parts a machine does:* the home line seven seconds
  (a first time adds fetching Alpine); a VPS from its line to "connected" about ten;
  Paper fetched from the catalogue and installed in eight, and at its first start
  fifteen (Phase 0). Under a minute of machine. The rest is typing, and agreeing to
  Mojang's EULA, which nobody does for the owner. Not yet done with a person and a
  stopwatch.
- *A public release, 1.0.0 (2026-10-07).* Signed with the owner's key, which is on the
  homelab and nowhere in the repository, and served by GitHub Pages from the branch
  `gh-pages` at `https://lanceranara13.github.io/homewarp`; the commit it was built
  from is tagged `v1.0.0`. Fetched back from that address, the four programs matched
  the list and the list its signature. Tried once from there as anyone would: the home
  line put a second Homewarp on the homelab in seven seconds, and the line its panel
  gave put the Gate on the owner's VPS, which opened its firewall and started. Both
  were then taken away again, and whether that panel showed the VPS as connected was
  not looked at. How a release is published is in `docs/releasing.md`.
- *Not done, and why:*
  - *Updating by itself.* Running the line again is the update. A Core that replaces
    the container it runs in, and a Gate that takes a new program through the tunnel,
    are each a piece of work with its own ways of going wrong.
  - *A home machine with a firewall of its own and no Docker.* Homewarp needs Docker
    to run servers at all; the doors are how its ports are published.

**Phase 7 — Later**
- Minecraft hostname routing (many servers on one `:25565`), sleep + wake-on-connect,
  friendly "server offline" responses from the Gate, PROXY protocol v2.
- Player lists, mod/plugin browser, S3 backups, webhooks.
- Import servers from an existing Pelican/Pterodactyl node.
- Multiple Gates (regions) and multiple home nodes.
- *Begun 2026-10-07. Done:*
  - *Who is on a server, and sleep and wake* (Minecraft, Java Edition).
    - *Asked as a game's list of servers asks.* Core speaks the opening of Minecraft's
      protocol and no more (`minecraft.rs`): the handshake, the list's question and its
      answer. A running server is asked every fifteen seconds, on the servers' own
      bridge, and what it says of its players goes to the page with everything else the
      page is told, over the socket it has already: no request is added.
    - *Only a server of Minecraft's is asked.* An egg does not say what game it is, but
      it says which files it sets up, and `server.properties` or `velocity.toml` is one
      of Minecraft's. A server of another game is sent nothing. At first every server
      with a TCP port was asked, four times, and left alone once it had not answered:
      to another game the question is a few bytes of nonsense on its own port. The lab
      showed what that costs. Its stand-in game, a listener with room for five
      connections waiting, lost up to six of twenty players who arrived in the same
      moment as the question, and only then. A server of Minecraft's that does not
      answer four times running (Bedrock, which shares the file's name and speaks over
      UDP) is asked no more.
    - *Sleep.* A server may be given a number of minutes (Settings, Advanced). Empty
      for that long, it is stopped the way its template stops it, and is then *asleep*:
      a state of its own, written down, so that a Homewarp that starts again finds it so.
    - *Wake.* While it sleeps, Core's own program listens on its port in a container of
      the tightest kind (no files, no capabilities, 64 MB), as the probe of the tunnel
      does. A game's list is told that the server sleeps. A player who joins is told to
      come back in a minute, and the stand-in ends, saying who it was; Core starts the
      server and writes down the name and the address. Asking wakes nothing, and
      neither does anything that is not the first packet of a login with a name in it.
      Whoever can reach the port can send that packet, though: the stand-in has no
      way to know a player from a stranger, which the server itself decides only once
      it is up. A server that is woken by strangers costs a start each time, and is
      asleep again after its minutes.
    - *Stop, of one that is asleep, keeps it down:* the stand-in goes, and nothing wakes
      it after that. Start wakes it by hand.
    - *A stopped server that says so* (Settings, Advanced; off unless asked for). The
      same stand-in, told to stay: a game's list is told that the server is offline,
      and so is a player who joins, and nothing is started. It is the plan's "friendly
      offline responses", from home and not from the Gate. It costs a small container
      for as long as the server is stopped, which is why it is asked for server by
      server. The lab, where it is tried through the Gate, found what it broke: a
      server with something standing in for it could not be removed, because the
      server's own task and the removal both took the stand-in's container away at the
      same moment and Docker refuses the second. One that is being removed already is
      taken for removed now.
    - *Not Bedrock, and not other games.* Bedrock speaks over UDP, where there is no
      connection to see the beginning of. A game that cannot be asked who is on it
      cannot be known to be empty.
    - *In the lab,* with a server of the lab's own that answers as Minecraft does, and
      through the Gate: left running while somebody is on it; asleep a minute after the
      last one left; a game's list told so; Stop keeps it down; a Homewarp that starts
      again finds it asleep and stands in again; a join wakes it, and the player's own
      address is what is written down.
    - *On the real machines* (2026-10-07, a throwaway copy on the homelab): Velocity from
      the catalogue, which is Minecraft's protocol in somebody else's program and needs
      no EULA. Core's count agreed with `mcstatus`, a program that only asks (0 of 500).
      Asleep after its minute, `mcstatus` was told "Lobby is asleep. Join to wake it
      up.", and `minecraft-protocol`, which logs in as a game does, was told it was
      waking, and Velocity came up. Then the same through the VPS at its public
      address, where what was written down was home's own address as the internet sees
      it. In a browser: the count on the list and on the server's page, the pill, the
      setting, Stop. Both machines were left as found.
  - *The DNS records for a name without a port.* A Minecraft server behind a VPS shows
    the two records (an address for the name, and `_minecraft._tcp` for the port) that
    let players join as `play.example.com`. It is how many servers share one address
    without anything standing between the player and the server.
  - *A config file that is set line by line is not made empty* (found with Velocity's
    egg, on the homelab). Its `velocity.toml` is set by replacing the line that begins
    `bind = `. The file is not there before the first start, Homewarp made it, empty,
    as Wings does, and Velocity, finding a file, never wrote its own: it came up on
    port 25565 whatever the server's port was, at every start. Such a file is left for
    the server to make now, and said so in the console; from the second start the port
    is in it. Files that are set key by key are made as before, with their keys.
  - *Notices* (the plan's "webhooks"; Settings, Notices). Homewarp tells an address of
    the owner's what happens while nobody is at the panel: a server that crashed, and
    that Homewarp has given up starting it again; one put to sleep, or woken, and by
    whom; what a schedule did; a new certificate; a VPS that has stopped answering, and
    that it answers again. Or, if asked, every line of the Activity page.
    - *A notice is a line of the Activity page, sent on.* Whatever is told was written
      down first, so there is one list of what counts as having happened. Three things
      that were only said in a console are written down now for that: a crash, the
      giving up, and the VPS lost and found (after three rounds without an answer, not
      at the first).
    - *As Discord and Slack take a message:* JSON to an address that is itself the
      secret, with the sentence under both the names it is read by (`content`, `text`)
      and the line's parts beside it for a program. Sent one after another, in the
      order things happened; tried three times; sixty-four may wait, and one more is
      let go.
    - *The address is held to what a fetch is held to:* `https`, a name, an address on
      the internet, and no sending on to another. So nothing is sent into a home
      network, and a receiver there is not served. It is kept and not shown again: the
      page and the Activity page have the site and its last four characters.
    - *Tried* against an address that keeps what it is sent, on the internet (a
      throwaway one, deleted after): "Send one now"; a server made to end by itself,
      whose four crashes and the giving up arrived in that order; and, with the real
      VPS connected to a throwaway copy, its Gate stopped and started, which arrived as
      lost and as back. Discord itself was not tried: there is no webhook of the
      owner's here to try it with.
  - *Mods and plugins, from Modrinth* (a Minecraft server's Mods tab).
    - *What it does:* searches Modrinth for what the server runs (one of twelve
      loaders, and a version of Minecraft if one is given), lists a project's newest
      releases, and installs one: into `plugins/` for Paper and its kin and the
      proxies, into `mods/` for Fabric, Quilt, Forge and NeoForge. What is installed is
      listed, and taken out, by the files routes there were already.
    - *Core asks Modrinth, the page does not.* The panel's pages talk to the panel and
      to nothing else, and no picture is loaded from anyone.
    - *Held to more than a fetch is.* A release is named by its id and described by
      Modrinth, not by the page. Its file is fetched only from Modrinth's own file
      server, has to be a `.jar` by a plain name, 64 MB at the most, and is written only
      once its SHA-512 is the one Modrinth gives. What goes into an address (a loader, a
      version, an id) is held to what such a thing looks like first.
    - *A mod is somebody's program, and the page says so.* It runs inside the server's
      container with the server's files, as one uploaded by hand would. Installing is
      for an account that may write the server's files. What a release needs beside
      itself is counted and said, and not installed with it.
    - *The tab is there for a Minecraft server,* which is told by the file its template
      sets up (`server.properties`, `velocity.toml`): an egg does not say what game it
      is.
    - *Tried against Modrinth itself,* from a throwaway copy on the homelab: LuckPerms
      found for Velocity; its releases listed; the newest installed (1.5 MB, the
      server's user's, and its checksum the same by `sha512sum` and Modrinth's API
      asked with `curl`); a release for another loader refused; and Velocity, started
      again, said "Loaded plugin luckperms 5.5.71". In a browser: search, releases,
      install, the list, and removal.
  - *A store elsewhere for backups* (the plan's "S3 backups"; Settings, and each
    server's Backups tab). A backup beside its server is lost with the disk they are
    both on.
    - *What it does:* the owner gives a bucket that is spoken to as Amazon's S3 is (an
      address, a bucket, a folder in it, a key). Every backup that is made is then
      copied there too, read as it is sent and not held in memory; a copy goes when its
      backup goes, by hand or by the count that is kept; one that did not arrive says
      why on the Backups tab, is written down (and so told, where notices are on), and
      can be copied again.
    - *Tried before it is kept.* Saving the settings writes a few bytes to the bucket
      and takes them away, and a store that did not take them is not kept: one that is
      thought to hold copies and holds none is worse than none. The key's secret is
      kept and not given back.
    - *Three requests, signed by hand:* put, delete, and the try. Signature Version 4
      with `ring`, held against the three examples Amazon publishes for it, signature
      for signature. No library of Amazon's, and nothing else of S3.
    - *The store is the owner's own,* at an address the owner typed, and may be at home
      or on plain HTTP. It is not held to what a fetch from a stranger's address is.
    - *Up to 5 GB a backup,* which is what a store takes in one piece. A larger one
      says so; sending in parts is not built.
    - *After a lost disk* there is no Homewarp to ask: the copy is the backup's own
      file, by the server's name and the day, fetched with any S3 tool, put among a
      new server's files and unpacked there.
    - *Tried against MinIO,* a throwaway one on the homelab: a wrong secret, a bucket
      that is not there and a dead address each refused in words; a 300 MB backup
      copied, and fetched back by `curl` signing for itself with the same SHA-256;
      the store stopped (the copy failed, with its reason, and was written down) and
      started (copied again by hand); one kept and a third made (the first two gone
      from the bucket); the last deleted (the bucket empty). Then a copy fetched,
      uploaded under Files and unpacked: the same world, byte for byte. Amazon's own
      was not tried: there is no account of the owner's here to try it with.
- *Not done, and why:*
  - *Answering from the Gate while home is away.* A stopped server can say so now (it
    is among what is done, above), but that is said from home. With home itself away
    the answer would have to come from the VPS: a port of the Gate's own, which a
    VPS's firewall shuts until it is asked. One more opening on somebody's VPS, for a
    courtesy.
  - *Hostname routing, and PROXY protocol with it.* A program that reads the name a
    player typed has to stand between the player and the server, and the server then
    sees the program's address and not the player's, unless it is one that reads the
    PROXY header (Paper and Velocity do, others do not). The DNS records above give a
    name to each server without that.
  - *An importer for another panel's servers.* It would read a Pelican or Pterodactyl
    database and a Wings node's files, and there is neither here to read: the owner's
    old panel left a database on the VPS and no node. What can be had without one is
    written down instead (`docs/moving.md`): the egg is imported as it is, and the
    old panel's own backup is uploaded and unpacked. Of that, only the unpacking has
    been done with a real archive.
  - *More than one home.* Everything assumes one. (More than one Gate was left
    here too, and is built since: Phase 8.)

**Phase 8 — Several VPSes, traffic as it happens, and updates** — *done 2026-10-07, on the owner's word that day; but for a second real VPS, which there is not, and a release made with the owner's key, which is the owner's to make*

- **More than one VPS.** What Phase 7 left as "a design of its own". It turned out
  to be a small one, because the Gate needs none of it: a Gate has one home, and
  goes on as it was.
  - *A tunnel to each, at home.* Each VPS has an interface of its own
    (`homewarp0` to `homewarp7`), four addresses of its own out of `10.213.77.0/27`
    (the first tunnel has the four it always had), and a mark and a routing table of
    its own (`0x4857` and `4857`, and the next numbers after them). Players come from
    anywhere on every tunnel, so the tunnels cannot share an interface: a reply is
    sent back by the mark of the connection it answers, and so leaves by the tunnel
    it came in by. Home's one table of rules says of all the tunnels together what
    it said of the one.
  - *A server is reached through one VPS.* Its owner chooses which, on the server's
    form, where there is more than one. Core says which it would pick and why: of
    those that answer, the one with the least against it, counted in milliseconds
    of the way from home and back, with a busy VPS (the Gate now says its load), one
    that carries much already, and above all one that hides players' addresses each
    counting against. A new server takes that one unless another is asked for; a
    server keeps the one it has; when a VPS is disconnected its servers go to the
    best of those left, and a server that had none takes the first that is
    connected. A Gate is told only the ports of the servers that are reached
    through it. The panel's own port is forwarded by all of them, and a
    certificate's question is answered on all of them, so the panel's name may lead
    to any.
  - *Two Homewarps on one machine.* Core writes down which tunnels are its own
    (`data/tunnels`) and, when it starts, takes down only those. Until now a second
    Core on the machine (the throwaway copy of §10) took the first one's tunnel
    down as it started, for the ten seconds the first needed to put it back: found
    on the homelab, where the owner's own Homewarp has a VPS now.
  - *The database.* `gate`, which could hold one row, is `gates`, with the tunnel's
    number and a name; a server has `gate_id`; what a Gate counted is kept under
    the Gate that counted it. A Homewarp with a VPS comes through the change with
    its VPS as the first, on the tunnel it had.
- **Traffic as it happens.**
  - *Through each tunnel.* Core looks every two seconds at what the kernel has
    counted in and out of each tunnel's interface, and keeps five minutes of it.
    Nothing is asked of a VPS for it. The Network page draws all the tunnels
    together and each on its VPS's card.
  - *To and from each server.* Docker says with the rest of a server's usage what
    its interfaces have carried; the server's page shows the two rates beside the
    processor and the memory, and draws the two minutes it has watched.
- **The Network page** is a card to a VPS where it was a diagram of the one
  (DESIGN.md, Key screens), and Settings is divided into groups that are listed
  down its left side.
- **Updates.**
  - *Looking.* Where releases are served there is a list of them, `RELEASES`, a line
    to a channel, signed with the key releases are signed with. Core is built with
    the public half of that key, fetches the list every six hours and when asked,
    and believes it for its signature. A newer release is written down once, which
    is how a notice of it is sent.
  - *Stable and beta.* A beta is numbered as one (`1.2.0-beta.1`) and listed on its
    own line. A Homewarp takes betas only if its owner says so, and then takes what
    has been released too, whichever is newer.
  - *Putting it in place.* A Homewarp that the installer set up, which it knows by
    how its own container was made, fetches the release's program, holds it against
    the signed list of checksums, copies its database (`VACUUM INTO`), and starts a
    helper apart from itself: Docker's own command-line image, given the folder the
    installer made and the Docker socket. The helper puts the program where the
    installer put the old one, tells Compose the new version, and has it start
    everything again. Then it waits half a minute and looks whether the new Core is
    up and has not had to be started twice. If it is not, the old program, the old
    file and the database as it was are put back.
  - *Backing up first.* Every server can be backed up before, as its Backups tab
    does; the update goes on only once every backup is made. The database is copied
    either way, since undoing needs it.
  - *The Gate* is not updated from the panel: commands go from home to the Gate's
    own small API and nowhere else on a VPS (§5.1). The Updates page gives the line
    to run there, which is the install line with `update` where the token would be.
- *What was tried, and where.*
  - *The lab*, which has a second VPS since this phase: 195 checks, none failing.
    With two VPSes connected, a server is reached at its own VPS and not at the
    other, by a player who is seen as themselves; each tunnel has its interface, its
    addresses and its way back; a server goes to the VPS that is left when its own
    is disconnected. Core finds the release it was made from and nothing newer;
    sees a newer one and a beta once the list names them; believes no list that was
    changed after it was signed; and a Gate is replaced by the line and keeps its
    keys.
  - *The homelab*, beside the owner's own Homewarp and without touching it: a second
    Homewarp set up by the installer, from a release signed with a key made for the
    trial and served on the machine's own loopback. From the panel it was updated
    from one beta to the next, with its one server backed up first and running all
    the while: about a minute, and the page came back by itself as the new version.
    Then it was given a release that is signed as any other and whose program does
    not start: half a minute after it was started the helper had put the version
    before it back, and the panel said so, with what the helper had written down.
    Everything of the trial was removed afterwards.
  - *The pages*, in a browser against the lab with two VPSes connected: the Network
    page with its cards and its charts, the choice of VPS on a server's form, a
    server's own traffic, the Updates page.
- *Not done.*
  - *A second real VPS.* The owner has one, and it is the Gate of their own
    Homewarp now. Two tunnels have run in the lab only.
  - *A release.* The numbers above are trial numbers. The next release is the
    owner's to number, build and sign (docs/releasing.md), and the first that an
    installed Homewarp can find by itself is the one after 1.0.0: 1.0.0 does not
    look, and is updated by the install line once.
  - *Reaching one server through several VPSes at once.* A server has one. Several
    would let players in different places each use the nearest, and is a join table
    and a list of boxes away.

## 12. Risks

| Risk | Plan |
|---|---|
| Transparent-mode return path fights Docker's firewall rules on some hosts | Proven on the real VPS and, with a real Docker daemon, in the lab on both firewall backends (§10). Other Docker versions and hosts with their own firewall are still unknowns. Self-probe + NAT fallback mean it degrades instead of breaking. |
| The VPS's own firewall blocks the tunnel or the forwards (ufw, firewalld) | Found on the real VPS. The installer detects it and opens what it needs through that tool, or stops with the exact command (§5.3). |
| The VPS is not a blank box: ports already taken, other software's NAT rules | Found on the real VPS. Check each forward for a conflict before applying it and say which rule is in the way; never assume 80, 443 or 25565 are free. |
| A cheap VPS has less CPU than its size says (steal) | Measured: about 40–60 Mbit/s through the tunnel on the owner's VPS. Show Gate CPU steal on the Network page and warn when it is high. |
| Core in a container may be unable to set host sysctls it needs elsewhere | Fine on this homelab (values already correct). Checked at startup with a clear message; the systemd install path covers other hosts. |
| Egg edge cases (odd config parsers, startup quirks) | A first run over 116 published eggs read all but one and found two faults in the importer (§10). Still to do: keep such a corpus as a standing test; report unsupported features at import time rather than at start. |
| One public IP means one `:25565` | v1: per-server ports + show the SRV record to add. Phase 7: hostname routing. |
| Homelab disk nearly full | Build-cache budget and prune script from day one; flagged to the owner. |
| Slow builds on 2 vCPU | Warm incremental checks at home; release builds can move to CI later. |
| ISP blocks or throttles UDP (rare) | Later: WireGuard over TCP/WebSocket as a fallback transport. |
| Egg and yolk licensing for bundling | Verify each source repository's licence before shipping bundled templates; otherwise fetch on demand. |

## 13. Open questions

Decided by the owner on 2026-10-05:

| Question | Decision |
|---|---|
| Name | Homewarp (§2). Remaining: trademark and `.com` / `.gg` check, and renaming the project folder when the repository is created. |
| Design direction | A · Deepslate — what `DESIGN.md` already describes, so it stands as written. |
| VPS connection | Built-in WireGuard, with Tailscale as an option (§5.10). |
| Panel access | Home network only (§5.9). |
| Earning | Free, donations only (§14). |
| Front end | React (§8). |
| VPS | The owner has one, reachable as `ssh server1`; described and tested in §10. |

Still open:

1. **Licence** — decided by the owner on 2026-10-07: AGPL-3.0-or-later (`LICENSE`).
   It had stood open as: MIT/Apache-2.0 (like
   Calagopus/Pterodactyl) or AGPL-3.0 (like Pelican)? With free, donations only, either
   works: MIT/Apache maximises adoption; AGPL stops a company taking a modified version
   closed.
2. **Disk at home** — decided by the owner on 2026-10-06: server data goes in the
   deployment's own `data/` directory (`/home/lance/homewarp/data`), no separate volume.
   The homelab had 31 GB free that day, 87 % used, and a server with its images takes a
   few of them, so free space is the thing to watch and to show.
3. **The VPS's state** (§10) — SSH accepting passwords, and 197 package updates
   pending. The old forwards, the full disk and the missing log rotation are dealt with.
   Neither blocks the lab or a first Gate.
4. **Tailscale test** — put the homelab on the tailnet, or let the VPS accept the subnet
   router's routes, so the external-link mode can be tried on the real machines.
5. **The panel online, at a domain, through the VPS** — raised by the owner on 2026-10-06,
   in their words: "i think i want it to be hosted to the VPS so that it can be accessable
   online using maybe reverse proxy and connect it to domain". Not decided. If confirmed
   it replaces "home network only" above. What has to be settled:
   - *Where the panel runs.* It can stay at home and be reached through the VPS, which
     keeps §5.1 intact. Moving it onto the VPS would put the keys to the home machine's
     Docker on the exposed box and bring back the panel-and-daemon split this design
     removed.
   - *Who ends TLS.* A reverse proxy on the VPS (its nginx already holds 80 and 443)
     reads everything, passwords and session cookies included, so a taken-over VPS
     becomes a taken-over panel: the first row of §6 stops being true. Passing TLS
     through untouched keeps it true (§5.9), but on this VPS that means putting every
     existing site behind an SNI router.
   - *What must exist first.* A link from the VPS to home (the Gate of Phase 3, or
     something hand-made until then), a rule at home letting that link reach the panel's
     port and nothing else, and the Phase 5 items listed under Phase 1 in §11.
   - *The VPS itself* still takes root logins by password (item 3). That matters more
     once it stands in front of the panel.
6. **Core's privileges on the homelab** — the Docker socket has the owner's go-ahead,
   given on 2026-10-06 in the words "sure ok if its non breaking". That condition is a
   rule for the runtime: Core touches only the containers, networks and volumes it made
   and labelled as its own, never prunes, and never restarts the daemon. Setting up the
   tunnel from a container in the host's own network namespace has it too: the owner
   was told on 2026-10-06 what Core would make there (one interface, one nftables table,
   one routing rule and its table, and none of them until a VPS is connected) and what
   the Gate would put on the VPS (one binary, one small directory, one systemd unit and
   the openings in ufw), and answered "Sure commit and dont stop until finished with
   phase 3". The same rule holds for the network as for Docker: Core touches what it
   made and nothing else, and `deploy/compose.yml` says what that is.

## 14. Earning from it (explored 2026-10-05)

Two payment shapes fit, and they fit different things:

- A **one-time code** suits software that runs on the buyer's own machine.
- A **subscription** suits a service that costs money every month to run.

### What comparable products charge

| Product | What it is | Price | Shape |
|---|---|---|---|
| AMP (CubeCoders) | Closed-source game panel | From £7.50 once (5 servers) up to £30 (50 servers); lifetime updates | One-time |
| Unraid | Home server OS | $49 or $109 once with a year of updates, then an optional $36/year; $249 lifetime | One-time, paid updates |
| playit.gg | Hosted tunnel for game servers | Free tier; $3/month or $30/year | Subscription |
| Portwarp | Hosted tunnel for game servers | Free tier; $2.99/month | Subscription |
| Home Assistant Cloud | Remote access for free home software | $6.50/month or $65/year; nothing in the software is locked | Subscription for a service |
| Coolify | Free self-hosted deploy tool | Self-hosted free with every feature; $5/month for a managed dashboard | Subscription for a service |
| Pangolin | Self-hosted tunnel and proxy | Free under AGPL; paid self-hosted from $449/year; cloud from $4/user/month | Dual licence + cloud |
| Pelican, Pterodactyl | Free game panels | Free; donations | None |

### Options

| Model | What is sold | Fit |
|---|---|---|
| Free, donations only | Nothing | Simplest. Income small and unpredictable. |
| **One-time supporter code** | The software, once | Good. Home hosters accept paying once and resist monthly fees for software on their own hardware. |
| One-time code, paid updates after a year | Software, then updates | Workable once releases are regular. More to explain. |
| Subscription for features | The software, monthly | Poor. Feels like rent on your own machine and needs a licence server that must stay reachable — at odds with a LAN-only tool. |
| **Subscription for a hosted relay** | A service: no VPS needed | An honest subscription, since it has a monthly cost. Competes with playit.gg and Portwarp at about $3, and means carrying bandwidth, abuse handling and uptime. |
| Commercial licence for hosting companies | Rights | Only matters if companies adopt it. Requires AGPL. |

### Decision (owner, 2026-10-05): free, donations only

Homewarp is free. No licence codes, no subscription, no locked features, and therefore
no licence-checking code and no entitlement layer to build. The sections below are kept
as the record of what was compared.

What this needs:

- A donation link or two (GitHub Sponsors, Ko-fi or similar). Check that the chosen
  platform pays out to the owner's country before publishing links.
- The **donation heart** in the panel — a pixel heart, the way games draw health. It is
  specified in `DESIGN.md` (Components → Donation heart).
- A "Support" line in the README and the docs.

Rules for asking, so it attracts without nagging:

| Where | Behaviour |
|---|---|
| Panel shell | A small heart, always in the same corner. Click opens a popover with the links. |
| First success | Once per install, when the first server reaches Running: a toast with one sentence and one link. Never again. |
| Settings → About | The links, and a switch to hide the heart. |
| Never | No modal, no banner, no countdown, no startup prompt, nothing on the console or file pages, no tracking of clicks. |

The staged alternative that was recommended before the decision, for the record: launch
free, add a one-time supporter code (roughly $15–25) once people use it, and a hosted
relay subscription only on demand.

### How a one-time code works

1. The buyer pays on a checkout page run by a merchant of record, which also collects and
   remits sales tax and VAT.
2. A webhook issues a code: a small signed record — id, tier, features, issue date,
   updates-until — signed with Ed25519 and encoded as text.
3. The buyer pastes it into Settings → Licence.
4. Core verifies the signature with a public key built into the binary. No account, no
   internet access, no expiry.

Accepted cost: a code can be shared and cannot be withdrawn after a refund. Online
activation counts would fix that at the price of phoning home; not worth it here.

### How a subscription works

- **Hosted relay:** enforced on the relay. An account and a token; the relay stops
  forwarding when payment stops. Nothing to enforce inside Core.
- **Features:** Core would hold a short-lived signed licence and renew it against a licence
  server every few weeks, with a grace period offline. More code, more failure modes, and
  the least liked model among self-hosters. Not recommended.

### Licence interplay

Under MIT/Apache anyone may remove the check and redistribute, so a permissive licence
only supports the supporter/honour model. To keep paid extras or a commercial licence
possible:

- **AGPL-3.0 core + commercial licence** (Pangolin's approach) — recommended if any
  income is intended. Outside contributions then need a contributor agreement.
- MIT/Apache core + a closed add-on module in official builds (open core).
- A source-available licence such as FSL — visible code, not open source.

### Tooling

| Need | Options | Notes |
|---|---|---|
| Taking payment, handling tax | Lemon Squeezy, Paddle, Polar | About 5 % + 50 ¢ per sale (roughly $1.50 on a $20 code). Gumroad is 10 % + 50 ¢. Check that payouts reach the owner's country before choosing. |
| Issuing codes | Built-in licence keys in Polar or Lemon Squeezy; Keygen for offline-verifiable signed keys | Keygen's Community Edition is free to self-host and there is a `keygen-rs` crate. A hand-rolled Ed25519 signer is also small. |
| Donations | GitHub Sponsors, Ko-fi | No code. |

## Sources

- https://pelican.dev/docs/panel/getting-started
- https://pelican.dev/docs/eggs/creating-a-custom-egg/
- https://pelican.dev/docs/comparison/
- https://raw.githubusercontent.com/pelican-eggs/minecraft/refs/heads/main/java/paper/egg-paper.yaml
- https://github.com/calagopus/panel
- https://calagopus.com/compare/pterodactyl-vs-pelican
- https://github.com/fosrl/pangolin
- https://docs.pangolin.net/development/system-architecture
- https://letsencrypt.org/2026/01/15/6day-and-ip-general-availability
- https://www.procustodibus.com/blog/2022/09/wireguard-port-forward-from-internet/
- https://github.com/DefGuard/wireguard-rs
- https://github.com/anderspitman/awesome-tunneling
- https://tailscale.com/docs/features/tailscale-funnel
- https://tailscale.com/blog/tailscale-rs-rust-tsnet-library-preview
- https://tailscale.com/docs/features/tags
- https://tailscale.com/blog/more-throughput
- https://ryanwelch.co.uk/blog/port-forward-with-tailscale/
- https://getdeploying.com/reference/compute-prices
- https://lowendbox.com/blog/1-vps-1-usd-vps-per-month/
- https://space-node.net/blog/minecraft-server-bandwidth-requirements-2026
- https://dev.to/ricco020/i-ran-100-iperf3-benchmarks-of-wireguard-vs-openvpn-on-contabo-vps-heres-the-raw-data-b52
- https://cubecoders.com/AMP
- https://unraid.net/pricing/
- https://playit.gg/pricing
- https://portwarp.com/compare/playit-gg-vs-portwarp
- https://www.nabucasa.com/
- https://pangolin.net/pricing
- https://polar.sh/docs/features/benefits/license-keys
- https://keygen.sh/docs/choosing-a-licensing-model/offline-licenses/
