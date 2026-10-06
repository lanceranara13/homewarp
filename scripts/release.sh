#!/bin/sh
# Puts a release together from what was built: the programs under the names a
# machine asks for them by, a list of their checksums, that list signed, and the
# two install scripts with the release's address, version and key written in.
#
#   release.sh <built> <to> <version> <address> <key>
#
#   built    where the programs are, as `scripts/dev.sh gate` leaves them
#   to       the folder to make: what a web server then serves at <address>
#   version  as the programs say it
#   address  where that folder will be reached, with no stroke at the end
#   key      an Ed25519 private key in PEM, which signs the list. A new one is
#            made with: openssl genpkey -algorithm ed25519 -out <key>
#
# Whoever has the key can sign a release, and whoever installs trusts the key
# and nothing else. It belongs with its owner and not in this repository.
set -eu

[ "$#" = 5 ] || {
  echo "usage: $0 <built> <to> <version> <address> <key>" >&2
  exit 2
}
built=$1 to=$2 version=$3 address=${4%/} key=$5
here=$(cd "$(dirname "$0")/.." && pwd)
[ -r "$key" ] || {
  echo "there is no key at $key. Make one: openssl genpkey -algorithm ed25519 -out $key" >&2
  exit 1
}

rm -rf "$to/$version"
mkdir -p "$to/$version"
put() { # its name as built, its name as released. Passed over if it was not built.
  [ -e "$built/$1" ] || return 0
  cp "$built/$1" "$to/$version/$2"
}
put homewarp-static homewarp-x86_64
put homewarp-static-arm64 homewarp-arm64
put homewarp-gate homewarp-gate-x86_64
put homewarp-gate-arm64 homewarp-gate-arm64
for needed in homewarp-x86_64 homewarp-gate-x86_64; do
  [ -e "$to/$version/$needed" ] || {
    echo "$needed was not built: there is nothing in $built to make it from" >&2
    exit 1
  }
done

(cd "$to/$version" && sha256sum homewarp-* > SHA256SUMS)
openssl pkeyutl -sign -inkey "$key" -rawin -in "$to/$version/SHA256SUMS" -out "$to/$version/SHA256SUMS.sig"
# What an install script holds the list against: the public half, in PEM.
PUBLIC=$(openssl pkey -in "$key" -pubout)
export PUBLIC

for script in install.sh install-gate.sh; do
  awk -v releases="$address" -v version="$version" '
    $0 == "@PUBLIC_KEY@" { print ENVIRON["PUBLIC"]; next }
    { gsub(/@RELEASES@/, releases); gsub(/@VERSION@/, version); print }
  ' "$here/deploy/$script" > "$to/$script"
  chmod 644 "$to/$script"
done
# Nothing may be left to fill in, or an install would ask the wrong place.
if grep -l '@[A-Z_]*@' "$to/install.sh" "$to/install-gate.sh" >/dev/null; then
  echo "a script still has a place to fill in" >&2
  exit 1
fi

echo "Homewarp $version, to be served at $address:"
(cd "$to" && ls -l install.sh install-gate.sh "$version" | awk 'NF >= 9 { printf "  %10s  %s\n", $5, $9 }')
