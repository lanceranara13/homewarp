#!/usr/bin/env bash
# The lab: a VPS without a VPS (PLAN.md §10).
#
# Runs on the homelab; `scripts/dev.sh lab` syncs the tree and starts it there.
# Every party is a container on two internal Docker networks, so the homelab's
# own networking is not touched:
#
#   client 203.0.113.50  ─┐
#   gate   203.0.113.10  ─┼─ "internet"
#   pebble 203.0.113.30  ─┤
#   router 203.0.113.20  ─┘
#   router 192.168.50.2  ─┐
#   home   192.168.50.20 ─┼─ "home LAN"
#   nas    192.168.50.30 ─┘
#
# The router is the home's line: it hides the LAN behind its one public address,
# as a home's router does. So home dials out from behind NAT, and a connection
# home makes to the VPS comes back from an address that is not home's own.
#
# home is docker-in-docker. A real Docker daemon publishes the game container's
# ports there, which is the part real-vps-spike.sh had to fake with a namespace.
#
# Both programs are the real ones, built by `scripts/dev.sh lab`: Core in home,
# and the Gate on the simulated VPS, enrolled with a join token as a VPS is.
#
# pebble is a certificate authority of the lab's own. The panel is given the
# name panel.lab, which leads to the gate as a real name leads to a VPS, and
# Core asks pebble for the certificate as it would ask Let's Encrypt.
#
# Usage: run.sh all | up | test | bench | down | clean
#        HOME_FW=nftables run.sh all     # Docker's nftables firewall backend at home
#        GATE_FW=firewalld run.sh all    # a VPS with firewalld in front of everything on it
set -euo pipefail
cd "$(dirname "$0")"

export HOME_FW=${HOME_FW:-iptables}
GATE_FW=${GATE_FW:-none}
# The VPS is then of an image that has firewalld (firewalld.Dockerfile). firewalld is a
# program of some tens of megabytes itself, and no part of what the Gate is held to.
[ "$GATE_FW" = firewalld ] && export GATE_IMAGE=homewarp-lab-firewalld GATE_DOCKERFILE=firewalld.Dockerfile GATE_MEM=${GATE_MEM:-256m}
GATE_IP=203.0.113.10 GATE2_IP=203.0.113.11 HOME_IP=203.0.113.20 HOME_IP_NEXT=203.0.113.21 CLIENT_IP=203.0.113.50
PEBBLE_IP=203.0.113.30
RELEASES_IP=203.0.113.40  # where releases are fetched from, as a web server on the internet
NAME=panel.lab  # the panel's name, which leads to the gate
TLS=8443        # the panel over TLS: its door at home, and the port the gate forwards for it
ROUTER_LAN_IP=192.168.50.2 HOME_LAN_IP=192.168.50.20 NAS_IP=192.168.50.30
GATE_TUN=10.213.77.1 HOME_TUN=10.213.77.2 GAME_IP=10.213.80.2
API_PORT=4857   # where the Gate answers home, on its tunnel address only
PORT=25565      # the game's port, tcp + udp, published by Docker
IPERF=25566     # iperf3 in a server of its own, published by Docker
CLOSED=25567    # open in the game container, not published
VOICE=25568     # a further port of the game's, for udp alone
MC=25570        # a server that speaks Minecraft, and is put to sleep
SECOND=25571    # a server that is reached through a second VPS
SVC=2222        # listen.sh on home, nas and client

dc()   { docker compose -p homewarp-lab --progress quiet "$@"; }
in_()  { dc exec -T "$1" sh -s; }
vars() { local v; for v in "$@"; do printf "%s='%s'\n" "$v" "${!v}"; done; }
# One field of the JSON on standard input.
field() { python3 -c 'import json, sys; print(json.load(sys.stdin).get(sys.argv[1]))' "$1"; }

# Asks Core, on home, as the panel's page would.
core() {  # method, path, [json]
  dc exec -T home curl -fsS -m 30 -b /run/hw/jar -c /run/hw/jar -X "$1" \
    -H 'Content-Type: application/json' ${3:+-d "$3"} "http://127.0.0.1:3600/api/v1$2"
}

# A lab egg, as the JSON Core's import takes: a server in the lab's own image.
egg() {  # name, startup command, [the config files it sets up, as an egg writes them]
  python3 - "$1" "$2" "${3:-{\}}" <<'PY'
import json, sys
print(json.dumps({"egg": json.dumps({
    "meta": {"version": "PTDL_v2"}, "name": sys.argv[1], "description": "For the lab.",
    "docker_images": {"lab": "homewarp-lab-yolk"}, "startup": sys.argv[2],
    "config": {"files": sys.argv[3], "startup": "{\"done\": \"ready\"}", "stop": "^C"},
    "scripts": {"installation": {"script": None, "container": "homewarp-lab-yolk", "entrypoint": "sh"}},
    "variables": []})}))
PY
}

# What the Gate now asks of whoever talks to it, and what it says of itself when
# home asks through the tunnel.
gate_token() { dc exec -T gate cat /run/hw/config.json | field token; }
gate_says()  {
  dc exec -T home curl -s -m 5 -H "Authorization: Bearer $(gate_token)" "http://$GATE_TUN:$API_PORT/v1/status"
}

# Core's view of a VPS, the first unless another is named by its address: one
# field of it, and waiting for a field to be a value.
gate_is() {  # field, [address]
  core GET /gates | python3 -c 'import json, sys
gates = [gate for gate in json.load(sys.stdin)["gates"] if gate["address"] == sys.argv[2]]
print(gates[0].get(sys.argv[1]) if gates else {"state": "none"}.get(sys.argv[1]))' "$1" "${2:-$GATE_IP}"
}
# Has Core begin to connect a VPS, and says the one command to run there.
connect() {  # address, [name]
  core POST /gates "{\"address\":\"$1\"${2:+,\"name\":\"$2\"}}" | python3 -c 'import json, sys
print(next(gate["command"] for gate in json.load(sys.stdin)["gates"] if gate["state"] == "waiting"))'
}
until_gate() {  # field, value, seconds, [address]
  for _ in $(seq "$3"); do
    [ "$(gate_is "$1" "${4:-$GATE_IP}" 2>/dev/null)" = "$2" ] && return 0
    sleep 1
  done
  return 1
}

# Has Core find out again how players' addresses arrive, and says what it found.
check_again() { core POST "/gates/$(gate_is id)/check" >/dev/null; gate_is player_addresses; }

# Starts the two programs, as their services would.
start_gate() { dc exec -d gate sh -c 'homewarp-gate run --dir /run/hw >>/run/hw/log 2>&1'; }
start_core() {
  # On every address, as the deployment listens: the rules are what keep the Gate from it.
  # And told which image to listen from when it probes the tunnel: here it does not run from one.
  # Over TLS it is served to a door, as the deployment serves it, and its certificate is asked of pebble.
  dc exec -d home sh -c "HOMEWARP_DATA=/run/hw/core HOMEWARP_LISTEN=0.0.0.0:3600 HOMEWARP_IMAGE=homewarp-lab-core \
    HOMEWARP_TLS=unix:/run/hw/core/run/tls.sock HOMEWARP_TLS_PORT=$TLS \
    HOMEWARP_ACME=https://pebble:14000/dir HOMEWARP_ACME_ROOT=/run/hw/pebble.pem \
    HOMEWARP_RELEASES=http://$RELEASES_IP HOMEWARP_RELEASE_KEY=\$(cat /run/hw/release-key 2>/dev/null) \
    homewarp >>/run/hw/core.log 2>&1"
  for _ in $(seq 40); do core GET /health >/dev/null 2>&1 && break; sleep 0.5; done
}

