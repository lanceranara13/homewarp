# Installing Homewarp

Two machines, one line on each. The first is the machine at home that will run
the servers. The second is a small VPS that players reach, and it is optional:
without one, servers are reached on the home network only.

`RELEASES` below stands for the address a release is served from. There is no
public one yet: until there is, a release is made and served by whoever builds
Homewarp ([releasing.md](releasing.md)).

## At home

It needs Linux on x86-64 or ARM64, Docker with the Compose plugin, `curl` and
`openssl` 3. Docker is not installed for you: what a machine runs its
containers with is its owner's choice.

```sh
curl -fsSL RELEASES/install.sh | sudo sh
```

That fetches Homewarp for the machine's processor, checks it (see *What is
checked*), and starts it. When it is done it prints the panel's address, which
is port 3600 of the machine, and a setup code: the first account is made in
the panel with that code.

What it makes is all in one place, `/opt/homewarp`:

| | |
|---|---|
| `compose.yml` | How Docker runs it. Written again by every install: what is to be different is said to the installer, below. |
| `image/` | The program, and the three lines Docker builds its image from. |
| `data/` | Everything made with Homewarp: the database, servers' files, backups. |

Four containers run. `homewarp` is the program itself, in the machine's own
network, because the tunnel to the VPS lives there. The other three are doors:
the same program, each with one port published and nothing else, passing what
arrives to the first. That is how the panel (3600), SFTP (2022) and the panel
over TLS (8443) are reached on a machine whose firewall shuts what it was not
told about.

Said before the line, these change what it does:

| | Otherwise |
|---|---|
| `HOMEWARP_DIR` | `/opt/homewarp` |
| `HOMEWARP_PORT` | `3600` |
| `HOMEWARP_SFTP_PORT` | `2022` |
| `HOMEWARP_TLS_PORT` | `8443` |
| `HOMEWARP_NAME` | `homewarp`: what the containers are called |

For example `curl -fsSL RELEASES/install.sh | sudo HOMEWARP_PORT=8080 sh`.

**To update**, run the same line again. The newest release takes the place of
the one that is running, and `data/` is left as it is. Servers that are running
stay running.

**To remove it**: `cd /opt/homewarp && sudo docker compose down`, then delete
the servers' containers (`docker ps -a --filter label=homewarp.server`) and the
folder. Deleting the folder deletes every server's files.

## On the VPS

It needs Linux on x86-64 or ARM64 with `nft` (the package `nftables`), `curl`
and `openssl` 3, and to be run as root.

The line is given by the panel, because it carries a token that is made for one
VPS and counts for a quarter of an hour: **Network**, then **Connect a VPS**.
It looks like this:

```sh
curl -fsSL RELEASES/install-gate.sh | sh -s -- eyJrIjoi…
```

It fetches the Gate for the VPS's processor, checks it, and makes the VPS the
Gate of the home whose panel gave the line: a WireGuard tunnel, one nftables
table, a service that keeps both up, and the openings a firewall on the VPS
needs. Nothing else on the VPS is touched, and the panel finds the Gate within
a few seconds.

A VPS with **ufw** or **firewalld** is asked for those openings in its own
words, and `homewarp-gate leave` takes them away again:

- ufw: the tunnel's UDP port, the Gate's API on the tunnel's interface, and
  routed traffic from the public interface into the tunnel.
- firewalld: the tunnel's UDP port in the zone players arrive in, and a zone
  called `homewarp` for the tunnel's interface that lets in the Gate's API and
  nothing else. firewalld is reloaded once to take that up.

Port 80 is not among them. It is needed only when the panel is given a name
(its certificate is asked for there), and on a VPS without a web server you
open it yourself: `ufw allow 80/tcp`, or `firewall-cmd --permanent
--add-service=http` and `firewall-cmd --reload`. The panel says so if it finds
the port shut.

**To remove it**: `homewarp-gate leave`, as root. It takes away everything the
line made.

## What is checked

A release is a folder with the programs in it, a list of their checksums
(`SHA256SUMS`) and that list's signature (`SHA256SUMS.sig`). The signature is
made with a key that only the releaser has; its public half is written into
both install scripts.

Each script fetches the list, checks the signature against that public key
with `openssl`, and only then holds the program against the list. A program
that was changed after it was released is refused, and so is one whose list was
changed to fit it. So nothing is trusted for where it came from, except the
script itself, which is why it is fetched over HTTPS.

If the machine's `openssl` is older than 3.0 it cannot check that kind of
signature, and the script stops and says so. `HOMEWARP_UNSIGNED=1` has it go on
without the check; then where it came from is all there is to trust.
