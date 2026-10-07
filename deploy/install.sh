#!/bin/sh
# Homewarp at home, in one line:
#
#   curl -fsSL @RELEASES@/install.sh | sh
#
# It fetches Homewarp for this machine, checks that it is the one that was
# released, and starts it with Docker: the panel is then at port 3600 by
# default. Run again later, it puts the newest release in place of the one that
# is running and leaves everything that was made with it where it is.
#
# On a terminal it asks first, once for each of the things below, and shows
# what it is going to do before it does it. Enter keeps the answer in brackets.
# A machine that has been installed on before is asked with what it has now as
# the answers. Where there is no terminal to ask on, or with --yes
# (sh -s -- --yes), it asks nothing and goes on with the answers it has.
#
# It needs Docker with Compose, which it does not install: what a machine runs
# its containers with is its owner's choice.
#
# Nothing here is trusted for where it came from. The list of checksums is
# signed, with a key whose public half is written into this script, and the
# program is held against that list before it is run.
#
# What can be said ahead of the questions, each with what it is when nothing is
# said (a question that was answered this way is not asked):
#   HOMEWARP_DIR=/opt/homewarp   where it is kept, with all that is made in it
#   HOMEWARP_PORT=3600           the panel
#   HOMEWARP_SFTP_PORT=2022      servers' files, over SFTP
#   HOMEWARP_TLS_PORT=8443       the panel over TLS, once it is given a name
#   HOMEWARP_NAME=homewarp       what its containers are called
#   HOMEWARP_YES=1               ask nothing, as --yes does
#
# A release fills in what is between the @ signs (scripts/dev.sh release).
set -eu

# All of it is in one function that is called on the last line, so that nothing
# runs until the whole script has arrived: one that was cut off on the way does
# nothing at all, and nothing in it can read the rest of it as its own input.
main() {
RELEASES="${HOMEWARP_RELEASES:-@RELEASES@}"
VERSION="@VERSION@"
DIR="${HOMEWARP_DIR:-/opt/homewarp}"
PORT="${HOMEWARP_PORT:-3600}"
SFTP_PORT="${HOMEWARP_SFTP_PORT:-2022}"
TLS_PORT="${HOMEWARP_TLS_PORT:-8443}"
NAME="${HOMEWARP_NAME:-homewarp}"

# Colour, where there is a terminal to show it on and nothing has asked for
# none. The violet is the name's, and the panel's accent; the rest are the
# panel's colours for what went well, what to look at and what did not.
plain='' bold='' accent='' dim='' good='' warn='' bad=''
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
      good=$(printf '\033[38;2;61;214;140m') warn=$(printf '\033[38;2;245;183;61m')
      bad=$(printf '\033[38;2;255;99;105m')
      ;;
    # One violet for all of the name: it holds until it is taken off.
    *)
      c1=$(printf '\033[38;5;141m') accent=$c1 dim=$(printf '\033[38;5;245m')
      good=$(printf '\033[38;5;78m') warn=$(printf '\033[38;5;214m') bad=$(printf '\033[38;5;203m')
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
step() { printf '  %s›%s %s\n' "$accent" "$plain" "$*"; }
done_() { printf '  %s✓%s %s\n' "$good" "$plain" "$*"; }
heading() { printf '  %s%s%s%s\n' "$bold" "$accent" "$*" "$plain"; }
# A name and what it is, as the summary shows them.
fact() { printf '    %s%-16s%s %s\n' "$dim" "$1" "$plain" "$2"; }

# The name, in its own colour, with the mark of the panel's icon before what
# it is for.
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
  printf '\n  %s◆%s %sGame servers at home, friends in through a small VPS.%s\n\n' \
    "$accent" "$plain" "$dim" "$plain"
}
banner

ASSUME_YES=0
[ "${HOMEWARP_YES:-}" != 1 ] || ASSUME_YES=1
for arg in "$@"; do
  case "$arg" in
    -y | --yes) ASSUME_YES=1 ;;
    *) stop "$arg is not something this takes. The one thing it takes is --yes, which asks nothing." ;;
  esac
done

# What a question is answered on: the terminal itself, because what arrives on
# the script's own input is the script. HOMEWARP_TTY names another place to read
# the answers from, which is how the questions are tried without a terminal.
INTERACTIVE=0
if [ "$ASSUME_YES" != 1 ] && ( : < "${HOMEWARP_TTY:-/dev/tty}" ) 2>/dev/null; then
  exec 3< "${HOMEWARP_TTY:-/dev/tty}"
  INTERACTIVE=1
fi