cmd_up() {
  local code command token root release version

  echo "== containers"
  dc build
  dc up -d
  for _ in $(seq 60); do dc exec -T home docker info >/dev/null 2>&1 && break; sleep 1; done
  docker save homewarp-lab-node | dc exec -T home docker load -q >/dev/null
  docker build -q -t homewarp-lab-yolk -f yolk.Dockerfile . >/dev/null
  docker save homewarp-lab-yolk | dc exec -T home docker load -q >/dev/null
  docker build -q -t homewarp-lab-core -f core.Dockerfile ../deploy/out >/dev/null
  docker save homewarp-lab-core | dc exec -T home docker load -q >/dev/null

  echo "== router: the home's line, with the LAN hidden behind its one address"
  { vars HOME_IP HOME_LAN_IP PORT IPERF; cat <<'EOF'; } | in_ router
set -eu
WAN=$(ip -o -4 addr show | awk -v ip="$HOME_IP" 'index($4, ip "/") == 1 { print $2 }')
nft -f - <<NFT
table ip lab
delete table ip lab
table ip lab {
  chain out { type nat hook postrouting priority srcnat; oifname "$WAN" masquerade; }
  # A port forward, as a home without a VPS would have: for the bench's comparison.
  chain in {
    type nat hook prerouting priority dstnat;
    iifname "$WAN" tcp dport { $PORT, $IPERF } dnat to $HOME_LAN_IP
    iifname "$WAN" udp dport { $PORT, $IPERF } dnat to $HOME_LAN_IP
  }
}
NFT
EOF

  echo "== home: Core itself, which makes the servers and its end of the tunnel"
  dc cp ../deploy/out/homewarp-static home:/usr/local/bin/homewarp
  { vars SVC ROUTER_LAN_IP GATE_IP PEBBLE_IP NAME TLS; cat <<'EOF'; } | in_ home
set -eu
ip route replace default via "$ROUTER_LAN_IP"
pkill homewarp 2>/dev/null || true
docker ps -aq --filter label=homewarp.server | xargs -r docker rm -f >/dev/null
docker rm -f panel-door >/dev/null 2>&1 || true
ip link del homewarp0 2>/dev/null || true
rm -rf /run/hw && mkdir -p /run/hw/core/run
# The panel's name leads to the VPS, and the authority has a name of its own.
grep -q " $NAME\$" /etc/hosts || printf '%s %s\n%s pebble\n' "$GATE_IP" "$NAME" "$PEBBLE_IP" >> /etc/hosts
# The door of the panel over TLS: Core's own program in a container, its port
# published as the deployment publishes it, and started again if it ends, as
# the deployment's is: the tests stop every program called homewarp, this too.
docker run -d --name panel-door --restart always -p "$TLS:$TLS" -v /run/hw/core/run:/run/homewarp homewarp-lab-core \
  /usr/local/bin/homewarp door "0.0.0.0:$TLS" /run/homewarp/tls.sock >/dev/null
SVC="$SVC" /lab/listen.sh </dev/null >/dev/null 2>&1 &
# The worst case for the return path: a host that filters reverse paths strictly.
sysctl -qw net.ipv4.conf.all.rp_filter=1 net.ipv4.conf.default.rp_filter=1
EOF
  # The root that pebble's own address is trusted by, which Core is told of.
  root=$(mktemp)
  dc cp pebble:/test/certs/pebble.minica.pem "$root" >/dev/null
  dc cp "$root" home:/run/hw/pebble.pem >/dev/null
  rm -f "$root"
  # The key the lab's release is signed with, made here and thrown away with
  # the lab. Core is told its public half, as a released Core is built with it.
  release=$(mktemp -d)
  openssl genpkey -algorithm ed25519 -out "$release/key.pem"
  openssl pkey -in "$release/key.pem" -pubout -outform DER | tail -c 32 | base64 | dc exec -T home sh -c 'cat > /run/hw/release-key'
  start_core
  code=$(dc exec -T home sh -c "grep -o 'setup code: .*' /run/hw/core.log | cut -d' ' -f3")
  core POST /setup "{\"code\":\"$code\",\"username\":\"lab\",\"password\":\"only-in-the-lab\"}" >/dev/null
  core POST /templates "$(egg 'Lab game' "PORT={{SERVER_PORT}} IPERF=5999 CLOSED=$CLOSED /lab/game.sh")" >/dev/null
  core POST /templates "$(egg 'Lab iperf' 'exec iperf3 -s -p {{SERVER_PORT}}')" >/dev/null
  core POST /servers "{\"name\":\"game\",\"template_id\":1,\"memory_mb\":256,\"port\":$PORT,\"ports\":[{\"port\":$VOICE,\"protocol\":\"udp\"}]}" >/dev/null
  for _ in $(seq 30); do [ -n "$(dc exec -T home docker ps -q --filter publish=$PORT)" ] && break; sleep 1; done
  core POST /servers "{\"name\":\"iperf\",\"template_id\":2,\"memory_mb\":256,\"port\":$IPERF}" >/dev/null
  for _ in $(seq 30); do [ -n "$(dc exec -T home docker ps -q --filter publish=$IPERF)" ] && break; sleep 1; done
  dc exec -T home docker ps --format '   {{.Names}}  {{.Ports}}' | cut -c1-130

  echo "== a release: the programs, a signed list of them, and the install scripts"
  # Made as a real one is (scripts/release.sh), signed with the lab's own key,
  # and put where the lab's internet fetches releases from. The key is kept
  # beside that web server, and not in what it serves: the tests sign a later
  # list of releases with it.
  version=$(../deploy/out/homewarp-gate version | cut -d' ' -f2)
  # A tree that is numbered as a beta is released as one. A beta's two
  # scripts are not put at the top, where the one line of the README fetches
  # them; the lab's one release is the newest there is whichever it is, and
  # the VPS is connected with the line that fetches from there.
  case "$version" in *-*) channel=beta ;; *) channel=stable ;; esac
  sh ../scripts/release.sh ../deploy/out "$release/served" "$version" "http://$RELEASES_IP" "$release/key.pem" "$channel" | sed 's/^/   /'
  cp "$release/served/$version/install.sh" "$release/served/$version/install-gate.sh" "$release/served/"
  dc exec -T releases sh -c 'rm -rf /releases/* /tmp/released'
  dc cp "$release/served/." releases:/releases >/dev/null
  dc cp "$release/key.pem" releases:/tmp/lab-key.pem >/dev/null
  rm -rf "$release"

  if [ "$GATE_FW" = firewalld ]; then
    echo "== the VPS has a firewall of its own: firewalld, as it is when first installed"
    if [ "$(dc exec -T gate firewall-cmd --state 2>/dev/null || true)" != running ]; then
      dc exec -T gate sh -c 'mkdir -p /run/dbus && { pidof dbus-daemon >/dev/null || dbus-daemon --system; }'
      dc exec -d gate firewalld --nofork --nopid
    fi
    for _ in $(seq 60); do [ "$(dc exec -T gate firewall-cmd --state 2>/dev/null || true)" = running ] && break; sleep 1; done
    echo "   firewalld $(dc exec -T gate firewall-cmd --version): $(dc exec -T gate firewall-cmd --state 2>&1 || true), and what arrives is for the zone $(dc exec -T gate firewall-cmd --get-default-zone)"
  fi

  echo "== a VPS is connected: Core makes the one command, and the VPS runs it"
  command=$(connect "$GATE_IP")
  token=${command##* }
  echo "   ${command:0:60}… (${#token} characters)"
  dc exec -T gate sh -c 'pkill homewarp-gate; ip link del homewarp0; rm -rf /run/hw /usr/local/bin/homewarp-gate; true' 2>/dev/null
  # The line the panel gave, as it is: it fetches the Gate, checks it and enrols
  # the VPS. What follows it is for the lab alone. No service in a container:
  # the lab starts the Gate itself. Nor a default route, on a network with no
  # way out: the interface is named.
  dc exec -T gate sh -c "$command --dir /run/hw --no-service --wan eth0" | sed 's/^/   /'
  printf '%s' "$token" | dc exec -T gate sh -c 'cat > /run/hw/join-token'
  start_gate
  until_gate player_addresses preserved 40 || true
  echo "   core says: state $(gate_is state), reachable $(gate_is reachable), player addresses $(gate_is player_addresses), $(gate_is latency_ms) ms"
  echo "   the gate says: $(gate_says)"
}

FAILED=0
ok()    { echo "ok    $1"; }
fail()  { echo "FAIL  $1"; FAILED=1; }
check() { if [ "$2" = "$3" ]; then ok "$1: ${2:-nothing}"; else fail "$1: got '$2', want '$3'"; fi; }
differ() { if [ -n "$2" ] && [ "$2" != "$3" ]; then ok "$1"; else fail "$1: '$2'"; fi; }

# The address the game server says a player connecting through the gate came from.
#
# Over TCP the lab's clients only listen. The stand-in servers answer and hang
# up without reading, so anything sent to them comes back as a reset, and about
# one time in a hundred that reset overtook the answer: a failure of this
# harness, measured, that looked like one of the tunnel.
seen() {  # tcp | udp
  local cmd="socat -u TCP:$GATE_IP:$PORT,connect-timeout=4 -"
  [ "$1" = udp ] && cmd="echo hi | socat -t 3 - UDP:$GATE_IP:$PORT"
  dc exec -T client sh -c "$cmd 2>/dev/null || true" | awk -v p="$1" '$1 == p { print $2 }'
}

# Waits for players to get through again, and says who the server then sees.
seen_again() {  # seconds
  local from=
  for _ in $(seq "$1"); do
    from=$(seen tcp)
    [ -n "$from" ] && break
    sleep 1
  done
  echo "$from"
}

# What answers when $1 connects to $2:$3; empty when nothing does.
reach() {
  local cmd="socat -u TCP:$2:$3,connect-timeout=3 - 2>/dev/null || true"
  if [ "$1" = game ]; then
    dc exec -T home sh -c "docker exec \$(docker ps -q --filter publish=$PORT) sh -c '$cmd'"
  else
    dc exec -T "$1" sh -c "$cmd"
  fi
}
blocked() {  # label, from, address, port
  if [ -z "$(reach "$2" "$3" "$4")" ]; then ok "$1"; else fail "$1"; fi
}

