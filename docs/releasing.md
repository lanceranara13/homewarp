# Making a release

A release is a folder that a web server serves. Whoever installs from it trusts
one thing, a signing key, and so the key is the first thing to have.

## The key, once

```sh
ssh home 'umask 077; openssl genpkey -algorithm ed25519 -out /home/lance/homewarp/release.key'
```

It is an Ed25519 private key. Whoever has it can sign a release that every
install script already out there will accept, so it stays with its owner: not
in the repository, and with a copy somewhere that is not the homelab. If it is
lost, a new key means new install scripts, and machines that update with an old
script will refuse what the new key signed, which is what they should do.

## The release

```sh
RELEASES=https://lanceranara13.github.io/homewarp bash scripts/dev.sh release
```

A beta is made the same way, with `CHANNEL=beta` before it and a beta's number
in `Cargo.toml` (`1.2.0-beta.1`). Only a Homewarp whose owner asked for betas
is offered it, and the one line in the README goes on installing the newest
that was released.

`RELEASES` is where the folder will be reached. It is written into the install
scripts, and an installed Homewarp tells it to the VPS it gives a command to,
so it has to be the real address before the release is made.

On the homelab this builds the web interface, then both programs for x86-64 and
for ARM64 as single files that need nothing installed, runs the ARM64 ones in
an emulator to see that they start, and puts together `/home/lance/homewarp/release`:

```
install.sh                 Homewarp at home, in one line: the newest that was released
install-gate.sh            the Gate on a VPS, in one line
RELEASES                   the newest release of each channel, a line to a channel
RELEASES.sig               that list, signed with the key
1.1.0/
  homewarp-x86_64          Core, with the web interface inside
  homewarp-arm64
  homewarp-gate-x86_64     the Gate
  homewarp-gate-arm64
  SHA256SUMS               the checksums of those four
  SHA256SUMS.sig           that list, signed with the key
  install.sh               the two scripts once more, as they are for this version
  install-gate.sh
```

`RELEASES` is what an installed Homewarp reads to find out whether there is a
newer one (Settings, then Updates, in the panel). It holds a line for `stable`
and a line for `beta`, and a release rewrites the line of its own channel and
keeps the other. Where the folder on the homelab has no list, because it was
made anew, the one that is served already is fetched and kept if it is signed
with the same key.

Core is built with the public half of the key inside it. That is what it holds
the list against, and the checksums of a release it is about to put in place of
itself: a release is believed for its signature, wherever it was fetched from.

The version is the one in `Cargo.toml`, which is set and committed before the
release is made (`web/openapi.json` says it too). A second release of the same
version replaces the first.

## Serving it

Any web server that serves files will do, at the address that was given as
`RELEASES`, over HTTPS: the install scripts are the one thing an installer
takes on trust, and HTTPS is what that trust rests on. Copy the whole folder,
the two scripts and the version's folder with them.

Homewarp's own are served by GitHub Pages, from the branch `gh-pages` of this
repository, which holds the release folder and nothing of the source. A new
release is laid over what is there and pushed:

```sh
git worktree add ../homewarp-pages gh-pages
ssh home 'tar -C /home/lance/homewarp/release -cf - .' | tar -C ../homewarp-pages -xf -
git -C ../homewarp-pages add -A
git -C ../homewarp-pages commit -m "Release 1.0.0"
git -C ../homewarp-pages push
git worktree remove ../homewarp-pages
```

Older versions' folders stay, and the two scripts at the top are the newest
release's: they name its version. The branch has a `.gitattributes` that keeps
git from changing a line ending, since a file that differs by one byte is not
the file that was signed, and a `.nojekyll` so that Pages serves the files as
they are. Pages takes a minute to show a push. The commit the release was built
from is tagged (`git tag v1.0.0`, `git push origin v1.0.0`).

## Checking it

What an installer does can be done by hand:

```sh
cd release/1.0.0
openssl pkey -in ../../release.key -pubout -out /tmp/release.pub
openssl pkeyutl -verify -pubin -inkey /tmp/release.pub -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum -c SHA256SUMS
```

The lab does the same from the other side with every run (`lab/run.sh`): it
makes a release with a key of its own, has its VPS install from it by the line
the panel gives, and then changes the program, the list and the signature in
turn to see each refused.

## What is not here yet

- **Nothing updates without being asked.** A Homewarp looks for a newer
  release by itself, and says so in the panel and in a notice; putting it in
  place is a button its owner presses.
- **The Gate is not updated from the panel.** Nothing at home can run a thing
  on a VPS. The Updates page gives the line to run there.
- **No image in a registry.** The installer builds the image on the machine,
  from the program and Alpine's packages for `nft` and `ip`.
- **1.0.0 does not look.** It was released before there was a list to look
  at, so it is updated by the install line, once.
