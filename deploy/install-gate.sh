#!/bin/sh
# Homewarp Gate, in one line. It fetches the Gate for this machine, checks that
# it is the one that was released, and makes this VPS the Gate of the home whose
# panel gave the command:
#
#   curl -fsSL @RELEASES@/install-gate.sh | sh -s -- <join token>
#
# Nothing here is trusted for where it came from. The list of checksums is
# signed, with a key whose public half is written into this script, and the
# program is held against that list before it is put anywhere.
#
# A release fills in what is between the @ signs (scripts/dev.sh release).
set -eu

# All of it is in one function that is called on the last line, so that nothing
# runs until the whole script has arrived: one that was cut off on the way does
# nothing at all, and nothing in it can read the rest of it as its own input.
main() {
RELEASES="${HOMEWARP_RELEASES:-@RELEASES@}"
VERSION="@VERSION@"
PROGRAM=/usr/local/bin/homewarp-gate

say() { printf '%s\n' "$*"; }
stop() {
  printf 'Homewarp: %s\n' "$*" >&2
  exit 1
}
# The name, in its own colour where there is a terminal to show it on and
# nothing has asked for none.
banner() {
  tint='' plain=''
  if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
    tint=$(printf '\033[38;5;141m') plain=$(printf '\033[0m')
    case "${COLORTERM:-}" in truecolor | 24bit) tint=$(printf '\033[38;2;143;124;255m') ;; esac
  fi
  printf '%s' "$tint"
  cat <<'BANNER'

    /\      _   _
   /  \    | | | | ___  _ __ ___   _____      ____ _ _ __ _ __
  / /\ \   | |_| |/ _ \| '_ ` _ \ / _ \ \ /\ / / _` | '__| '_ \
  \ \/ /   |  _  | (_) | | | | | |  __/\ V  V / (_| | |  | |_) |
   \  /    |_| |_|\___/|_| |_| |_|\___| \_/\_/ \__,_|_|  | .__/
    \/                                                   |_|   Gate
BANNER
  printf '%s\n' "$plain"
}
banner

[ "$#" -ge 1 ] || stop "usage: sh -s -- <join token>. The panel gives the whole command: Network, then Connect a VPS."
[ "$(id -u)" = 0 ] || stop "run this as root: it sets up the network."

case "$(uname -m)" in
  x86_64 | amd64) target=x86_64 ;;
  aarch64 | arm64) target=arm64 ;;
  *) stop "there is no Gate for this kind of processor ($(uname -m))." ;;
esac
for tool in curl sha256sum openssl; do
  command -v "$tool" >/dev/null 2>&1 || stop "this needs $tool, which is not installed here."
done
# The signature is one that openssl has been able to check since 3.0.
case "$(openssl version | cut -d' ' -f2)" in
  0.* | 1.*) [ "${HOMEWARP_UNSIGNED:-}" = 1 ] || stop "the openssl here is too old to check the release's signature (it takes 3.0). Nothing was installed." ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() {
  curl -fsSL --retry 2 -o "$work/$1" "$RELEASES/$VERSION/$1" || stop "$1 could not be fetched from $RELEASES."
}
file="homewarp-gate-$target"
fetch SHA256SUMS
fetch SHA256SUMS.sig
fetch "$file"

# The list first: it is what was signed.
cat > "$work/release.pub" <<'KEY'
@PUBLIC_KEY@
KEY
if [ "${HOMEWARP_UNSIGNED:-}" = 1 ]; then
  say "Homewarp: the signature was not checked, as asked."
elif ! openssl pkeyutl -verify -pubin -inkey "$work/release.pub" -rawin \
  -in "$work/SHA256SUMS" -sigfile "$work/SHA256SUMS.sig" >/dev/null 2>&1; then
  stop "the list of checksums is not signed with Homewarp's key. Nothing was installed."
fi
# Then the program against the list.
(cd "$work" && grep " $file\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1) ||
  stop "$file is not the file that was released. Nothing was installed."

# Beside it and then over it: an older one may be running.
cp "$work/$file" "$PROGRAM.new"
chmod 755 "$PROGRAM.new"
mv "$PROGRAM.new" "$PROGRAM"
say "Homewarp Gate $("$PROGRAM" version | cut -d' ' -f2) is on this machine."
# What was fetched is cleared away here: the program takes this one's place,
# and nothing of this script is left to do it afterwards.
rm -rf "$work"
trap - EXIT
exec "$PROGRAM" join "$@"
}

main "$@"
