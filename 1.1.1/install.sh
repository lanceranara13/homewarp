#!/bin/sh
# Homewarp at home, in one line:
#
#   curl -fsSL https://lanceranara13.github.io/homewarp/install.sh | sh
#
# It fetches Homewarp for this machine, checks that it is the one that was
# released, and starts it with Docker: the panel is then at port 3600. Run
# again later, it puts the newest release in place of the one that is running
# and leaves everything that was made with it where it is.
#
# It needs Docker with Compose, which it does not install: what a machine runs
# its containers with is its owner's choice.
#
# Nothing here is trusted for where it came from. The list of checksums is
# signed, with a key whose public half is written into this script, and the
# program is held against that list before it is run.
#
# What can be said otherwise, each with what it is when nothing is said:
#   HOMEWARP_DIR=/opt/homewarp   where it is kept, with all that is made in it
#   HOMEWARP_PORT=3600           the panel
#   HOMEWARP_SFTP_PORT=2022      servers' files, over SFTP
#   HOMEWARP_TLS_PORT=8443       the panel over TLS, once it is given a name
#   HOMEWARP_NAME=homewarp       what its containers are called
#
# A release fills in what is between the @ signs (scripts/dev.sh release).
set -eu

# All of it is in one function that is called on the last line, so that nothing
# runs until the whole script has arrived: one that was cut off on the way does
# nothing at all, and nothing in it can read the rest of it as its own input.
main() {
RELEASES="${HOMEWARP_RELEASES:-https://lanceranara13.github.io/homewarp}"
VERSION="1.1.1"
DIR="${HOMEWARP_DIR:-/opt/homewarp}"
PORT="${HOMEWARP_PORT:-3600}"
SFTP_PORT="${HOMEWARP_SFTP_PORT:-2022}"
TLS_PORT="${HOMEWARP_TLS_PORT:-8443}"
NAME="${HOMEWARP_NAME:-homewarp}"

say() { printf '%s\n' "$*"; }
stop() {
  printf 'Homewarp: %s\n' "$*" >&2
  exit 1
}
# The name, in its own colour where there is a terminal to show it on and
# nothing has asked for none.
banner() {
  c1='' c2='' c3='' c4='' c5='' c6='' c7='' c8='' plain=''
  if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
    plain=$(printf '\033[0m')
    case "${COLORTERM:-}" in
      # A shade to a letter, from the deep violet to the pale.
      truecolor | 24bit)
        c1=$(printf '\033[38;2;110;86;248m') c2=$(printf '\033[38;2;122;100;249m')
        c3=$(printf '\033[38;2;135;114;250m') c4=$(printf '\033[38;2;147;128;251m')
        c5=$(printf '\033[38;2;159;142;252m') c6=$(printf '\033[38;2;171;156;253m')
        c7=$(printf '\033[38;2;184;170;254m') c8=$(printf '\033[38;2;196;184;255m')
        ;;
      # One violet for all of it: it holds until it is taken off.
      *) c1=$(printf '\033[38;5;141m') ;;
    esac
  fi
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
  echo
}
banner

case "$(uname -m)" in
  x86_64 | amd64) target=x86_64 ;;
  aarch64 | arm64) target=arm64 ;;
  *) stop "there is no Homewarp for this kind of processor ($(uname -m))." ;;
esac
for tool in curl sha256sum openssl docker; do
  command -v "$tool" >/dev/null 2>&1 || stop "this needs $tool, which is not installed here."
done
docker compose version >/dev/null 2>&1 || stop "this needs Docker's Compose plugin (docker compose), which is not installed here."
docker info >/dev/null 2>&1 || stop "Docker does not answer. Is it running, and may this account use it?"
# The signature is one that openssl has been able to check since 3.0.
case "$(openssl version | cut -d' ' -f2)" in
  0.* | 1.*) [ "${HOMEWARP_UNSIGNED:-}" = 1 ] || stop "the openssl here is too old to check the release's signature (it takes 3.0). Nothing was installed." ;;
