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

[ "$#" = 5 ] || [ "$#" = 6 ] || {
  echo "usage: $0 <built> <to> <version> <address> <key> [stable|beta]" >&2
  exit 2
}
built=$1 to=$2 version=$3 address=${4%/} key=$5 channel=${6:-stable}
# A beta is numbered as one, and a release is not: 1.2.0-beta.1 comes before
# 1.2.0, and an installed Homewarp goes by that order.
case "$channel:$version" in
  stable:*-*) echo "$version is a beta's number, and this is to be a release. Say beta, or number it as a release." >&2; exit 2 ;;
  stable:* | beta:*-*) ;;
  beta:*) echo "a beta is numbered as one, such as 1.2.0-beta.1: $version is a release's number." >&2; exit 2 ;;
  *) echo "a release is stable or beta, not $channel." >&2; exit 2 ;;
esac
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
  ' "$here/deploy/$script" > "$to/$version/$script"
  chmod 644 "$to/$version/$script"
  # Nothing may be left to fill in, or an install would ask the wrong place.
  if grep -q '@[A-Z_]*@' "$to/$version/$script"; then
    echo "$script still has a place to fill in" >&2
    exit 1
  fi
  # Every release has its two scripts beside its programs. The two at the top
  # are what the one line in the README fetches: the newest that was released,
  # which a beta is not.
  if [ "$channel" = stable ]; then cp "$to/$version/$script" "$to/$script"; fi
done

# The list of releases, a line to a channel: what an installed Homewarp reads
# to find out whether there is a newer one. It is signed as the checksums are,
# and believed for that and for nothing else.
#
# The line of the other channel is kept. Where this folder has no list, the
# one that is served already is taken, if it is signed with this key: a list
# that anybody could have written is not signed here as Homewarp's own.
list=$to/RELEASES
if [ ! -e "$list" ]; then
  served=$(mktemp -d)
  echo "$PUBLIC" > "$served/release.pub"
  if curl -fsS -m 20 -o "$served/RELEASES" "$address/RELEASES" 2>/dev/null &&
    curl -fsS -m 20 -o "$served/RELEASES.sig" "$address/RELEASES.sig" 2>/dev/null &&
    openssl pkeyutl -verify -pubin -inkey "$served/release.pub" -rawin \
      -in "$served/RELEASES" -sigfile "$served/RELEASES.sig" >/dev/null 2>&1; then
    cp "$served/RELEASES" "$list"
  fi
  rm -rf "$served"
fi
{
  [ -e "$list" ] && grep -E '^(stable|beta) [0-9A-Za-z.+-]+$' "$list" | grep -v "^$channel " || true
  echo "$channel $version"
} | sort > "$list.new"
mv "$list.new" "$list"
openssl pkeyutl -sign -inkey "$key" -rawin -in "$list" -out "$list.sig"

echo "Homewarp $version, a $channel release, to be served at $address:"
(cd "$to" && ls -l RELEASES RELEASES.sig "$version" | awk 'NF >= 9 { printf "  %10s  %s\n", $5, $9 }')
sed 's/^/  on the channel /' "$list"
