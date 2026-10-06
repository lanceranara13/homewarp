# Homewarp — project plan

> Name decided 2026-10-05 (see §2). Status: Phase 0 and Phase 1 are done (§11). The tunnel
> is proven on a real VPS and against a real Docker daemon, the Paper egg runs end to end,
> and Core's skeleton — accounts, sessions, first-run setup, the panel's shell — is staged
> on the homelab at port 3600. How the panel is reached is being reconsidered (§13).

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

**Addressing.** Tunnel `10.213.77.0/30`, game bridge `10.213.80.0/24`, both checked for
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

HTTP + JSON. No gRPC, no message broker.

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

### 5.7 Data model (SQLite)

`users`, `sessions`, `api_keys`, `templates`, `template_variables`, `servers`,
`server_variables`, `allocations`, `gates`, `backups`, `schedules`, `schedule_tasks`,
`subusers`, `audit_log`, `settings`.

One file, WAL mode, embedded migrations. No external database.

A template is one row: the whole document the importer made, as JSON, with the egg it
came from beside it. Nothing is ever asked of a part of a template, and a better
importer can read the egg again. So there is no `template_variables` table; a server's
own values can be kept by variable name.

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
| Public via Gate, with domain | Gate forwards 443 raw; **Core terminates TLS** (ACME TLS-ALPN-01). The VPS never sees plaintext. | Opt-in |
| Public via Gate, no domain | Same, using a Let's Encrypt IP-address certificate (GA since January 2026, 160-hour lifetime, auto-renewed). | Opt-in |

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
| `scratch` | a throwaway copy of the last build on `:3601`, with data of its own and the same Docker daemon, for trying what needs an account without touching staging's. `scratch down` removes it, its servers' containers and its data |

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

**Phase 3 — Gate and tunnel**
- `homewarp-gate`, enrollment with key rotation, declarative forwards, self-probe,
  transparent + NAT modes, allocations with a protocol for each port (§5.6; a server
  has one port now, published for TCP and UDP both), Network page with live health.
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
- **Still to do in this phase:** Core's end of the tunnel (the interface, the way back,
  forwards from servers' ports, asking after the Gate); enrolment with a join token and
  fresh keys; the self-probe and the choice of mode; the Network page and the wizard;
  the ARM64 build; and then the real VPS, which is a working machine (§10) and will be
  touched only after the lab is green from end to end.

**Phase 4 — Day-two features**
- File manager (browse, edit, upload, archive/extract), backups (tar.zst, restore,
  retention), schedules, audit log, sub-users and permissions, SFTP.

**Phase 5 — Hardening and public panel**
- TLS/ACME (domain and IP certificates), TOTP, passkeys, rate limits.
- LAN-egress block, Gate rate limits, "harden VPS" with commit-confirm.
- Fuzz the egg and config-file parsers; path-traversal test suite; threat-model review.

**Phase 6 — Packaging and onboarding**
- One-line installers, signed releases, self-update, ARM64 builds of Core, docs.
- Template catalogue browser, and importing an egg from a URL (from Phase 2: both have
  Core fetch from the internet); first-run wizard polish; five-minute target measured.

**Phase 7 — Later**
- Minecraft hostname routing (many servers on one `:25565`), sleep + wake-on-connect,
  friendly "server offline" responses from the Gate, PROXY protocol v2.
- Player lists, mod/plugin browser, S3 backups, webhooks.
- Import servers from an existing Pelican/Pterodactyl node.
- Multiple Gates (regions) and multiple home nodes.

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

1. **Licence** — to be decided later, before the first release. MIT/Apache-2.0 (like
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
   and labelled as its own, never prunes, and never restarts the daemon. Still needing a
   go-ahead before Phase 3 deploys: setting up the tunnel from a container in the host's
   own network namespace.

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