esac
mkdir -p "$DIR/image" "$DIR/data" 2>/dev/null || stop "$DIR cannot be written to. Run this as root, or name another place with HOMEWARP_DIR."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() {
  curl -fsSL --retry 2 -o "$work/$1" "$RELEASES/$VERSION/$1" || stop "$1 could not be fetched from $RELEASES."
}
file="homewarp-$target"
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
# Then the program against the list.
(cd "$work" && grep " $file\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1) ||
  stop "$file is not the file that was released. Nothing was installed."

# The image is made here, from the program and the two tools it drives the
# network with. No registry is asked for Homewarp itself.
cp "$work/$file" "$DIR/image/homewarp"
chmod 755 "$DIR/image/homewarp"
cat > "$DIR/image/Dockerfile" <<'DOCKERFILE'
FROM alpine:3.20
RUN apk add --no-cache ca-certificates iproute2 nftables
COPY homewarp /usr/local/bin/homewarp
ENTRYPOINT ["/usr/local/bin/homewarp"]
DOCKERFILE

# Core runs each server as a container of its own, so it is given the Docker
# socket, and it keeps its end of the tunnel in the machine's own network. What
# it is reached by are doors: the same program once more, in containers of the
# ordinary kind, each with one port published and one socket to pass it to.
door() { # name, port inside and out, socket
  cat <<DOOR
  $1:
    image: homewarp:$VERSION
    container_name: $NAME-$1
    depends_on: [core]
    restart: unless-stopped
    read_only: true
    cap_drop: [ALL]
    security_opt: ["no-new-privileges:true"]
    command: ["door", "0.0.0.0:$2", "/run/homewarp/$3"]
    ports: ["$2:$2"]
    volumes: ["$DIR/data/run:/run/homewarp"]
DOOR
}
{
  cat <<COMPOSE
# Written by Homewarp's installer for version $VERSION. Running the installer
# again writes it again: what is to be different is said to the installer.
name: $NAME
services:
  core:
    build: ./image
    image: homewarp:$VERSION
    container_name: $NAME
    restart: unless-stopped
    read_only: true
    cap_drop: [ALL]
    cap_add: [CHOWN, DAC_OVERRIDE, NET_ADMIN]
    security_opt: ["no-new-privileges:true"]
    network_mode: host
    environment:
      HOMEWARP_DATA: "$DIR/data"
      HOMEWARP_LISTEN: "unix:$DIR/data/run/panel.sock"
      HOMEWARP_SFTP: "unix:$DIR/data/run/sftp.sock"
      HOMEWARP_SFTP_PORT: "$SFTP_PORT"
      HOMEWARP_TLS: "unix:$DIR/data/run/tls.sock"
      HOMEWARP_TLS_PORT: "$TLS_PORT"
      # Where a VPS is told to fetch its Gate from: where this came from.
      HOMEWARP_RELEASES: "$RELEASES"
    volumes:
      - "$DIR/data:$DIR/data"
      - "/var/run/docker.sock:/var/run/docker.sock"
COMPOSE
  door door "$PORT" panel.sock
  door sftp-door "$SFTP_PORT" sftp.sock
  door tls-door "$TLS_PORT" tls.sock
} > "$DIR/compose.yml"

say "Homewarp $VERSION: starting it with Docker. The first time takes a minute."
(cd "$DIR" && docker compose up -d --build --quiet-pull >/dev/null 2>"$work/compose.log") || {
  cat "$work/compose.log" >&2
  stop "Docker could not start it. What it said is above."
}
for _ in $(seq 60); do
  curl -fsS -o /dev/null "http://127.0.0.1:$PORT/api/v1/health" 2>/dev/null && break
  sleep 1
done
curl -fsS -o /dev/null "http://127.0.0.1:$PORT/api/v1/health" 2>/dev/null ||
  stop "it was started and does not answer on port $PORT. See: docker logs $NAME"

# The address this machine reaches the internet from, which is the one the
# rest of the home network knows it by. The first address it has may be a
# bridge of Docker's.
address=$(ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p' | head -1)
[ -n "$address" ] || address=$(hostname -I 2>/dev/null | cut -d' ' -f1)
say "Homewarp $VERSION is running. Open http://${address:-this machine}:$PORT"
code=$(docker logs "$NAME" 2>&1 | grep -o 'setup code: .*' | tail -1 | cut -d' ' -f3)
if [ -n "$code" ]; then
  say "There is no account yet. Create the first one there with this setup code: $code"
fi
}

main "$@"
