# Homewarp

<a href="https://ko-fi.com/pixelsthecoder"><img src="assets/heart.svg" width="56" height="48" align="right" alt="Give Homewarp a heart on Ko-fi"></a>

Host game servers on your own PC at home. Friends join through a small VPS, so
your home address stays hidden and your router needs no setup.

Homewarp is a self-hosted panel: one program at home that installs, runs and
looks after game servers in Docker containers, and one small program on a cheap
VPS that passes players through an encrypted tunnel. It is free and open
source, and reads the "eggs" the Pterodactyl and Pelican community already
publishes for hundreds of games.

> **Status: early, and not released yet.** Everything described here is built
> and has been tried on real machines, but there is no public release to
> install from. Until there is, you build a release yourself
> ([docs/releasing.md](docs/releasing.md)). Expect rough edges.

![The Servers page](docs/screenshots/servers.png)

## Contents

- [How it works](#how-it-works)
- [What it does](#what-it-does)
- [Screenshots](#screenshots)
- [What you need](#what-you-need)
- [Installing](#installing)
- [Your first server](#your-first-server)
- [Letting friends in from the internet](#letting-friends-in-from-the-internet)
- [Day to day](#day-to-day)
- [Security](#security)
- [Building from source](#building-from-source)
- [What is not built](#what-is-not-built)
- [Support](#support)
- [Licence](#licence)

## How it works

```
players ──► VPS public IP ══ encrypted tunnel ══► home server ──► game containers
            (homewarp-gate)                        (homewarp + panel)
```

- **Home holds everything.** The panel, the worlds, the backups. One program
  and one database file.
- **The VPS is a front door.** It forwards packets and stores nothing, so the
  cheapest tier is enough: 1 CPU and 512 MB. Replace it with one command.
- **Home opens the tunnel.** No port forwarding, it works behind CGNAT, and
  your home IP is never published.
- **Players keep their own address.** A server sees who is really connecting,
  so bans and limits work as they would on a rented host.
- **Any game.** Templates are Pterodactyl and Pelican eggs, imported as they
  are.

The VPS is optional. Without one, servers are reached on your home network
only.

## What it does

**Servers**
- Install, start, stop and restart servers from a template, each in its own
  locked-down container: not root, no capabilities, read-only root, memory and
  process limits, and kept from your home network.
- A live console to read and type into, with processor and memory use.
- Started again after a crash, a little later each time, and left alone after
  four in a row.
- Files in the browser and over SFTP: edit, upload, download, pack and unpack.
- Backups made by hand or by the clock, kept to a number you choose, and put
  back with one button.
- Schedules: commands, restarts and backups at times you set.

**Templates**
- Import an egg from a file, pasted text, or its address.
- A catalogue of the eggs the Pelican community publishes. Homewarp ships none
  of them: one is fetched from its authors when you pick it, and shown to you
  before it is imported.

**Minecraft (Java Edition)**
- Who is on a server, on its page and in the list.
- Sleep: a server nobody has been on for a while is stopped, and the first
  player to join wakes it.
- A stopped server can tell players that it is offline.
- Mods and plugins from Modrinth, each checked against the checksum Modrinth
  gives before it is written.
- The DNS records that let players join by a name with no port after it.

**Through the VPS**
- Connected with one command, which carries a token that works once.
- The tunnel is put back by itself after a reboot at either end.
- A limit on new connections from one address, dropped at the VPS.
- "Harden this VPS": shuts what is not listening, on trial for a minute and
  undone by itself unless you keep it.
- Works with a VPS that runs ufw or firewalld.

**The panel**
- More accounts, each let into chosen servers for chosen things.
- Two-step sign-in and passkeys, and limits on wrong tries.
- An Activity page of who did what.
- Reachable from anywhere by a name of your own, over TLS that ends at home:
  the VPS passes it on and cannot read it. The certificate is asked for and
  renewed by Homewarp.
- Notices to a Discord or Slack channel when something happens while you are
  away: a crash, a server asleep or woken, a VPS that stopped answering.
- A copy of every backup in a bucket elsewhere (Amazon S3, Cloudflare R2,
  Backblaze B2, MinIO), so a lost disk is not a lost world.

## Screenshots

| | |
|---|---|
| ![A server's console](docs/screenshots/console.png) A server's console, with what it uses of the machine. | ![Mods from Modrinth](docs/screenshots/mods.png) Mods and plugins, found on Modrinth. |
| ![The catalogue of eggs](docs/screenshots/catalogue.png) The catalogue of games to pick from. | ![Backups](docs/screenshots/backups.png) Backups, made and put back. |
| ![The Network page](docs/screenshots/network.png) The Network page, before a VPS is connected. | |

## What you need

**At home:** a Linux machine on x86-64 or ARM64 with Docker and its Compose
plugin, `curl` and `openssl` 3. It stays on while servers run.

**A VPS (optional):** any small Linux VPS with a public IPv4 address, `nft`
(the package `nftables`), `curl` and `openssl` 3. It needs root for one
command.

**A domain (optional):** only to reach the panel from the internet by name, or
to give servers names of their own.

## Installing

Each machine takes one line. `RELEASES` is the address a release is served
from; there is no public one yet, so for now it is wherever you put the release
you built ([docs/releasing.md](docs/releasing.md)).

At home:

```sh
curl -fsSL RELEASES/install.sh | sudo sh
```

It fetches Homewarp for the machine's processor, checks its signature and
checksum, and starts it. When it is done it prints the panel's address, which
is port 3600 of the machine, and a setup code. Everything it makes is in
`/opt/homewarp`. Running the same line again later is the update.

The full account, with what can be changed and how to remove it, is in
[docs/installing.md](docs/installing.md).

## Your first server

1. Open the panel at `http://<your machine>:3600`.
2. Create your account with the setup code. It was printed by the installer,
   and is in `docker logs homewarp` if you have lost it.
3. Choose **New server**, then **Find a game**. Fetch the catalogue, pick a
   game, read the egg that is shown, and import it.
4. Give the server a name, its memory and a port. A game with an end-user
   licence, such as Minecraft, asks you to agree to it here.
5. **Create server.** The console shows the install, and then the server
   starting. It is reached at the address shown beside its name.

Some servers set their port in a file they write themselves on first start. If
the console says a file "is left for the server to make", stop the server and
start it once more.

## Letting friends in from the internet

1. In the panel, open **Network** and choose **Connect a VPS**. Give the VPS's
   public address.
2. The panel shows one command. Run it on the VPS as root. It installs the
   Gate, opens what the VPS's own firewall needs, and connects.
3. Within a few seconds the panel shows the VPS as connected, and each
   server's address becomes the VPS's. Share that address with your friends.

To reach the panel itself from outside, point a name at the VPS and give that
name to the panel on the same page. If the VPS has a firewall and no web
server, open port 80 on it first; the panel says so if it finds it shut.

To take a VPS away again: `homewarp-gate leave` on the VPS, as root.

## Day to day

| To | Go to |
|---|---|
| Put a server to sleep when it is empty | the server's **Settings**, Advanced |
| Add mods or plugins | the server's **Mods** tab |
| Back up now, or choose how many are kept | the server's **Backups** tab |
| Back up every night | the server's **Schedules** tab |
| Let a friend manage one server | the server's **Users** tab |
| Copy backups somewhere else | **Settings**, *A store elsewhere for backups* |
| Be told of crashes | **Settings**, *Notices* |
| Turn on two-step sign-in or add a passkey | **Settings** |

Moving a server over from Pterodactyl or Pelican, and getting one back from a
copy after a lost disk, are in [docs/moving.md](docs/moving.md).

## Security

A panel like this runs other people's programs on your machine, so it is built
on the assumption that a game server may be taken over.

- Servers run without root, without capabilities, on a read-only root, with
  limits, and are blocked from every private network range: a taken-over
  server cannot reach your router, your NAS or the panel.
- The VPS can reach the forwarded ports and nothing else at home. A VPS that
  is taken over learns no world, no password and no key that matters after the
  next key change.
- An egg's scripts run inside containers, and what is fetched from the
  internet is held to public `https` addresses and checked before use.
- Releases are signed, and the installers check the signature before running
  anything.

The whole model, held against the code as built, is in
[PLAN.md](PLAN.md#6-security-model). If you find a hole, please open an issue.

## Building from source

Homewarp is a Rust workspace with a React web interface that is compiled into
the program.

| | |
|---|---|
| `crates/homewarp-core` | The panel and its API, at home. |
| `crates/homewarp-gate` | The program on the VPS. |
| `crates/homewarp-runtime` | Docker, and a server's files. |
| `crates/homewarp-net` | The tunnel and firewall rules. |
| `crates/homewarp-template` | Eggs and config-file patching. |
| `crates/homewarp-proto` | What the two ends say to each other. |
| `web/` | The interface: React, TanStack, Tailwind. |
| `lab/` | A VPS, an internet and a home in containers, for testing the tunnel. |

The interface is built first, because the panel carries it inside:

```sh
cd web && npm install && npm run build && cd ..
cargo build --release -p homewarp-core -p homewarp-gate
cargo test --workspace
```

`scripts/dev.sh` does the same in containers on a second machine, along with
the lab and release builds; it is written around its author's own setup and is
worth reading before use. How a release is put together and signed is in
[docs/releasing.md](docs/releasing.md). The plan, with a record of what was
built and how each part was tried, is [PLAN.md](PLAN.md); the interface's
design is [DESIGN.md](DESIGN.md).

## What is not built

- A public release, and updating by itself: for now an update is the install
  line run again.
- Many Minecraft servers behind one port by hostname. The DNS records the
  panel shows give each server a name without it.
- An importer that reads another panel's database
  ([docs/moving.md](docs/moving.md) has the way by hand).
- More than one VPS, or more than one home machine.
- Bedrock Edition for the Minecraft extras: players, sleep and mods are Java
  Edition only. Bedrock servers themselves run like any other game.

## Support

<a href="https://ko-fi.com/pixelsthecoder"><img src="assets/heart.svg" width="14" height="12" alt=""></a> **[Give Homewarp a heart on Ko-fi](https://ko-fi.com/pixelsthecoder).**

Homewarp is free and stays free: no licence codes, no subscription, nothing
locked. If it saves you a hosting bill, a donation keeps it going.

## Licence

[GNU Affero General Public License v3.0 or later](LICENSE). You may use,
change and share Homewarp freely. If you change it and offer it to others,
including as a hosted service, you share your changes under the same licence.
