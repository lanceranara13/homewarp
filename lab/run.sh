#!/usr/bin/env bash
# The lab: a VPS without a VPS (PLAN.md §10).
#
# Runs on the homelab; `scripts/dev.sh lab` syncs the tree and starts it there.
# Every party is a container on two internal Docker networks, so the homelab's
# own networking is not touched:
#
#   client 203.0.113.50  ─┐
#   gate   203.0.113.10  ─┼─ "internet"
#   home   203.0.113.20  ─┘
#   home   192.168.50.20 ─┬─ "home LAN"
#   nas    192.168.50.30 ─┘
#
# home is docker-in-docker. A real Docker daemon publishes the game container's
# ports there, which is the part real-vps-spike.sh had to fake with a namespace.
#
# The simulated VPS runs the Gate program itself, built by `scripts/dev.sh lab`.
#
# Usage: run.sh all | up | test | bench | down | clean
#        HOME_FW=nftables run.sh all     # Docker's nftables firewall backend at home
set -euo pipefail
cd "$(dirname "$0")"

export HOME_FW=${HOME_FW:-iptables}
GATE_IP=203.0.113.10 HOME_IP=203.0.113.20 CLIENT_IP=203.0.113.50
HOME_LAN_IP=192.168.50.20 NAS_IP=192.168.50.30
GATE_TUN=10.213.77.1 HOME_TUN=10.213.77.2 GAME_IP=10.213.80.2
WG_PORT=51820
API_PORT=4857   # where the Gate answers home, on its tunnel address only
PORT=25565      # the game's port, tcp + udp, published by Docker
IPERF=25566     # iperf3 in the game container, published by Docker
CLOSED=25567    # open in the game container, not published
SVC=2222        # listen.sh on home, nas and client

dc()   { docker compose -p homewarp-lab --progress quiet "$@"; }
in_()  { dc exec -T "$1" sh -s; }
vars() { local v; for v in "$@"; do printf "%s='%s'\n" "$v" "${!v}"; done; }

# Asks Core, on home, as the panel's page would.
core() {  # method, path, [json]
  dc exec -T home curl -fsS -m 10 -b /run/hw/jar -c /run/hw/jar -X "$1" \
    -H 'Content-Type: application/json' ${3:+-d "$3"} "http://127.0.0.1:3600/api/v1$2"
}

# A lab egg, as the JSON Core's import takes: a server in the lab's own image.
egg() {  # name, startup command
  python3 - "$1" "$2" <<'PY'
import json, sys
print(json.dumps({"egg": json.dumps({
    "meta": {"version": "PTDL_v2"}, "name": sys.argv[1], "description": "For the lab.",
    "docker_images": {"lab": "homewarp-lab-yolk"}, "startup": sys.argv[2],
    "config": {"files": "{}", "startup": "{\"done\": \"ready\"}", "stop": "^C"},
    "scripts": {"installation": {"script": None, "container": "homewarp-lab-yolk", "entrypoint": "sh"}},
    "variables": []})}))
PY
}

# What the Gate says of itself, asked from home through the tunnel.
gate_says() {
  dc exec -T home curl -s -m 5 -H "Authorization: Bearer $(dc exec -T gate cat /run/hw/token)" \
    "http://$GATE_TUN:$API_PORT/v1/status"
}

# Tells Core where its Gate is, as enrolling a VPS will. Core then makes its end
# of the tunnel and tells the Gate what to forward: its servers' ports.
push() {  # transparent | nat
  local nat=false
  [ "$1" = nat ] && nat=true
  core PUT /gate "{\"address\":\"$GATE_IP\",\"wg_port\":$WG_PORT,\"private_key\":\"$(dc exec -T home cat /run/hw/key)\",\"gate_public_key\":\"$(dc exec -T gate sh -c 'wg pubkey < /run/hw/key')\",\"preshared_key\":\"$(dc exec -T home cat /run/hw/psk)\",\"token\":\"$(dc exec -T gate cat /run/hw/token)\",\"api_port\":$API_PORT,\"nat\":$nat}"
  # Core gets to it within its ten seconds.
  for _ in $(seq 20); do
    gate_says | grep -q "\"mode\":\"$1\",\"forwards\":4" && break
    sleep 1
  done
  echo "   the gate says: $(gate_says)"
}