# Each of these says whether an answer will do, and if not, why in $why.
why=''
good_dir() {
  case "$1" in
    /*) ;;
    *) why="a folder is written in full, starting with /."; return 1 ;;
  esac
  case "$1" in
    *[!A-Za-z0-9_./+-]*) why="letters, digits and . _ + - / only: Docker is given this path unquoted."; return 1 ;;
  esac
  case "$1" in
    *[!/]*) ;;
    *) why="that is the whole disk: choose a folder in it."; return 1 ;;
  esac
}
# A folder without the / that may have been left on the end of it.
tidy() {
  while [ "${DIR%/}" != "$DIR" ] && [ "$DIR" != / ]; do DIR=${DIR%/}; done
}
can_write() {
  place=$1
  while [ ! -d "$place" ]; do place=$(dirname "$place"); done
  [ -w "$place" ] || { why="this account cannot write in $place. Run this as root, or choose another folder."; return 1; }
}
good_port() {
  case "$1" in
    '' | *[!0-9]* | ??????*) why="a port is a number from 1 to 65535."; return 1 ;;
  esac
  if [ "$1" -lt 1 ] || [ "$1" -gt 65535 ]; then
    why="a port is a number from 1 to 65535."
    return 1
  fi
}
good_name() {
  case "$1" in
    '' | [!a-z0-9]* | *[!a-z0-9_-]*) why="lower-case letters, digits, - and _, starting with a letter or a digit."; return 1 ;;
  esac
}
# The three ports are three doors, and no two of them can be the same.
apart() { # port, which of the three it is for
  for other in PORT SFTP_PORT TLS_PORT; do
    [ "$other" != "$2" ] || continue
    eval "there=\$$other"
    if [ "$there" = "$1" ]; then
      why="$1 is already one of the other two ports."
      return 1
    fi
  done
}
listening() {
  command -v ss >/dev/null 2>&1 || return 1
  ss -ltn 2>/dev/null | awk 'NR > 1 { print $4 }' | grep -q ":$1\$"
}
# A port that will do to be asked for: a port, not another door's, and not one
# that something else on this machine already has (the one this install has
# now is its own, and does not count).
open_port() { # port, which of the three it is for
  good_port "$1" && apart "$1" "$2" || return 1
  eval "own=\${OLD_$2:-}"
  if [ "$1" != "$own" ] && listening "$1"; then
    why="something on this machine is already listening on $1."
    return 1
  fi
}
folder() { good_dir "$1" && can_write "$1"; }

# One question. The answer goes into the variable named first, unless the
# environment variable named second said it already, or there is nobody to ask.
# Then what the question is, what is good to know before answering it, and the
# check an answer has to pass.
ask() {
  [ "$INTERACTIVE" = 1 ] || return 0
  eval "said=\${$2:-}"
  [ -z "$said" ] || return 0
  eval "fallback=\$$1"
  printf '  %s%s%s\n' "$dim" "$4" "$plain"
  while :; do
    printf '  %s?%s %s%s%s %s[%s]%s ' "$accent" "$plain" "$bold" "$3" "$plain" "$dim" "$fallback" "$plain"
    IFS= read -r answer <&3 || {
      echo
      stop "there was nobody to answer. Nothing was installed."
    }
    [ -n "$answer" ] || answer=$fallback
    if "$5" "$answer" "$1"; then break; fi
    printf '    %s%s%s\n' "$warn" "$why" "$plain"
  done
  eval "$1=\$answer"
  echo
}

# What an install made before is asked with as its answers: the folder is
# where it is, and the rest is what its compose.yml says.
keep() { # variable, environment variable, check
  eval "was=\$OLD_$1"
  eval "said=\${$2:-}"
  if [ -z "$said" ] && [ -n "$was" ] && "$3" "$was" "$1"; then eval "$1=\$was"; fi
}
remember() {
  OLD_PORT='' OLD_SFTP_PORT='' OLD_TLS_PORT='' OLD_NAME=''
  [ -r "$DIR/compose.yml" ] || return 0
  OLD_PORT=$(sed -n 's|.*"0\.0\.0\.0:\([0-9]*\)", "/run/homewarp/panel\.sock".*|\1|p' "$DIR/compose.yml" | head -1)
  OLD_SFTP_PORT=$(sed -n 's|^ *HOMEWARP_SFTP_PORT: "\([0-9]*\)".*|\1|p' "$DIR/compose.yml" | head -1)
  OLD_TLS_PORT=$(sed -n 's|^ *HOMEWARP_TLS_PORT: "\([0-9]*\)".*|\1|p' "$DIR/compose.yml" | head -1)
  OLD_NAME=$(sed -n 's|^name: \([a-z0-9_-]*\)$|\1|p' "$DIR/compose.yml" | head -1)
  keep PORT HOMEWARP_PORT good_port
  keep SFTP_PORT HOMEWARP_SFTP_PORT good_port
  keep TLS_PORT HOMEWARP_TLS_PORT good_port
  keep NAME HOMEWARP_NAME good_name
}

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

# A Homewarp that is running already is updated where it is, not made again
# somewhere else beside it to fight it for its ports. Its folder and its name
# are then not asked: a changed one would be a second Homewarp over the first's
# servers, which docs/moving.md is about, and not what an update does.
FOUND=0
if [ -z "${HOMEWARP_DIR:-}" ]; then
  here=$(docker inspect --format '{{ index .Config.Labels "com.docker.compose.project.working_dir" }}' "$NAME" 2>/dev/null) || here=''
  if [ -n "$here" ] && good_dir "$here"; then
    DIR=$here
    FOUND=1
  fi
fi
tidy
[ ! -r "$DIR/compose.yml" ] || FOUND=1

if [ "$INTERACTIVE" = 1 ]; then
  heading "Setup"
  say "  Enter keeps the answer in brackets. Nothing is done until the last question."
  echo
  OLD_PORT='' OLD_SFTP_PORT='' OLD_TLS_PORT='' OLD_NAME=''
  if [ "$FOUND" = 1 ]; then
    step "Homewarp is installed in $DIR already, and is updated there: its folder and its"
    say "    name stay as they are (docs/moving.md is about moving it). The ports are asked"
    say "    with what it has now."
    echo
  else
    ask DIR HOMEWARP_DIR "Where should Homewarp keep its files?" \
      "Everything made with it goes inside: the database, servers' worlds and backups. Pick a disk with room." folder
    tidy
  fi
  remember
  ask PORT HOMEWARP_PORT "Which port should the panel be on?" \
    "The panel is a web page: http://<this machine>:<port>. It is the one to open in a browser." open_port
  ask SFTP_PORT HOMEWARP_SFTP_PORT "Which port should SFTP be on?" \
    "Servers' files are reached over SFTP here, for uploading worlds and mods." open_port
  ask TLS_PORT HOMEWARP_TLS_PORT "Which port should the panel's TLS be on?" \
    "Used once the panel is given a name, to serve it over https." open_port
  if [ "$FOUND" != 1 ]; then
    ask NAME HOMEWARP_NAME "What should its containers be called?" \
      "Docker's name for them, as in docker logs <name>." good_name
  fi
else
  remember
fi

# What is to be done is checked once more, whoever said it. Nobody is asked
# here: whatever was said ahead of the questions, or is left, has to do.
good_dir "$DIR" || stop "$DIR cannot be where Homewarp is kept: $why"
good_name "$NAME" || stop "$NAME cannot be what its containers are called: $why"
for which in PORT SFTP_PORT TLS_PORT; do
  eval "chosen=\$$which"
  good_port "$chosen" || stop "$chosen cannot be the port of $which: $why"
  apart "$chosen" "$which" || stop "$why"
done

if [ "$INTERACTIVE" = 1 ]; then
  heading "Ready"
  fact 'Version' "$VERSION"
  fact 'Folder' "$DIR"
  fact 'Panel' "port $PORT"
  fact 'SFTP' "port $SFTP_PORT"
  fact 'TLS' "port $TLS_PORT"
  fact 'Containers' "$NAME"
  echo
  printf '  %s?%s %sInstall it with these?%s %s[Y/n]%s ' "$accent" "$plain" "$bold" "$plain" "$dim" "$plain"
  IFS= read -r answer <&3 || answer=n
  echo
  case "$answer" in
    '' | [Yy]*) ;;
    *)
      say "Homewarp: nothing was installed, as asked."
      exit 0
      ;;
  esac
else
  say "Homewarp $VERSION: in $DIR, the panel on port $PORT, SFTP on $SFTP_PORT, TLS on $TLS_PORT."
fi
mkdir -p "$DIR/image" "$DIR/data" 2>/dev/null || stop "$DIR cannot be written to. Run this as root, or name another place with HOMEWARP_DIR."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fetch() {
  curl -fsSL --retry 2 -o "$work/$1" "$RELEASES/$VERSION/$1" || stop "$1 could not be fetched from $RELEASES."
}
file="homewarp-$target"
step "Fetching Homewarp $VERSION for $target."
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
# And the list is this version's own: every release's list is signed, and
# names the same files, so the version is among what is listed.
named=$(printf '%s\n' "$VERSION" | sha256sum | cut -d' ' -f1)
[ "${HOMEWARP_UNSIGNED:-}" = 1 ] || grep -q "^$named [ *]VERSION\$" "$work/SHA256SUMS" ||
  stop "the list of checksums is not that of Homewarp $VERSION. Nothing was installed."
# Then the program against the list.
(cd "$work" && grep " $file\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1) ||
  stop "$file is not the file that was released. Nothing was installed."
done_ "It is the release: signed with Homewarp's key, and the checksum matches."

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
# again writes it again, with the answers it was given then: what is to be
# different is said to the installer.
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

step "Homewarp $VERSION: starting it with Docker. The first time takes a minute."
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
echo
printf '  %s✓%s Homewarp %s is running. Open %s%shttp://%s:%s%s\n' \
  "$good" "$plain" "$VERSION" "$bold" "$accent" "${address:-this machine}" "$PORT" "$plain"
code=$(docker logs "$NAME" 2>&1 | grep -o 'setup code: .*' | tail -1 | cut -d' ' -f3)
if [ -n "$code" ]; then
  printf '    There is no account yet. Create the first one there with this setup code: %s%s%s%s\n' \
    "$bold" "$accent" "$code" "$plain"
fi
echo
}

main "$@"
