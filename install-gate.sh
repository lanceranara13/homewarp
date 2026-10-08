#!/bin/sh
# Homewarp Gate, in one line. It fetches the Gate for this machine, checks that
# it is the one that was released, and makes this VPS the Gate of the home whose
# panel gave the command:
#
#   curl -fsSL https://lanceranara13.github.io/homewarp/install-gate.sh | sh -s -- <join token>
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
RELEASES="${HOMEWARP_RELEASES:-https://lanceranara13.github.io/homewarp}"
VERSION="1.1.2"
PROGRAM=/usr/local/bin/homewarp-gate

# Colour, where there is a terminal to show it on and nothing has asked for
# none. The violet is the name's, and the panel's accent; the rest are the
# panel's colours for what went well and what did not. The same as the one
# at home (install.sh) uses.
plain='' bold='' accent='' dim='' good='' bad=''
c1='' c2='' c3='' c4='' c5='' c6='' c7='' c8=''
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  plain=$(printf '\033[0m') bold=$(printf '\033[1m')
  case "${COLORTERM:-}" in
    # A shade to a letter, from the deep violet to the pale.
    truecolor | 24bit)
      c1=$(printf '\033[38;2;110;86;248m') c2=$(printf '\033[38;2;122;100;249m')
      c3=$(printf '\033[38;2;135;114;250m') c4=$(printf '\033[38;2;147;128;251m')
      c5=$(printf '\033[38;2;159;142;252m') c6=$(printf '\033[38;2;171;156;253m')
      c7=$(printf '\033[38;2;184;170;254m') c8=$(printf '\033[38;2;196;184;255m')
      accent=$(printf '\033[38;2;143;124;255m') dim=$(printf '\033[38;2;138;145;158m')
      good=$(printf '\033[38;2;61;214;140m') bad=$(printf '\033[38;2;255;99;105m')
      ;;
    # One violet for all of the name: it holds until it is taken off.
    *)
      c1=$(printf '\033[38;5;141m') accent=$c1 dim=$(printf '\033[38;5;245m')
      good=$(printf '\033[38;5;78m') bad=$(printf '\033[38;5;203m')
      ;;
  esac
fi

say() { printf '%s\n' "$*"; }
stop() {
  if [ -t 2 ]; then
    printf '%sHomewarp:%s %s\n' "$bad" "$plain" "$*" >&2
  else
    printf 'Homewarp: %s\n' "$*" >&2
  fi
  exit 1
}
# The name, in its own colour, with the mark of the panel's icon before what
# this is.
banner() {
  row() {
    printf '  %s%s%s%s%s%s%s%s%s%s%s%s%s%s%s%s%s\n' "$c1" "$1" "$c2" "$2" "$c3" "$3" "$c4" "$4" \
      "$c5" "$5" "$c6" "$6" "$c7" "$7" "$c8" "$8" "$plain"
  }
  echo
  row '██╗  ██╗' ' ██████╗ ' '███╗   ███╗' '███████╗' '██╗    ██╗' ' █████╗ ' '██████╗ ' '██████╗ '
  row '██║  ██║' '██╔═══██╗' '████╗ ████║' '██╔════╝' '██║    ██║' '██╔══██╗' '██╔══██╗' '██╔══██╗'
  row '███████║' '██║   ██║' '██╔████╔██║' '█████╗  ' '██║ █╗ ██║' '███████║' '██████╔╝' '██████╔╝'
  row '██╔══██║' '██║   ██║' '██║╚██╔╝██║' '██╔══╝  ' '██║███╗██║' '██╔══██║' '██╔══██╗' '██╔═══╝ '
  row '██║  ██║' '╚██████╔╝' '██║ ╚═╝ ██║' '███████╗' '╚███╔███╔╝' '██║  ██║' '██║  ██║' '██║     '
  row '╚═╝  ╚═╝' ' ╚═════╝ ' '╚═╝     ╚═╝' '╚══════╝' ' ╚══╝╚══╝ ' '╚═╝  ╚═╝' '╚═╝  ╚═╝' '╚═╝     '
  printf '\n  %s◆%s %sGate%s %s· the VPS end of Homewarp, where players arrive.%s\n\n' \
    "$accent" "$plain" "$bold" "$plain" "$dim" "$plain"
}
banner

[ "$#" -ge 1 ] || stop "usage: sh -s -- <join token>, or sh -s -- update. The panel gives the whole command: Network, then Connect a VPS."
[ "$(id -u)" = 0 ] || stop "run this as root: it sets up the network."
# With "update" in place of a token, the Gate that is here is replaced by this
# release's and started again. Nothing else of it is touched: its keys, what
# it forwards and its guard are as they were.
UNIT=/etc/systemd/system/homewarp-gate.service
if [ "$1" = update ]; then
  [ -x "$PROGRAM" ] || stop "there is no Gate on this machine to update. The panel gives the command that connects one."
fi

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
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAzaS6ZMZU8dkYCso2FMbCtnf7/gDgcU+9AWbGoyEaml0=
-----END PUBLIC KEY-----
KEY
if [ "${HOMEWARP_UNSIGNED:-}" = 1 ]; then
  say "Homewarp: the signature was not checked, as asked."
elif ! openssl pkeyutl -verify -pubin -inkey "$work/release.pub" -rawin \
  -in "$work/SHA256SUMS" -sigfile "$work/SHA256SUMS.sig" >/dev/null 2>&1; then
  stop "the list of checksums is not signed with Homewarp's key. Nothing was installed."
fi
# And the list is this version's own: every release's list is signed, and
# names the same files, so the version is among what is listed.
named=$(printf '%s\n' "$VERSION" | sha256sum | cut -d' ' -f1)
[ "${HOMEWARP_UNSIGNED:-}" = 1 ] || grep -q "^$named [ *]VERSION\$" "$work/SHA256SUMS" ||
  stop "the list of checksums is not that of Homewarp $VERSION. Nothing was installed."
# Then the program against the list.
(cd "$work" && grep " $file\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1) ||
  stop "$file is not the file that was released. Nothing was installed."

# Beside it and then over it: an older one may be running.
cp "$work/$file" "$PROGRAM.new"
chmod 755 "$PROGRAM.new"
mv "$PROGRAM.new" "$PROGRAM"
printf '%sHomewarp Gate %s is on this machine.%s\n' "$good" "$("$PROGRAM" version | cut -d' ' -f2)" "$plain"
# What was fetched is cleared away here: the program takes this one's place,
# and nothing of this script is left to do it afterwards.
rm -rf "$work"
trap - EXIT
if [ "$1" = update ]; then
  # The service runs the program from where it was just put. Where nothing
  # starts the Gate for this machine, whoever does has to start it again.
  if [ -e "$UNIT" ] && command -v systemctl >/dev/null 2>&1; then
    systemctl restart homewarp-gate || stop "the new Gate is in place, and its service would not start again. See: journalctl -u homewarp-gate"
    say "It has been started again. Players who were connected stay connected."
  else
    say "Nothing starts the Gate on this machine by itself: stop the one that is running and start it again."
  fi
  exit 0
fi
exec "$PROGRAM" join "$@"
}

main "$@"
