#!/usr/bin/env bash
# The lab: a VPS without a VPS (PLAN.md §10).
#
# Runs on the homelab; `scripts/dev.sh lab` syncs the tree and starts it there.
# Every party is a container on two internal Docker networks, so the homelab's
# own networking is not touched:
#
#   client 203.0.113.50  ─┐
#   gate   203.0.113.10  ─┼─ "internet"
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
# Usage: run.sh all | up | test | bench | down | clean
#        HOME_FW=nftables run.sh all     # Docker's nftables firewall backend at home
set -euo pipefail
cd "$(dirname "$0")"

export HOME_FW=${HOME_FW:-iptables}
GATE_IP=203.0.113.10 HOME_IP=203.0.113.20 HOME_IP_NEXT=203.0.113.21 CLIENT_IP=203.0.113.50
ROUTER_LAN_IP=192.168.50.2 HOME_LAN_IP=192.168.50.20 NAS_IP=192.168.50.30
GATE_TUN=10.213.77.1 HOME_TUN=10.213.77.2 GAME_IP=10.213.80.2
API_PORT=4857   # where the Gate answers home, on its tunnel address only
PORT=25565      # the game's port, tcp + udp, published by Docker
IPERF=25566     # iperf3 in a server of its own, published by Docker
CLOSED=25567    # open in the game container, not published
VOICE=25568     # a further port of the game's, for udp alone
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

# What the Gate now asks of whoever talks to it, and what it says of itself when
# home asks through the tunnel.
gate_token() { dc exec -T gate cat /run/hw/config.json | field token; }
gate_says()  {
  dc exec -T home curl -s -m 5 -H "Authorization: Bearer $(gate_token)" "http://$GATE_TUN:$API_PORT/v1/status"
}

# Core's view of its Gate: one field of it, and waiting for a field to be a value.
gate_is() { core GET /gate | field "$1"; }
until_gate() {  # field, value, seconds
  for _ in $(seq "$3"); do
    [ "$(gate_is "$1" 2>/dev/null)" = "$2" ] && return 0
    sleep 1
  done
  return 1
}

# Has Core find out again how players' addresses arrive, and says what it found.
check_again() { core POST /gate/check | field player_addresses; }

# Starts the two programs, as their services would.
start_gate() { dc exec -d gate sh -c 'homewarp-gate run --dir /run/hw >>/run/hw/log 2>&1'; }
start_core() {
  # On every address, as the deployment listens: the rules are what keep the Gate from it.
  # And told which image to listen from when it probes the tunnel: here it does not run from one.
  dc exec -d home sh -c 'HOMEWARP_DATA=/run/hw/core HOMEWARP_LISTEN=0.0.0.0:3600 HOMEWARP_IMAGE=homewarp-lab-core homewarp >>/run/hw/core.log 2>&1'
  for _ in $(seq 40); do core GET /health >/dev/null 2>&1 && break; sleep 0.5; done
}