# Starts the Gate program on the simulated VPS, as its service would.
start_gate() { dc exec -d gate sh -c 'homewarp-gate /run/hw >>/run/hw/log 2>&1'; }

cmd_up() {
  local HOME_PUB GATE_PUB TOKEN code

  echo "== containers"
  dc build
  dc up -d
  for _ in $(seq 60); do dc exec -T home docker info >/dev/null 2>&1 && break; sleep 1; done
  docker save homewarp-lab-node | dc exec -T home docker load -q >/dev/null
  docker build -q -t homewarp-lab-yolk -f yolk.Dockerfile . >/dev/null
  docker save homewarp-lab-yolk | dc exec -T home docker load -q >/dev/null

  echo "== keys: each side makes its own; only public keys and the preshared key travel"
  HOME_PUB=$(in_ home <<'EOF'
set -eu
umask 077
mkdir -p /run/hw
wg genkey > /run/hw/key
wg genpsk > /run/hw/psk
wg pubkey < /run/hw/key
EOF
)
  dc exec -T home cat /run/hw/psk | dc exec -T gate sh -c 'umask 077; mkdir -p /run/hw; cat > /run/hw/psk'
  GATE_PUB=$(in_ gate <<'EOF'
set -eu
umask 077
wg genkey > /run/hw/key
wg pubkey < /run/hw/key
EOF
)

  echo "== gate: the Gate program, told who it is and who its home is"
  TOKEN=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
  { vars HOME_PUB WG_PORT HOME_TUN GATE_TUN API_PORT TOKEN; cat <<'EOF'; } | in_ gate
set -eu
umask 077
pkill homewarp-gate 2>/dev/null || true
ip link del homewarp0 2>/dev/null || true
printf '%s' "$TOKEN" > /run/hw/token
rm -f /run/hw/desired.json /run/hw/log
cat > /run/hw/config.json <<JSON
{ "private_key": "$(cat /run/hw/key)", "listen_port": $WG_PORT,
  "address": "$GATE_TUN", "home_address": "$HOME_TUN",
  "home_public_key": "$HOME_PUB", "preshared_key": "$(cat /run/hw/psk)",
  "token": "$TOKEN", "wan": "eth0", "api_port": $API_PORT }
JSON
EOF
  dc cp ../deploy/out/homewarp-gate gate:/usr/local/bin/homewarp-gate
  start_gate
  for _ in $(seq 20); do dc exec -T gate test -e /sys/class/net/homewarp0 && break; sleep 0.5; done
  dc exec -T gate sh -c 'cat /run/hw/log' | sed 's/^/   /'

  echo "== home: Core itself, which makes the servers and its end of the tunnel"
  dc cp ../deploy/out/homewarp-static home:/usr/local/bin/homewarp
  { vars SVC; cat <<'EOF'; } | in_ home
set -eu
pkill homewarp 2>/dev/null || true
docker ps -aq --filter label=homewarp.server | xargs -r docker rm -f >/dev/null
ip link del homewarp0 2>/dev/null || true
rm -rf /run/hw/core /run/hw/jar && mkdir -p /run/hw/core
SVC="$SVC" /lab/listen.sh </dev/null >/dev/null 2>&1 &
# The worst case for the return path: a host that filters reverse paths strictly.
sysctl -qw net.ipv4.conf.all.rp_filter=1 net.ipv4.conf.default.rp_filter=1
HOMEWARP_DATA=/run/hw/core HOMEWARP_LISTEN=127.0.0.1:3600 homewarp >/run/hw/core.log 2>&1 &
EOF
  for _ in $(seq 40); do core GET /health >/dev/null 2>&1 && break; sleep 0.5; done
  code=$(dc exec -T home sh -c "grep -o 'setup code: .*' /run/hw/core.log | cut -d' ' -f3")
  core POST /setup "{\"code\":\"$code\",\"username\":\"lab\",\"password\":\"only-in-the-lab\"}" >/dev/null
  core POST /templates "$(egg 'Lab game' "PORT={{SERVER_PORT}} IPERF=5999 CLOSED=$CLOSED /lab/game.sh")" >/dev/null
  core POST /templates "$(egg 'Lab iperf' 'exec iperf3 -s -p {{SERVER_PORT}}')" >/dev/null
  core POST /servers "{\"name\":\"game\",\"template_id\":1,\"memory_mb\":256,\"port\":$PORT}" >/dev/null
  for _ in $(seq 30); do [ -n "$(dc exec -T home docker ps -q --filter publish=$PORT)" ] && break; sleep 1; done
  core POST /servers "{\"name\":\"iperf\",\"template_id\":2,\"memory_mb\":256,\"port\":$IPERF}" >/dev/null
  for _ in $(seq 30); do [ -n "$(dc exec -T home docker ps -q --filter publish=$IPERF)" ] && break; sleep 1; done
  dc exec -T home docker ps --format '   {{.Names}}  {{.Ports}}' | cut -c1-110

  echo "== Core is told where its Gate is"
  push transparent
}