# The first line of what Core's panel answers when $1 asks it at $2; empty when it does not.
# The asking end stays open a moment: a server drops a request whose sender has hung up.
panel() {
  dc exec -T "$1" sh -c "{ printf 'GET /api/v1/health HTTP/1.0\r\n\r\n'; sleep 1; } | socat - TCP:$2:3600,connect-timeout=3 2>/dev/null | head -1 | tr -d '\r'" || true
}

# What $1 is answered when it asks for a page: the status, or 000 when nothing answers.
status() {  # party, curl arguments...
  local party=$1
  shift
  dc exec -T "$party" curl -s -m 5 -o /dev/null -w '%{http_code}' "$@" || true
}

# Asks the Gate through the tunnel, as Core does, and says the status it answered with.
gate_asked() {  # method, path, [json]
  status home -X "$1" -H "Authorization: Bearer $(gate_token)" -H 'Content-Type: application/json' \
    ${3:+-d "$3"} "http://$GATE_TUN:$API_PORT$2"
}

# The panel as a browser on the internet reaches it: by its name, at the VPS, over TLS,
# trusting the lab's authority and nobody else.
browse() {  # curl arguments..., path
  dc exec -T client curl -sS -m 8 --cacert /tmp/roots.pem --resolve "$NAME:$TLS:$GATE_IP" "${@:1:$#-1}" "https://$NAME:$TLS${!#}"
}

panel_is() { core GET /panel | field "$1"; }

# What the VPS's firewalld answers, on one line.
fw() { dc exec -T gate firewall-cmd "$@" 2>&1 | tr '\n' ' ' | sed 's/ *$//'; }

