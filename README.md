# Homewarp

<img src="assets/heart.svg" width="56" height="48" align="right" alt="Pixel heart">

Host game servers on your own PC at home. Friends join through a small VPS, so your
home address stays hidden and your router needs no setup.

> **Status: early build, not usable yet.** The tunnel design is proven against a real VPS
> and a real Docker daemon: player addresses arrive intact, with no added latency. An
> unmodified Paper egg installs, starts and stops. The panel imports eggs, makes servers
> from them, installs, starts and stops them, starts them again after a crash, and has a
> live console to read and type into. Paper, a SteamCMD game and a non-game app have each
> been installed and run from their published eggs. The tunnel is in it: a VPS is connected
> from the panel with one command, makes keys of its own on first contact, and forwards
> each server's ports home, where the panel checks for itself whether servers see their
> players' own addresses. Both ends put the tunnel back when a reboot or another program
> takes it away. A server's files are managed from the panel and over SFTP, backed up and
> put back, and worked on by the clock; further accounts can be let into single servers
> for chosen things, and what is done is written down. There are no releases yet, so
> nothing here installs with one line. The
> plan is in [PLAN.md](PLAN.md) and the interface design in [DESIGN.md](DESIGN.md).

## How it works

```
players ──► VPS public IP ══ encrypted tunnel ══► home server ──► game containers
            (homewarp-gate)                        (homewarp + panel)
```

- **Home holds everything.** The panel, the worlds, the backups. One program and one
  database file.
- **The VPS is a front door.** It forwards packets and stores nothing, so it is designed
  for the cheapest tier: 1 CPU and 512 MB. Replace it with one command.
- **Home opens the tunnel.** No port forwarding, works behind CGNAT, and your home IP is
  never published.
- **Any game.** Templates import Pterodactyl and Pelican eggs as they are.

## The goal

Two commands to set up — one at home, one on the VPS — a three-step wizard, and a
joinable server in under five minutes. The build order is in
[PLAN.md](PLAN.md#11-phases).

## Support

<img src="assets/heart.svg" width="14" height="12" alt=""> **Give Homewarp a heart.**

Homewarp is free and stays free: no licence codes, no subscription, nothing locked.
If it saves you a hosting bill, a donation keeps it going.

Donation links will be listed here once they are set up.

## Licence

To be decided before the first release.