FAILED=0
ok()    { echo "ok    $1"; }
fail()  { echo "FAIL  $1"; FAILED=1; }
check() { if [ "$2" = "$3" ]; then ok "$1: ${2:-nothing}"; else fail "$1: got '$2', want '$3'"; fi; }

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

cmd_test() {
  local HOME_PUB token rss
  echo "home: $(dc exec -T home sh -c 'docker version --format "Docker {{.Server.Version}}"; iptables --version' | tr '\n' ' ') firewall backend $HOME_FW"

  echo "== transparent mode: the address the game server sees (want $CLIENT_IP)"
  check "tcp source" "$(seen tcp)" "$CLIENT_IP"
  check "udp source" "$(seen udp)" "$CLIENT_IP"

  echo "== control: without the reply mark nothing comes back, so the return path is what carries it"
  dc exec -T home nft flush chain inet homewarp mark_in
  dc exec -T home nft add rule inet homewarp mark_in iifname homewarp0 ct state new ct mark set 0x4857
  check "tcp without the reply mark" "$(seen tcp)" ""
  dc exec -T home nft add rule inet homewarp mark_in iifname != homewarp0 ct mark 0x4857 meta mark set ct mark
  check "tcp with it restored" "$(seen tcp)" "$CLIENT_IP"

  echo "== NAT mode: asked for by home, and the server sees the gate (want $GATE_TUN)"
  push nat
  check "tcp source" "$(seen tcp)" "$GATE_TUN"
  check "udp source" "$(seen udp)" "$GATE_TUN"
  push transparent
  check "and the player's own again in transparent mode" "$(seen tcp)" "$CLIENT_IP"

  echo "== containment: a taken-over gate, its allowed-ips widened and home's networks routed into the tunnel"
  HOME_PUB=$(dc exec -T home wg show homewarp0 public-key)
  { vars HOME_PUB HOME_TUN; cat <<'EOF'; } | in_ gate
wg set homewarp0 peer "$HOME_PUB" allowed-ips "$HOME_TUN/32,192.168.50.0/24,10.213.80.0/24"
ip route replace 192.168.50.0/24 dev homewarp0
ip route replace 10.213.80.0/24 dev homewarp0
EOF
  check "control: the NAS answers on the LAN" "$(reach home "$NAS_IP" "$SVC")" "reached $HOME_LAN_IP"
  check "control: home answers on the LAN" "$(reach nas "$HOME_LAN_IP" "$SVC")" "reached $NAS_IP"
  check "control: the unpublished port is open in the container" "$(reach home "$GAME_IP" "$CLOSED")" "closed"
  blocked "gate cannot reach a service on home's tunnel address" gate "$HOME_TUN" "$SVC"
  blocked "gate cannot reach a service on home's LAN address" gate "$HOME_LAN_IP" "$SVC"
  blocked "gate cannot reach the NAS" gate "$NAS_IP" "$SVC"
  blocked "gate cannot reach an unpublished port of the game container" gate "$GAME_IP" "$CLOSED"
  blocked "gate cannot reach the published port by the container's own address" gate "$GAME_IP" "$PORT"
  { vars HOME_PUB HOME_TUN; cat <<'EOF'; } | in_ gate
ip route del 192.168.50.0/24 dev homewarp0
ip route del 10.213.80.0/24 dev homewarp0
wg set homewarp0 peer "$HOME_PUB" allowed-ips "$HOME_TUN/32"
EOF

  echo "== containment: the game container"
  check "game reaches the internet from home's own address, not through the gate" "$(reach game "$CLIENT_IP" "$SVC")" "reached $HOME_IP"
  blocked "game cannot reach the NAS" game "$NAS_IP" "$SVC"
  blocked "game cannot reach a service on the home host by its bridge address" game 10.213.80.1 "$SVC"
  blocked "game cannot reach a service on the home host by its LAN address" game "$HOME_LAN_IP" "$SVC"

  echo "== the Gate program"
  token=$(dc exec -T gate cat /run/hw/token)
  check "it answers nobody without its token" "$(dc exec -T home curl -s -m 5 -o /dev/null -w '%{http_code}' "http://$GATE_TUN:$API_PORT/v1/status")" "401"
  check "and answers home, which has it" "$(dc exec -T home curl -s -m 5 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $token" "http://$GATE_TUN:$API_PORT/v1/status")" "200"
  blocked "it does not answer on the public address" client "$GATE_IP" "$API_PORT"
  rss=$(dc exec -T gate sh -c 'awk "/VmRSS/ { print \$2 }" /proc/$(pidof homewarp-gate)/status')
  if [ "$rss" -le 20480 ]; then ok "it holds $((rss / 1024)) MB of memory, within the 20 MB of PLAN.md §7.1"; else fail "it holds $rss kB of memory, over its 20 MB"; fi
  check "control: a player gets through before it is stopped" "$(seen tcp)" "$CLIENT_IP"
  dc exec -T gate pkill homewarp-gate
  sleep 1
  check "players still get through while it is not running" "$(seen tcp)" "$CLIENT_IP"
  start_gate
  sleep 2
  check "and when it is started again over the tunnel it left" "$(seen tcp)" "$CLIENT_IP"
  dc exec -T gate pkill homewarp-gate
  dc exec -T gate sh -c 'ip link del homewarp0; nft delete table inet homewarp'
  check "control: with the tunnel gone from the kernel, nobody gets through" "$(seen tcp)" ""
  start_gate
  # As after a reboot of the VPS. Home notices when something it sent goes
  # unanswered, which Core's asking after the Gate will see to. Here a ping does.
  for _ in $(seq 20); do
    dc exec -T home ping -c 1 -W 1 -q "$GATE_TUN" >/dev/null 2>&1 || true
    [ -n "$(seen tcp)" ] && break
    sleep 2
  done
  check "started as after a reboot, it is back to what it was last told" "$(seen tcp)" "$CLIENT_IP"

  echo "-- drop counters at home"
  dc exec -T home nft list table inet homewarp | grep 'counter packets' | sed 's/^[[:space:]]*/   /'
  return $FAILED
}