cmd_test() {
  local HOME_PUB joined carried rss size wan token answer wg_port command mc asks joins told restored version before second
  echo "home: $(dc exec -T home sh -c 'docker version --format "Docker {{.Server.Version}}"; iptables --version' | tr '\n' ' ') firewall backend $HOME_FW"

  echo "== a release is trusted for its signature, and not for where it came from"
  # A machine that has nothing of Homewarp on it yet, given a token that is none:
  # the Gate is fetched, checked and put in place, and only then is the token read.
  install() { dc exec -T client sh -c "curl -fsSL http://$RELEASES_IP/install-gate.sh | sh -s -- not-a-token 2>&1 | tail -1" || true; }
  installed() { dc exec -T client sh -c 'test -e /usr/local/bin/homewarp-gate && echo there || echo not there'; }
  check "the panel's command fetched the Gate from the release" "$(core GET /activity | python3 -c 'import json, sys; print(any(e["action"] == "gate.connect" for e in json.load(sys.stdin)))') $(dc exec -T gate /usr/local/bin/homewarp-gate version)" "True $(../deploy/out/homewarp-gate version)"
  dc exec -T client rm -f /usr/local/bin/homewarp-gate
  check "control: the Gate as it was released is installed, and only then is the token found wanting" "$(install)" "Error: That is not a join token. Copy the whole command from the panel."
  check "control: it is on the machine" "$(installed)" "there"
  dc exec -T client rm -f /usr/local/bin/homewarp-gate
  dc exec -T releases sh -c 'cp -r /releases /tmp/released && cd /releases/*/ && echo changed >> homewarp-gate-x86_64'
  check "a program that was changed after it was released is not installed" "$(install)" "Homewarp: homewarp-gate-x86_64 is not the file that was released. Nothing was installed."
  dc exec -T releases sh -c 'cd /releases/*/ && sha256sum homewarp-* > SHA256SUMS'
  check "nor is one whose list of checksums was made to fit it: that list is not the one that was signed" "$(install)" "Homewarp: the list of checksums is not signed with Homewarp's key. Nothing was installed."
  dc exec -T releases sh -c 'cd /releases/*/ && openssl genpkey -algorithm ed25519 -out /tmp/other.pem && openssl pkeyutl -sign -inkey /tmp/other.pem -rawin -in SHA256SUMS -out SHA256SUMS.sig'
  check "nor one whose list is signed with somebody else's key" "$(install)" "Homewarp: the list of checksums is not signed with Homewarp's key. Nothing was installed."
  dc exec -T releases sh -c 'cd /releases/*/ && rm SHA256SUMS.sig'
  check "nor one with no signature at all" "$(install)" "Homewarp: SHA256SUMS.sig could not be fetched from http://$RELEASES_IP."
  check "and nothing of any of them is on the machine" "$(installed)" "not there"
  # Its contents, and not the folder: the web server stands in the folder it was started in.
  dc exec -T releases sh -c 'rm -rf /releases/* && cp -r /tmp/released/. /releases/ && rm -rf /tmp/released'
  check "the release put back as it was installs again" "$(install) $(installed)" "Error: That is not a join token. Copy the whole command from the panel. there"
  dc exec -T client rm -f /usr/local/bin/homewarp-gate

  echo "== updates: Core looks where its releases are, and believes the list there for its signature"
  update_is() { core "${2:-GET}" "/update${3:-}" | field "$1"; }  # field, [method], [what follows /update]
  version=$(../deploy/out/homewarp-gate version | cut -d' ' -f2)
  # A tree that is numbered as a beta was released as one, and is found by a
  # Homewarp that takes betas: which this one does for the one question.
  case "$version" in *-*) core PUT /update '{"channel":"beta"}' >/dev/null ;; esac
  check "it finds the release it was made from, and nothing newer" "$(update_is newest POST /check) $(update_is available) $(update_is problem)" "$version False None"
  core PUT /update '{"channel":"stable"}' >/dev/null
  # The list as a later release would leave it, signed with the release's own
  # key, which the lab kept for this; or not signed anew, when that is said.
  relist() {  # the list's lines, [unsigned]
    local work
    work=$(mktemp -d)
    printf '%b' "$1" > "$work/RELEASES"
    dc cp releases:/tmp/lab-key.pem "$work/key.pem" >/dev/null
    openssl pkeyutl -sign -inkey "$work/key.pem" -rawin -in "$work/RELEASES" -out "$work/RELEASES.sig"
    dc cp "$work/RELEASES" releases:/releases/RELEASES >/dev/null
    if [ "${2:-signed}" = signed ]; then dc cp "$work/RELEASES.sig" releases:/releases/RELEASES.sig >/dev/null; fi
    rm -rf "$work"
  }
  dc exec -T releases sh -c 'cp /releases/RELEASES /tmp/RELEASES && cp /releases/RELEASES.sig /tmp/RELEASES.sig'
  relist 'stable 99.0.0\nbeta 99.1.0-beta.1\n'
  check "a newer release is seen" "$(update_is newest POST /check) $(update_is available)" "99.0.0 True"
  check "and written down, for an owner who is not at the panel" "$(core GET /activity | python3 -c 'import json, sys; print([e["detail"] for e in json.load(sys.stdin) if e["action"] == "update.available"])')" "['99.0.0']"
  check "a Homewarp that takes betas sees the beta" "$(core PUT /update '{"channel":"beta"}' | field newest)" "99.1.0-beta.1"
  check "and one that does not, does not" "$(core PUT /update '{"channel":"stable"}' | field newest)" "99.0.0"
  relist 'stable 100.0.0\n' unsigned
  check "a list that was changed after it was signed is not believed" "$(update_is problem POST /check | grep -c 'not signed') $(update_is newest)" "1 99.0.0"
  check "this Core was not set up by the installer, and says that it cannot replace itself" "$(update_is installs) $(dc exec -T home curl -s -o /dev/null -w '%{http_code}' -b /run/hw/jar -X POST -H 'Content-Type: application/json' -d '{}' http://127.0.0.1:3600/api/v1/update/install)" "False 409"
  check "and gives the line that installs the release by hand" "$(update_is line)" "curl -fsSL http://$RELEASES_IP/99.0.0/install.sh | sh"
  dc exec -T releases sh -c 'cp /tmp/RELEASES /releases/RELEASES && cp /tmp/RELEASES.sig /releases/RELEASES.sig'
  check "with the list as it was, there is nothing newer again" "$(update_is available POST /check) $(update_is problem)" "False None"
  check "a release has its two scripts beside its programs" "$(status client "http://$RELEASES_IP/$version/install.sh") $(status client "http://$RELEASES_IP/$version/install-gate.sh")" "200 200"

  echo "== enrolment: the VPS ran one command, and what that command carried no longer counts"
  check "Core has a Gate" "$(gate_is state)" "connected"
  check "which answers through the tunnel" "$(gate_is reachable)" "True"
  joined=$(dc exec -T gate cat /run/hw/join-token | python3 -c '
import base64, sys
text = sys.stdin.read().strip()
print(base64.urlsafe_b64decode(text + "=" * (-len(text) % 4)).decode())')
  carried=$(echo "$joined" | field k | dc exec -T gate wg pubkey)
  differ "the Gate has a key of its own, made on the VPS" "$(dc exec -T gate wg show homewarp0 public-key)" "$carried"
  check "and home knows it by that key" "$(dc exec -T home wg show homewarp0 peers)" "$(dc exec -T gate wg show homewarp0 public-key)"
  differ "the preshared key is a new one" "$(dc exec -T gate wg show homewarp0 preshared-keys | cut -f2)" "$(echo "$joined" | field p)"
  differ "and so is the Gate's token" "$(gate_token)" "$(echo "$joined" | field t)"
  check "the token the command carried opens nothing" "$(dc exec -T home curl -s -m 5 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(echo "$joined" | field t)" "http://$GATE_TUN:$API_PORT/v1/status")" "401"
  check "each end has the other as its one peer" "$(dc exec -T gate wg show homewarp0 peers | wc -l) $(dc exec -T home wg show homewarp0 peers | wc -l)" "1 1"
  check "the command cannot be run a second time for another Gate" "$(gate_is command)" "None"

  echo "== the Gate is updated on the VPS itself, by one line that leaves it as it is set up"
  before=$(dc exec -T gate wg show homewarp0 public-key)
  check "the line puts the release's Gate in place of the one that is there" "$(dc exec -T gate sh -c "curl -fsSL http://$RELEASES_IP/$version/install-gate.sh | sh -s -- update 2>&1" | tail -2 | head -1)" "Homewarp Gate $version is on this machine."
  check "its keys are as they were, and home still reaches it" "$(dc exec -T gate wg show homewarp0 public-key) $(gate_is reachable)" "$before True"
  check "a machine with no Gate on it is told so" "$(dc exec -T client sh -c "curl -fsSL http://$RELEASES_IP/$version/install-gate.sh | sh -s -- update 2>&1 | tail -1")" "Homewarp: there is no Gate on this machine to update. The panel gives the command that connects one."

  if [ "$GATE_FW" = firewalld ]; then
    echo "== the VPS's own firewall: firewalld was asked for what the tunnel needs, and for nothing more"
    wg_port=$(dc exec -T gate cat /run/hw/config.json | field listen_port)
    check "the tunnel's port is open where players arrive" "$(fw --zone=public --list-ports)" "$wg_port/udp"
    check "the tunnel's interface is in a zone of the Gate's own" "$(fw --get-zone-of-interface=homewarp0)" "homewarp"
    check "which lets in the Gate's API and nothing else" "$(fw --zone=homewarp --list-ports), $(fw --zone=homewarp --list-services)" "$API_PORT/tcp, "
    check "and all of it is kept for the next time the VPS starts" "$(fw --permanent --zone=homewarp --list-ports) $(fw --permanent --zone=public --list-ports)" "$API_PORT/tcp $wg_port/udp"
    dc exec -d gate /lab/listen.sh
    sleep 1
    check "control: something else on the VPS listens" "$(reach gate 127.0.0.1 "$SVC")" "reached 127.0.0.1"
    blocked "and firewalld keeps the internet from it" client "$GATE_IP" "$SVC"
    blocked "and home too, through the tunnel" home "$GATE_TUN" "$SVC"
    dc exec -T gate sh -c "pkill -f 'TCP4-LISTEN:$SVC'; true"

    echo "== a certificate, while the VPS's firewall shuts port 80"
    core PUT /panel "{\"name\":\"$NAME\",\"agreed\":true}" >/dev/null
    for _ in $(seq 90); do [ "$(panel_is state)" = failed ] && break; sleep 1; done
    check "there is none, and Core says what to open" "$(panel_is state) $(panel_is problem | grep -c 'firewall-cmd --permanent --add-service=http')" "failed 1"
    echo "   core says: $(panel_is problem)"
    core PUT /panel '{"name":null}' >/dev/null
    # As the VPS's owner then does, for that and for what else the VPS serves:
    # the sections below have it listen on these, as a web server of its own would.
    dc exec -T gate sh -c 'firewall-cmd -q --permanent --add-service=http && firewall-cmd -q --permanent --add-port=2301-2303/tcp && firewall-cmd -q --reload'
  fi

  echo "== the self-probe: Core connected to the VPS from home, and saw how that arrived"
  check "players' addresses, as Core found them" "$(gate_is player_addresses)" "preserved"
  check "so the Gate was left in transparent mode" "$(gate_says | field mode)" "transparent"

  echo "== transparent mode: the address the game server sees (want $CLIENT_IP)"
  check "tcp source" "$(seen tcp)" "$CLIENT_IP"
  check "udp source" "$(seen udp)" "$CLIENT_IP"

  echo "== ports: each of a server's, for the protocols it was given"
  check "the Gate forwards all five" "$(gate_says | field forwards)" "5"
  check "the further port for udp" "$(dc exec -T gate nft list map inet homewarp fwd_udp | grep -c "$VOICE : ")" "1"
  check "and not for tcp" "$(dc exec -T gate nft list map inet homewarp fwd_tcp | grep -c "$VOICE : " || true)" "0"
  check "as Docker publishes it at home" "$(dc exec -T home docker ps --format '{{.Ports}}' --filter publish=$VOICE/udp | grep -o "$VOICE->$VOICE/[a-z]*" | sort -u | tr '\n' ' ')" "$VOICE->$VOICE/udp "

  echo "== traffic: what goes through a port, as the Gate counts it and home adds it up"
  counted() { core GET /gates | python3 -c 'import json, sys; print(sum(port["traffic_bytes"] for port in json.load(sys.stdin)["ports"]))'; }
  check "the Gate has counted through the ports just used" \
    "$(gate_says | python3 -c 'import json, sys; print(sum(1 for port in json.load(sys.stdin)["traffic"] if port["bytes"] > 0) >= 2)')" "True"
  before=$(counted)
  # Home hears of it the next time it asks the Gate, which is every few seconds.
  for _ in $(seq 25); do
    seen tcp >/dev/null
    [ "$(counted)" -gt "$before" ] && break
    sleep 1
  done
  if [ "$(counted)" -gt "$before" ]; then ok "home has added it to the hour: $(counted) bytes in the last day"; else fail "home added nothing to the $before bytes it had"; fi

  echo "== sleep: a server nobody is on is stopped, and a player who joins wakes it"
  # One of Minecraft's by what its egg sets up, which is how Core tells: only
  # such a server is asked who is on it. The lab's other servers are not.
  core POST /templates "$(egg 'Lab minecraft' 'PORT={{SERVER_PORT}} exec /lab/mc.sh' \
    '{"server.properties": {"parser": "properties", "find": {"server-port": "{{server.build.default.port}}"}}}')" >/dev/null
  mc=$(core POST /servers "{\"name\":\"sleepy\",\"template_id\":3,\"memory_mb\":128,\"port\":$MC,\"protocol\":\"tcp\",\"sleep_minutes\":1}" | field id)
  sleepy() { core GET "/servers/$mc" | field "$1"; }
  until_sleepy() {  # state, seconds
    for _ in $(seq "$2"); do [ "$(sleepy state)" = "$1" ] && return 0; sleep 1; done
    return 1
  }
  on_it() { core GET "/servers/$mc" | python3 -c 'import json, sys; players = json.load(sys.stdin)["players"]; print(players and "%s of %s" % (players["online"], players["max"]))'; }
  until_on() {  # what on_it says, seconds
    for _ in $(seq "$2"); do [ "$(on_it)" = "$1" ] && return 0; sleep 1; done
    return 1
  }
  in_sleepy() { dc exec -T home sh -c "docker exec \$(docker ps -q --filter publish=$MC) $1"; }
  # What a game sends the VPS, as printf writes it: a handshake, and then its
  # list's question or the first packet of a login. What comes back, without
  # the bytes that frame it, which leaves the text.
  asks='\007\000\057\001x\143\335\001\001\000'
  joins='\007\000\057\001x\143\335\002\014\000\012Lab_Player'
  minecraft() {
    dc exec -T client sh -c "printf '$1' | socat -t 3 - TCP:$GATE_IP:$MC,connect-timeout=3 2>/dev/null | tr -d '\000-\037\177-\377'" || true
  }
  until_sleepy running 60 || true
  until_on '0 of 20' 30 || true
  check "it runs, and Core has asked who is on it, as a game's list of servers asks" "$(sleepy state), $(on_it)" "running, 0 of 20"
  in_sleepy 'touch /home/container/someone'
  until_on '1 of 20' 40 || true
  check "somebody comes on, and Core knows" "$(on_it)" "1 of 20"
  sleep 80
  check "with somebody on it, it is left running past its minute" "$(sleepy state)" "running"
  in_sleepy 'rm /home/container/someone'
  until_sleepy asleep 120 || true
  check "a minute after the last one left, it is asleep" "$(sleepy state)" "asleep"
  for _ in $(seq 20); do [ -n "$(dc exec -T home docker ps -q --filter publish=$MC)" ] && break; sleep 1; done
  check "what listens in its place is Core's own program, on the server's port" "$(dc exec -T home docker ps --no-trunc --filter publish=$MC --format '{{.Names}} {{.Command}}' | grep -c -- '-standin .*stand-in')" "1"
  check "a game's list is told that it sleeps, through the VPS" "$(minecraft "$asks" | grep -c 'sleepy is asleep. Join to wake it up.')" "1"
  check "and asking wakes nothing" "$(sleepy state)" "asleep"
  core POST "/servers/$mc/power" '{"action":"stop"}' >/dev/null
  sleep 3
  check "stopped by its owner, it is offline, and nothing listens in its place" "$(sleepy state) $(dc exec -T home docker ps -q --filter publish=$MC | wc -l)" "offline 0"
  check "so a player who joins then wakes nothing" "$(minecraft "$joins" | wc -c) $(sleepy state)" "0 offline"
  # Stopped, it may say so, where its owner asks for that.
  core PUT "/servers/$mc" "{\"name\":\"sleepy\",\"memory_mb\":128,\"port\":$MC,\"protocol\":\"tcp\",\"sleep_minutes\":1,\"says_offline\":true}" >/dev/null
  for _ in $(seq 20); do [ -n "$(dc exec -T home docker ps -q --filter publish=$MC)" ] && break; sleep 1; done
  sleep 1
  check "told to say so, a stopped server tells a game's list that it is offline" "$(minecraft "$asks" | grep -c 'sleepy is offline.')" "1"
  check "and a player who joins is told the same, and starts nothing" "$(minecraft "$joins" | grep -c 'sleepy is offline.') $(sleepy state)" "1 offline"
  check "and so is the next: it goes on saying so" "$(minecraft "$joins" | grep -c 'sleepy is offline.')" "1"
  core POST "/servers/$mc/power" '{"action":"start"}' >/dev/null
  until_sleepy asleep 150 || true
  check "started again and left alone, it is asleep again" "$(sleepy state)" "asleep"
  dc exec -T home sh -c 'pkill homewarp; true'
  start_core
  check "a Homewarp that starts again finds it asleep" "$(sleepy state)" "asleep"
  check "and players get through as before" "$(seen_again 45)" "$CLIENT_IP"
  for _ in $(seq 20); do [ -n "$(dc exec -T home docker ps -q --filter publish=$MC)" ] && break; sleep 1; done
  sleep 1
  told=$(minecraft "$joins")
  check "a player who joins through the VPS is told that it is waking" "$(echo "$told" | grep -c 'sleepy is waking up. Join again in a minute.')" "1"
  until_sleepy running 60 || true
  check "and it is: started by Homewarp, with nobody at the panel" "$(sleepy state)" "running"
  check "Core wrote down who woke it, by the player's own address" "$(core GET /activity | python3 -c 'import json, sys; print(next((entry["detail"] for entry in json.load(sys.stdin) if entry["action"] == "server.wake"), None))')" "Lab_Player from $CLIENT_IP"
  check "and each time it was put to sleep" "$(core GET /activity | python3 -c 'import json, sys; print(sum(entry["action"] == "server.sleep" for entry in json.load(sys.stdin)))')" "2"
  # Core was started again half a minute ago and has every server in hand. One
  # that is not Minecraft's is sent nothing: the lab's game says so itself when
  # a connection is dropped on it with something sent and unread, as a question
  # in another game's language is. (It once was, and twenty players arriving
  # in the same moment as the question lost some of their number for it.)
  check "a server that is not Minecraft's has been asked nothing" "$(core GET /servers/1 | python3 -c 'import json, sys; print(sum("reset by peer" in line for line in json.load(sys.stdin)["console"]))')" "0"
  check "and that one is still as it was: nobody is said to be on it" "$(core GET /servers/1 | field players)" "None"
  core POST "/servers/$mc/power" '{"action":"stop"}' >/dev/null
  until_sleepy offline 30 || true
  core DELETE "/servers/$mc" >/dev/null
  for _ in $(seq 15); do [ "$(gate_says | field forwards)" = 5 ] && break; sleep 1; done
  check "removed, its port is forwarded no more" "$(gate_says | field forwards)" "5"

  echo "== the limit on new connections: how many one address may open in a second"
  # How many of twenty connections opened at the same moment are answered.
  burst() {
    dc exec -T client sh -c "for _ in \$(seq 20); do socat -u TCP:$GATE_IP:$PORT,connect-timeout=2 - 2>/dev/null & done; wait" | grep -c '^tcp ' || true
  }
  limit_is() {  # waits for the Gate's rules to carry a limit
    for _ in $(seq 15); do
      dc exec -T gate nft list table inet homewarp | grep -q "limit rate over $1/second burst $(($1 * 2)) packets" && return 0
      sleep 1
    done
    return 1
  }
  got=$(burst)
  if [ "$got" -ge 18 ]; then ok "control: at thirty a second, twenty at once get through ($got)"; else fail "control: of twenty at once, only $got got through"; fi
  core PUT /settings '{"new_connections":2}' >/dev/null
  if limit_is 2; then ok "the Gate is told the limit the owner set: 2 a second"; else fail "the Gate's rules do not carry the limit that was set"; fi
  got=$(burst)
  if [ "$got" -ge 1 ] && [ "$got" -le 12 ]; then ok "of twenty at once, $got get through and the rest are dropped at the Gate"; else fail "of twenty at once, $got got through with two a second allowed"; fi
  check "the Gate counted what it dropped" "$(dc exec -T gate nft list chain inet homewarp to_home | grep -c 'limit rate over.*counter packets [1-9]')" "1"
  core PUT /settings '{"new_connections":30}' >/dev/null
  if limit_is 30; then ok "and thirty a second again"; else fail "the limit was not set back"; fi
  got=$(burst)
  if [ "$got" -ge 18 ]; then ok "with which twenty at once get through as before ($got)"; else fail "of twenty at once, only $got got through after the limit was set back"; fi

  echo "== the VPS itself: hardened on trial, and undone by itself unless it is kept"
  guard_is() { core GET "/gates/$(gate_is id)/guard" | field "$1"; }
  guard_has() {  # which list, which kind
    core GET "/gates/$(gate_is id)/guard" | python3 -c 'import json, sys; print(json.load(sys.stdin)[sys.argv[1]][sys.argv[2]])' "$1" "$2"
  }
  # Something of the VPS's own that listens before it is hardened: an SSH of its, say.
  dc exec -d gate sh -c 'SVC=2301 /lab/listen.sh'
  sleep 1
  check "control: what listens on the VPS is reached" "$(reach client "$GATE_IP" 2301)" "reached $CLIENT_IP"
  check "there is no guard, and the Gate says what listens there" "$(guard_is state) $(guard_has listening tcp)" "off [2301, $API_PORT]"
  core PUT "/gates/$(gate_is id)/guard" >/dev/null
  check "hardened, on trial" "$(guard_is state)" "trial"
  check "open is what was listening, and the tunnel's own port" "$(guard_has open tcp) $(guard_has open udp)" "[2301, $API_PORT] [51820]"
  # And something that begins to listen after.
  dc exec -d gate sh -c 'SVC=2302 /lab/listen.sh'
  sleep 1
  check "what was listening is still reached" "$(reach client "$GATE_IP" 2301)" "reached $CLIENT_IP"
  blocked "what began to listen afterwards is not" client "$GATE_IP" 2302
  check "and the panel says which that is" "$(guard_has shut tcp)" "[2302]"
  check "players get through as before" "$(seen tcp)" "$CLIENT_IP"
  check "and so does home, to its Gate" "$(gate_is reachable)" "True"
  check "the VPS can still be pinged" "$(dc exec -T client sh -c "ping -c 1 -W 2 $GATE_IP >/dev/null 2>&1 && echo yes")" "yes"
  # Nobody keeps it: its minute runs out.
  for _ in $(seq 80); do [ "$(guard_is state)" = off ] && break; sleep 1; done
  check "not kept, it is undone by the VPS itself within its minute" "$(guard_is state)" "off"
  check "and what it shut is reached again" "$(reach client "$GATE_IP" 2302)" "reached $CLIENT_IP"
  check "nothing of it is written down" "$(dc exec -T gate sh -c 'ls /run/hw | grep -c guard' || true)" "0"

  echo "== the VPS itself: kept"
  core PUT "/gates/$(gate_is id)/guard" >/dev/null
  core POST "/gates/$(gate_is id)/guard/keep" >/dev/null
  check "kept" "$(guard_is state) $(guard_has open tcp)" "kept [2301, 2302, $API_PORT]"
  dc exec -d gate sh -c 'SVC=2303 /lab/listen.sh'
  sleep 1
  blocked "what begins to listen now is shut" client "$GATE_IP" 2303
  if [ "$(guard_is dropped)" -ge 1 ]; then ok "the Gate counts what it drops: $(guard_is dropped) packets"; else fail "the Gate counted nothing dropped"; fi
  # Its minute over, it is still there.
  dc exec -T gate pkill homewarp-gate
  sleep 1
  start_gate
  for _ in $(seq 20); do [ "$(guard_is state 2>/dev/null)" = kept ] && break; sleep 1; done
  check "started again, the Gate comes back with its guard" "$(guard_is state)" "kept"
  blocked "and what it shut is still shut" client "$GATE_IP" 2303
  check "what is open is still reached" "$(reach client "$GATE_IP" 2301)" "reached $CLIENT_IP"
  # A certificate's question is asked on port 80, where nothing listened when the VPS was hardened.
  check "the Gate takes an answer to a certificate's question" "$(gate_asked PUT "/v1/challenge/guarded-Token" '{"answer":"guarded-Token.mark"}')" "200"
  check "and the guard lets whoever asks in to it" "$(dc exec -T client curl -s -m 5 "http://$GATE_IP/.well-known/acme-challenge/guarded-Token")" "guarded-Token.mark"
  check "the answer taken away" "$(gate_asked DELETE "/v1/challenge/guarded-Token")" "204"
  check "the port is shut again" "$(status client "http://$GATE_IP/.well-known/acme-challenge/guarded-Token")" "000"
  check "hardened again, what listens now is open" "$(core PUT "/gates/$(gate_is id)/guard" | field state) $(reach client "$GATE_IP" 2303)" "trial reached $CLIENT_IP"
  core DELETE "/gates/$(gate_is id)/guard" >/dev/null
  check "the guard taken away, on trial as it was" "$(guard_is state)" "off"
  dc exec -T gate sh -c 'pkill -f "TCP4-LISTEN:230"; true'
  check "and nothing of it is left in the Gate's rules" "$(dc exec -T gate nft list table inet homewarp | grep -c guard || true)" "0"

  echo "== control: without the reply mark nothing comes back, so the return path is what carries it"
  dc exec -T home nft flush chain inet homewarp mark_in
  dc exec -T home nft add rule inet homewarp mark_in iifname homewarp0 ct state new ct mark set 0x4857
  check "tcp without the reply mark" "$(seen tcp)" ""
  dc exec -T home nft add rule inet homewarp mark_in iifname != homewarp0 ct mark 0x4857 meta mark set ct mark
  # Once in six runs the first try after the rule was put back got nothing, and
  # why was not found. It is tried again before that is called a failure, and
  # the run says when it had to be, so that it can be counted.
  restored=$(seen tcp)
  if [ -z "$restored" ]; then
    echo "   (nothing at the first try with the rule restored: trying for ten seconds more)"
    restored=$(seen_again 10)
  fi
  check "tcp with it restored" "$restored" "$CLIENT_IP"

  echo "== NAT mode: a home whose replies cannot leave through the tunnel (want $GATE_TUN)"
  # Something on the machine that sends marked replies the ordinary way out, ahead of Homewarp's rule.
  dc exec -T home ip rule add pref 100 fwmark 0x4857 lookup main
  check "control: nobody gets through" "$(seen tcp)" ""
  check "Core finds that out and has the Gate stand in for players" "$(check_again)" "hidden"
  check "the Gate is in NAT mode" "$(gate_says | field mode)" "nat"
  check "tcp source" "$(seen tcp)" "$GATE_TUN"
  check "udp source" "$(seen udp)" "$GATE_TUN"
  dc exec -T home ip rule del pref 100
  check "and with the way back open again, Core finds that out too" "$(check_again)" "preserved"
  check "the player's own address again" "$(seen tcp)" "$CLIENT_IP"

  echo "== containment: a taken-over gate, its allowed-ips widened and home's networks routed into the tunnel"
  HOME_PUB=$(dc exec -T home wg show homewarp0 public-key)
  # Whoever has taken a VPS over is not running the Gate program, which would find
  # its interface changed and put it back within half a minute, at the cost of the
  # session. The kernel goes on forwarding without it.
  dc exec -T gate pkill homewarp-gate
  { vars HOME_PUB HOME_TUN; cat <<'EOF'; } | in_ gate