cmd_up() {
  local code command token

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
  { vars SVC ROUTER_LAN_IP; cat <<'EOF'; } | in_ home
set -eu
ip route replace default via "$ROUTER_LAN_IP"
pkill homewarp 2>/dev/null || true
docker ps -aq --filter label=homewarp.server | xargs -r docker rm -f >/dev/null
ip link del homewarp0 2>/dev/null || true
rm -rf /run/hw && mkdir -p /run/hw/core
SVC="$SVC" /lab/listen.sh </dev/null >/dev/null 2>&1 &
# The worst case for the return path: a host that filters reverse paths strictly.
sysctl -qw net.ipv4.conf.all.rp_filter=1 net.ipv4.conf.default.rp_filter=1
EOF
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

  echo "== a VPS is connected: Core makes the one command, and the VPS runs it"
  command=$(core POST /gate "{\"address\":\"$GATE_IP\"}" | field command)
  token=${command##* }
  echo "   ${command:0:60}… (${#token} characters)"
  dc cp ../deploy/out/homewarp-gate gate:/usr/local/bin/homewarp-gate
  dc exec -T gate sh -c 'pkill homewarp-gate; ip link del homewarp0; rm -rf /run/hw; true' 2>/dev/null
  # No service in a container: the lab starts the Gate itself.
  # Nor a default route, on a network with no way out: the interface is named.
  dc exec -T gate homewarp-gate join "$token" --dir /run/hw --no-service --wan eth0 | sed 's/^/   /'
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

cmd_test() {
  local HOME_PUB joined carried rss size wan
  echo "home: $(dc exec -T home sh -c 'docker version --format "Docker {{.Server.Version}}"; iptables --version' | tr '\n' ' ') firewall backend $HOME_FW"

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
  check "the command cannot be run a second time for another Gate" "$(core GET /gate | field command)" "None"

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
  counted() { core GET /gate | python3 -c 'import json, sys; print(sum(port["traffic_bytes"] for port in json.load(sys.stdin)["ports"]))'; }
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

  echo "== control: without the reply mark nothing comes back, so the return path is what carries it"
  dc exec -T home nft flush chain inet homewarp mark_in
  dc exec -T home nft add rule inet homewarp mark_in iifname homewarp0 ct state new ct mark set 0x4857
  check "tcp without the reply mark" "$(seen tcp)" ""
  dc exec -T home nft add rule inet homewarp mark_in iifname != homewarp0 ct mark 0x4857 meta mark set ct mark
  check "tcp with it restored" "$(seen tcp)" "$CLIENT_IP"

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

  echo "== self-healing: home"
  dc exec -T home sh -c 'ip link del homewarp0; nft delete table inet homewarp; ip rule del fwmark 0x4857 lookup 4857; true'
  check "control: with home's end gone from the kernel, nobody gets through" "$(seen tcp)" ""
  check "Core puts it back by itself" "$(seen_again 30)" "$CLIENT_IP"
  dc exec -T home sh -c 'pkill homewarp; ip link del homewarp0; nft delete table inet homewarp; ip rule del fwmark 0x4857 lookup 4857; true'
  start_core
  # As after a reboot of the home machine, with its servers started again by Docker.
  check "started as after a reboot, Core brings the tunnel back" "$(seen_again 30)" "$CLIENT_IP"
  check "and still has its Gate" "$(core GET /gate | field state)" "connected"

  echo "== self-healing: the home's address changes (to $HOME_IP_NEXT)"
  wan=$(dc exec -T router sh -c "ip -o -4 addr show | awk -v ip=$HOME_IP 'index(\$4, ip \"/\") == 1 { print \$2 }'")
  # The old address goes first: taken away second, it would take the new one with it.
  dc exec -T router sh -c "ip addr del $HOME_IP/24 dev $wan && ip addr add $HOME_IP_NEXT/24 dev $wan"
  check "players get through again" "$(seen_again 45)" "$CLIENT_IP"
  check "and the Gate hears home from its new address" "$(gate_says | field home_endpoint)" "$HOME_IP_NEXT"
  dc exec -T router sh -c "ip addr del $HOME_IP_NEXT/24 dev $wan && ip addr add $HOME_IP/24 dev $wan"
  check "and from the old one, changed back" "$(seen_again 45)" "$CLIENT_IP"

  echo "== a VPS is disconnected"
  core DELETE /gate
  check "Core has no Gate" "$(gate_is state)" "none"
  check "home's end is gone from the kernel" "$(dc exec -T home sh -c 'ls /sys/class/net | grep -c homewarp0; nft list table inet homewarp >/dev/null 2>&1 && echo table; ip rule | grep -c 0x4857' | tr '\n' ' ')" "0 0 "
  check "nobody gets through" "$(seen tcp)" ""
  check "and the Gate was told to forward nothing more" "$(dc exec -T gate cat /run/hw/desired.json | field forwards)" "[]"

  echo "-- drop counters at the gate"
  dc exec -T gate nft list table inet homewarp | grep 'counter packets' | sed 's/^[[:space:]]*/   /'
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
