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
RELEASES=https://example.com/homewarp bash scripts/dev.sh release
```

`RELEASES` is where the folder will be reached. It is written into the install
scripts, and an installed Homewarp tells it to the VPS it gives a command to,
so it has to be the real address before the release is made.

On the homelab this builds the web interface, then both programs for x86-64 and
for ARM64 as single files that need nothing installed, runs the ARM64 ones in
an emulator to see that they start, and puts together `/home/lance/homewarp/release`:

```
install.sh                 Homewarp at home, in one line
install-gate.sh            the Gate on a VPS, in one line
0.0.0/
  homewarp-x86_64          Core, with the web interface inside
  homewarp-arm64
  homewarp-gate-x86_64     the Gate
  homewarp-gate-arm64
  SHA256SUMS               the checksums of those four
  SHA256SUMS.sig           that list, signed with the key
```

The version is the one in `Cargo.toml`. A second release of the same version
replaces the first.

## Serving it

Any web server that serves files will do, at the address that was given as
`RELEASES`, over HTTPS: the install scripts are the one thing an installer
takes on trust, and HTTPS is what that trust rests on. Copy the whole folder,
the two scripts and the version's folder with them.

## Checking it

What an installer does can be done by hand:

```sh
cd release/0.0.0
openssl pkey -in ../../release.key -pubout -out /tmp/release.pub
openssl pkeyutl -verify -pubin -inkey /tmp/release.pub -rawin -in SHA256SUMS -sigfile SHA256SUMS.sig
sha256sum -c SHA256SUMS
```

The lab does the same from the other side with every run (`lab/run.sh`): it
makes a release with a key of its own, has its VPS install from it by the line
the panel gives, and then changes the program, the list and the signature in
turn to see each refused.

## What is not here yet

- **Nothing updates by itself.** Running the install line again is the update.
- **No image in a registry.** The installer builds the image on the machine,
  from the program and Alpine's packages for `nft` and `ip`.
- **The licence** is not decided (PLAN.md §13), and a public release waits for it.