wg set homewarp0 peer "$HOME_PUB" allowed-ips "$HOME_TUN/32,192.168.50.0/24,10.213.80.0/24"
ip route replace 192.168.50.0/24 dev homewarp0
ip route replace 10.213.80.0/24 dev homewarp0
EOF
  check "control: the NAS answers on the LAN" "$(reach home "$NAS_IP" "$SVC")" "reached $HOME_LAN_IP"
  check "control: home answers on the LAN" "$(reach nas "$HOME_LAN_IP" "$SVC")" "reached $NAS_IP"
  check "control: the unpublished port is open in the container" "$(reach home "$GAME_IP" "$CLOSED")" "closed"
  check "control: the panel answers on the home network" "$(panel nas "$HOME_LAN_IP")" "HTTP/1.0 200 OK"
  blocked "gate cannot reach a service on home's tunnel address" gate "$HOME_TUN" "$SVC"
  check "gate cannot reach the panel on home's tunnel address" "$(panel gate "$HOME_TUN")" ""
  check "gate cannot reach the panel on home's LAN address" "$(panel gate "$HOME_LAN_IP")" ""
  blocked "gate cannot reach a service on home's LAN address" gate "$HOME_LAN_IP" "$SVC"
  blocked "gate cannot reach the NAS" gate "$NAS_IP" "$SVC"
  blocked "gate cannot reach an unpublished port of the game container" gate "$GAME_IP" "$CLOSED"
  blocked "gate cannot reach the published port by the container's own address" gate "$GAME_IP" "$PORT"
  { vars HOME_PUB HOME_TUN; cat <<'EOF'; } | in_ gate