iperf() {  # label, host, extra iperf3 args
  echo " $1"
  dc exec -T client sh -c "iperf3 -c $2 -p $IPERF -t 5 -f m ${3:-} 2>&1" | grep -E 'sender|receiver|error' | sed 's/^/   /'
}

cmd_bench() {
  echo "== throughput to the game container, 5 s each"
  echo "   (one 2-vCPU machine plays every party: compare the rows, do not quote them)"
  iperf "direct to home's published port, tcp"  "$HOME_IP"
  iperf "through the gate, tcp"                 "$GATE_IP"
  iperf "through the gate, tcp, server sends"   "$GATE_IP" -R
  iperf "direct to home, udp at 10 Mbit/s"      "$HOME_IP" "-u -b 10M"
  iperf "through the gate, udp at 10 Mbit/s"    "$GATE_IP" "-u -b 10M"
  echo "== gate container (limits: 1 CPU, 128 MB)"
  docker stats --no-stream --format '   memory {{.MemUsage}}' homewarp-lab-gate-1
}

cmd_down() { dc down --remove-orphans; }

case "${1:-all}" in
  all)
    cmd_down
    cmd_up
    rc=0
    cmd_test || rc=$?
    cmd_bench || true
    cmd_down
    exit $rc ;;
  up)    cmd_up ;;
  test)  cmd_test ;;
  bench) cmd_bench ;;
  down)  cmd_down ;;
  clean) dc down --remove-orphans -v --rmi local ;;
  *) echo "usage: $0 all | up | test | bench | down | clean" >&2; exit 2 ;;
esac