ip route del 192.168.50.0/24 dev homewarp0
ip route del 10.213.80.0/24 dev homewarp0
wg set homewarp0 peer "$HOME_PUB" allowed-ips "$HOME_TUN/32"
EOF
  start_gate

  echo "== containment: the game container"
  check "game reaches the internet from the home's own address, not through the gate" "$(reach game "$CLIENT_IP" "$SVC")" "reached $HOME_IP"
  blocked "game cannot reach the NAS" game "$NAS_IP" "$SVC"
  blocked "game cannot reach the home's router" game "$ROUTER_LAN_IP" "$SVC"
  blocked "game cannot reach a service on the home host by its bridge address" game 10.213.80.1 "$SVC"
  blocked "game cannot reach a service on the home host by its LAN address" game "$HOME_LAN_IP" "$SVC"

  echo "== the Gate program, against PLAN.md §7.1"
  check "it answers nobody without its token" "$(dc exec -T home curl -s -m 5 -o /dev/null -w '%{http_code}' "http://$GATE_TUN:$API_PORT/v1/status")" "401"
  check "and answers home, which has it" "$(dc exec -T home curl -s -m 5 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $(gate_token)" "http://$GATE_TUN:$API_PORT/v1/status")" "200"
  blocked "it does not answer on the public address" client "$GATE_IP" "$API_PORT"
  rss=$(dc exec -T gate sh -c 'awk "/VmRSS/ { print \$2 }" /proc/$(pidof homewarp-gate)/status')
  if [ "$rss" -le 20480 ]; then ok "it holds $((rss / 1024)) MB of memory, within its 20 MB"; else fail "it holds $rss kB of memory, over its 20 MB"; fi
  size=$(stat -c %s ../deploy/out/homewarp-gate)
  if [ "$size" -le 10485760 ]; then ok "it is one file of $((size / 1024)) kB, within its 10 MB"; else fail "it is $size bytes, over its 10 MB"; fi
  for arm in ../deploy/out/homewarp-gate-arm64; do
    if [ -e "$arm" ] && [ "$(stat -c %s "$arm")" -le 10485760 ]; then ok "and for ARM64 one of $(($(stat -c %s "$arm") / 1024)) kB"; else fail "there is no ARM64 Gate within 10 MB"; fi
  done
  check "the container it runs in, held to 1 CPU and 128 MB, was never out of memory" "$(docker inspect -f '{{.State.OOMKilled}}' homewarp-lab-gate-1)" "false"

  echo "== self-healing: the VPS"
  check "control: a player gets through before the Gate is stopped" "$(seen tcp)" "$CLIENT_IP"
  dc exec -T gate pkill homewarp-gate
  sleep 1
  check "players still get through while it is not running" "$(seen tcp)" "$CLIENT_IP"
  start_gate
  sleep 2
  check "and when it is started again over the tunnel it left" "$(seen tcp)" "$CLIENT_IP"
  dc exec -T gate nft delete table inet homewarp
  check "control: with its table emptied by something else, nobody gets through" "$(seen tcp)" ""
  check "the Gate puts its table back by itself" "$(seen_again 45)" "$CLIENT_IP"
  dc exec -T gate pkill homewarp-gate
  dc exec -T gate sh -c 'ip link del homewarp0; nft delete table inet homewarp'
  check "control: with the tunnel gone from the kernel, nobody gets through" "$(seen tcp)" ""
  start_gate
  # As after a reboot of the VPS. Home notices when something it sent goes
  # unanswered, which Core's asking after the Gate sees to.
  check "started as after a reboot, it is back with its own keys and what it was last told" "$(seen_again 45)" "$CLIENT_IP"

  if [ "$GATE_FW" = firewalld ]; then
    dc exec -T gate firewall-cmd -q --reload
    check "firewalld is reloaded: the Gate's table is left alone, and home still reaches the Gate" "$(seen tcp) $(gate_asked GET /v1/status)" "$CLIENT_IP 200"
  fi

  echo "== self-healing: home"
  dc exec -T home sh -c 'ip link del homewarp0; nft delete table inet homewarp; ip rule del fwmark 0x4857 lookup 4857; true'
  check "control: with home's end gone from the kernel, nobody gets through" "$(seen tcp)" ""
  check "Core puts it back by itself" "$(seen_again 30)" "$CLIENT_IP"
  dc exec -T home sh -c 'pkill homewarp; ip link del homewarp0; nft delete table inet homewarp; ip rule del fwmark 0x4857 lookup 4857; true'
  start_core
  # As after a reboot of the home machine, with its servers started again by Docker.
  check "started as after a reboot, Core brings the tunnel back" "$(seen_again 30)" "$CLIENT_IP"
  check "and still has its Gate" "$(gate_is state)" "connected"

  echo "== self-healing: the home's address changes (to $HOME_IP_NEXT)"
  wan=$(dc exec -T router sh -c "ip -o -4 addr show | awk -v ip=$HOME_IP 'index(\$4, ip \"/\") == 1 { print \$2 }'")
  # The old address goes first: taken away second, it would take the new one with it.
  dc exec -T router sh -c "ip addr del $HOME_IP/24 dev $wan && ip addr add $HOME_IP_NEXT/24 dev $wan"
  check "players get through again" "$(seen_again 45)" "$CLIENT_IP"
  check "and the Gate hears home from its new address" "$(gate_says | field home_endpoint)" "$HOME_IP_NEXT"
  dc exec -T router sh -c "ip addr del $HOME_IP_NEXT/24 dev $wan && ip addr add $HOME_IP/24 dev $wan"
  check "and from the old one, changed back" "$(seen_again 45)" "$CLIENT_IP"

  echo "== a certificate's question: the answer is put on the VPS, and served on port 80 while it is there"
  token=lab-Token_0123456789 answer=lab-Token_0123456789.a-key_s-mark
  check "nothing answers on the VPS's port 80" "$(status client "http://$GATE_IP/.well-known/acme-challenge/$token")" "000"
  check "the Gate takes an answer from home" "$(gate_asked PUT "/v1/challenge/$token" "{\"answer\":\"$answer\"}")" "200"
  check "and anyone on the internet is given it" "$(dc exec -T client curl -s -m 5 "http://$GATE_IP/.well-known/acme-challenge/$token")" "$answer"
  check "and nothing else: not another file" "$(status client --path-as-is "http://$GATE_IP/.well-known/acme-challenge/..%2F..%2Fhw%2Fconfig.json")" "404"
  check "nor any other page" "$(status client "http://$GATE_IP/")" "404"
  check "a token that names another directory is refused" "$(gate_asked PUT "/v1/challenge/..%2F..%2Fhw%2Fconfig.json" "{\"answer\":\"$answer\"}")" "422"
  check "and so is an answer that is not one" "$(gate_asked PUT "/v1/challenge/$token" '{"answer":"<script>"}')" "422"
  check "nobody without the token puts one there" "$(status home -X PUT -H 'Content-Type: application/json' -d "{\"answer\":\"$answer\"}" "http://$GATE_TUN:$API_PORT/v1/challenge/$token")" "401"
  check "the Gate takes the answer away" "$(gate_asked DELETE "/v1/challenge/$token")" "204"
  check "and lets go of port 80" "$(status client "http://$GATE_IP/.well-known/acme-challenge/$token")" "000"

  echo "== the panel online: a name that leads to the VPS, and TLS ended at home"
  check "control: the panel's port is not forwarded before it has a name" "$(status client -k --resolve "$NAME:$TLS:$GATE_IP" "https://$NAME:$TLS/api/v1/health")" "000"
  check "the name is not taken without the authority's terms agreed to" "$(dc exec -T home curl -s -o /dev/null -w '%{http_code}' -b /run/hw/jar -X PUT -H 'Content-Type: application/json' -d "{\"name\":\"$NAME\"}" http://127.0.0.1:3600/api/v1/panel)" "422"
  core PUT /panel "{\"name\":\"$NAME\",\"agreed\":true}" >/dev/null
  for _ in $(seq 60); do [ "$(panel_is state)" = on ] && break; sleep 1; done
  check "Core has a certificate for the name" "$(panel_is state) $(core GET /panel | python3 -c 'import json, sys; print(json.load(sys.stdin)["certificate"]["name"])')" "on $NAME"
  [ "$(panel_is state)" = on ] || echo "   core says: $(panel_is problem)"
  check "and says where the panel is" "$(panel_is address)" "https://$NAME:$TLS"
  check "the answers are gone from the VPS again" "$(dc exec -T gate sh -c 'ls /run/homewarp-gate/challenges | wc -l')" "0"
  for _ in $(seq 15); do [ "$(gate_says | field forwards)" = 6 ] && break; sleep 1; done
  check "the Gate forwards the panel's port as one more" "$(gate_says | field forwards)" "6"
  # What pebble signs with is made anew each time it starts: the lab asks it for the root.
  dc exec -T home curl -sk "https://pebble:15000/roots/0" | dc exec -T client sh -c 'cat > /tmp/roots.pem'
  check "a browser on the internet reaches the panel by its name, and trusts its certificate" "$(browse /api/v1/health | field status)" "ok"
  check "the certificate is for that name and no other" "$(status client --cacert /tmp/roots.pem --resolve "other.lab:$TLS:$GATE_IP" "https://other.lab:$TLS/api/v1/health")" "000"
  # As a browser sends it: over HTTP/2, and saying which page asked.
  # Two cookies: the session, and the one this browser is known to the account by.
  cookies=$(browse -D - -o /dev/null -X POST -H "Origin: https://$NAME:$TLS" -H 'Content-Type: application/json' -d '{"username":"lab","password":"only-in-the-lab"}' /api/v1/login | grep -i '^set-cookie:' || true)
  check "signed in there by the panel's own page, the cookies are for TLS only" "$(printf '%s
' "$cookies" | grep -c .) cookies, $(printf '%s
' "$cookies" | grep -ci '; Secure') for TLS only" "2 cookies, 2 for TLS only"
  check "and a page of another site's is refused there" "$(browse -o /dev/null -w '%{http_code}' -X POST -H 'Origin: https://elsewhere.example' -H 'Content-Type: application/json' -d '{"username":"lab","password":"only-in-the-lab"}' /api/v1/login)" "403"
  check "and on the home network it is as it was" "$(dc exec -T home curl -s -D - -o /dev/null -X POST -H 'Content-Type: application/json' -d '{"username":"lab","password":"only-in-the-lab"}' http://127.0.0.1:3600/api/v1/login | grep -ci '^set-cookie:.*; Secure' || true)" "0"
  check "Core knows the browser by its own address" "$(core GET '/activity' | python3 -c 'import json, sys; print(next(entry["detail"] for entry in json.load(sys.stdin) if entry["action"] == "account.sign_in" and "203.0.113" in entry["detail"]))' | grep -o "$CLIENT_IP")" "$CLIENT_IP"
  check "the VPS still cannot reach the panel's other port" "$(panel gate "$HOME_TUN")" ""
  blocked "nor a service on home's tunnel address" gate "$HOME_TUN" "$SVC"
  check "a server cannot be given the panel's port" "$(dc exec -T home curl -s -o /dev/null -w '%{http_code}' -b /run/hw/jar -X POST -H 'Content-Type: application/json' -d "{\"name\":\"clash\",\"template_id\":1,\"memory_mb\":256,\"port\":$TLS}" http://127.0.0.1:3600/api/v1/servers)" "422"
  core PUT /panel '{"name":null}' >/dev/null
  for _ in $(seq 15); do [ "$(gate_says | field forwards)" = 5 ] && break; sleep 1; done
  check "with its name taken away, the Gate forwards the port no more" "$(gate_says | field forwards)" "5"
  check "and nobody reaches the panel from the internet" "$(status client -k --resolve "$NAME:$TLS:$GATE_IP" "https://$NAME:$TLS/api/v1/health")" "000"
  check "players get through as before" "$(seen tcp)" "$CLIENT_IP"

  echo "== a second VPS: a tunnel of its own, and a server that is reached through it"
  dc exec -T gate2 sh -c 'pkill homewarp-gate; ip link del homewarp0; rm -rf /run/hw /usr/local/bin/homewarp-gate; true' 2>/dev/null
  command=$(connect "$GATE2_IP" Second)
  dc exec -T gate2 sh -c "$command --dir /run/hw --no-service --wan eth0" | sed 's/^/   /'
  dc exec -d gate2 sh -c 'homewarp-gate run --dir /run/hw >>/run/hw/log 2>&1'
  until_gate player_addresses preserved 60 "$GATE2_IP" || true
  check "Core has two, and the second is called what it was called" "$(gate_is state) $(gate_is state "$GATE2_IP") $(gate_is name "$GATE2_IP")" "connected connected Second"
  check "players' addresses through the second, as Core found them" "$(gate_is player_addresses "$GATE2_IP")" "preserved"
  check "home has a tunnel to each, and a way back by each" "$(dc exec -T home sh -c 'ls /sys/class/net | grep -c "^homewarp[01]$"; ip rule | grep -c "fwmark 0x485[78] lookup 485[78]"' | tr '\n' ' ')" "2 2 "
  check "the second tunnel has the next addresses" "$(dc exec -T gate2 sh -c "ip -o -4 addr show homewarp0" | awk '{ print $4 }')" "10.213.77.5/30"
  check "the servers there were stay with the first VPS" "$(gate_says | field forwards) $(gate_is servers "$GATE2_IP")" "5 0"
  second=$(gate_is id "$GATE2_IP")
  check "a server cannot be reached through a VPS there is not" "$(dc exec -T home curl -s -o /dev/null -w '%{http_code}' -b /run/hw/jar -X POST -H 'Content-Type: application/json' -d "{\"name\":\"nowhere\",\"template_id\":1,\"memory_mb\":256,\"port\":$SECOND,\"gate_id\":999}" http://127.0.0.1:3600/api/v1/servers)" "422"
  core POST /servers "{\"name\":\"second\",\"template_id\":1,\"memory_mb\":256,\"port\":$SECOND,\"gate_id\":$second}" >/dev/null
  seen_at() { dc exec -T client sh -c "socat -u TCP:$1:$2,connect-timeout=4 - 2>/dev/null || true" | awk '$1 == "tcp" { print $2 }'; }
  for _ in $(seq 40); do [ -n "$(seen_at "$GATE2_IP" "$SECOND")" ] && break; sleep 1; done
  check "a player reaches it at the second VPS, and is seen as themselves" "$(seen_at "$GATE2_IP" "$SECOND")" "$CLIENT_IP"
  check "and not at the first, which does not forward it" "$(seen_at "$GATE_IP" "$SECOND")" ""
  check "the first VPS's server is not reached at the second" "$(seen_at "$GATE2_IP" "$PORT")" ""
  check "and is reached where it was" "$(seen tcp)" "$CLIENT_IP"
  check "Core says which of the two a new server would take, and why" "$(core GET /gates | python3 -c 'import json, sys
said = json.load(sys.stdin)
print(said["recommended"]["gate_id"] in [gate["id"] for gate in said["gates"]], "ms from home" in said["recommended"]["why"])')" "True True"
  sleep 5
  check "and has looked at what passes through each tunnel" "$(core GET /gates/activity | python3 -c 'import json, sys
print(sorted(len(gate["samples"]) > 0 for gate in json.load(sys.stdin)["gates"]))')" "[True, True]"
  check "the second Gate says how busy its VPS is" "$(gate_is load_percent "$GATE2_IP" | grep -c '^[0-9][0-9]*$')" "1"
  core DELETE "/gates/$second"
  for _ in $(seq 40); do [ -n "$(seen_at "$GATE_IP" "$SECOND")" ] && break; sleep 1; done
  check "the second VPS disconnected, its server is reached through the first" "$(seen_at "$GATE_IP" "$SECOND")" "$CLIENT_IP"
  check "and home's second tunnel is gone, with its way back" "$(dc exec -T home sh -c 'ls /sys/class/net | grep -c "^homewarp1$"; ip rule | grep -c 0x4858' | tr '\n' ' ')" "0 0 "
  check "the first is as it was" "$(seen tcp) $(gate_is reachable)" "$CLIENT_IP True"

  echo "== a VPS is disconnected"
  core DELETE "/gates/$(gate_is id)"
  check "Core has no Gate" "$(gate_is state)" "none"
  check "home's end is gone from the kernel" "$(dc exec -T home sh -c 'ls /sys/class/net | grep -c homewarp0; nft list table inet homewarp >/dev/null 2>&1 && echo table; ip rule | grep -c 0x4857' | tr '\n' ' ')" "0 0 "
  check "nobody gets through" "$(seen tcp)" ""
  check "and the Gate was told to forward nothing more" "$(dc exec -T gate cat /run/hw/desired.json | field forwards)" "[]"

  echo "== with no VPS, a server is kept from the home network all the same"
  check "the table that does it is there" "$(dc exec -T home sh -c 'nft list table inet homewarp_keep >/dev/null 2>&1 && echo there')" "there"
  blocked "game cannot reach the NAS" game "$NAS_IP" "$SVC"
  blocked "game cannot reach a service on the home host by its LAN address" game "$HOME_LAN_IP" "$SVC"
  check "and still reaches the internet" "$(reach game "$CLIENT_IP" "$SVC")" "reached $HOME_IP"

  echo "== a home that is already on a network with the tunnel's addresses"
  dc exec -T home ip route add 10.213.77.0/24 dev eth0
  check "a VPS is not connected there, and Core says which network is in the way" "$(dc exec -T home curl -s -m 10 -b /run/hw/jar -X POST -H 'Content-Type: application/json' -d "{\"address\":\"$GATE_IP\"}" http://127.0.0.1:3600/api/v1/gates | python3 -c 'import json, sys; print("eth0 10.213.77.0/24" in json.load(sys.stdin).get("error", ""))')" "True"
  check "and nothing was begun" "$(gate_is state)" "none"
  dc exec -T home ip route del 10.213.77.0/24 dev eth0

  echo "-- drop counters at the gate"
  dc exec -T gate nft list table inet homewarp | grep 'counter packets' | sed 's/^[[:space:]]*/   /'

  if [ "$GATE_FW" = firewalld ]; then
    echo "== the VPS's own firewall: connected a second time, and then left"
    command=$(connect "$GATE_IP")
    dc exec -T gate pkill homewarp-gate || true
    check "the zone is there already, and the command says nothing against that" "$(dc exec -T gate sh -c "$command --dir /run/hw --no-service --wan eth0 2>&1" | grep -c 'firewalld: opened')" "1"
    start_gate
    until_gate state connected 40 || true
    check "and the VPS is connected again" "$(gate_is state)" "connected"
    core DELETE "/gates/$(gate_is id)"
    dc exec -T gate pkill homewarp-gate || true
    dc exec -T gate homewarp-gate leave --dir /run/hw | sed 's/^/   /'
    check "the Gate left: its zone is gone, and the tunnel's port is shut again" "$(fw --get-zones | tr ' ' '\n' | grep -c '^homewarp$' || true) '$(fw --zone=public --list-ports)' '$(fw --permanent --zone=public --list-ports)'" "0 '2301-2303/tcp' '2301-2303/tcp'"
    check "and what the owner opened is as it was" "$(fw --zone=public --list-services | tr ' ' '\n' | grep -c '^http$' || true)" "1"
  fi
  return $FAILED
}

iperf() {  # label, host, extra iperf3 args
  echo " $1"
  dc exec -T client sh -c "iperf3 -c $2 -p $IPERF -t 5 -f m ${3:-} 2>&1" | grep -E 'sender|receiver|error' | sed 's/^/   /'
}

# The processor time the Gate program has used so far, in hundredths of a second.
gate_ticks() { dc exec -T gate sh -c 'awk "{ print \$14 + \$15 }" /proc/$(pidof homewarp-gate)/stat'; }

cmd_bench() {
  local before
  echo "== throughput to a server at home, 5 s each"
  echo "   (one 2-vCPU machine plays every party: compare the rows, do not quote them)"
  before=$(gate_ticks)
  iperf "through the home's own port forward, tcp"  "$HOME_IP"
  iperf "through the gate, tcp"                 "$GATE_IP"
  iperf "through the gate, tcp, server sends"   "$GATE_IP" -R
  iperf "through the gate, tcp at 100 Mbit/s"   "$GATE_IP" "-b 100M"
  iperf "through the port forward, udp at 10 Mbit/s" "$HOME_IP" "-u -b 10M"
  iperf "through the gate, udp at 10 Mbit/s"    "$GATE_IP" "-u -b 10M"
  echo "== the Gate program while all of that passed (the container: 1 CPU, 128 MB)"
  echo "   processor time used: $(( $(gate_ticks) - before )) hundredths of a second"
  docker stats --no-stream --format '   memory {{.MemUsage}}' homewarp-lab-gate-1
}

cmd_down() { dc down --remove-orphans; }

case "${1:-all}" in
  all)
    cmd_down
    cmd_up
    rc=0
    # The bench first: the tests end by disconnecting the VPS.
    cmd_bench || true
    cmd_test || rc=$?
    cmd_down
    exit $rc ;;
  up)    cmd_up ;;
  test)  cmd_test ;;
  bench) cmd_bench ;;
  down)  cmd_down ;;
  clean) dc down --remove-orphans -v --rmi local ;;
  *) echo "usage: $0 all | up | test | bench | down | clean" >&2; exit 2 ;;
esac
