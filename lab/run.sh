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
# Usage: run.sh all | up | test | bench | down | clean
#        HOME_FW=nftables run.sh all     # Docker's nftables firewall backend at home
set -euo pipefail
cd "$(dirname "$0")"

export HOME_FW=${HOME_FW:-iptables}
GATE_IP=203.0.113.10 HOME_IP=203.0.113.20 CLIENT_IP=203.0.113.50
HOME_LAN_IP=192.168.50.20 NAS_IP=192.168.50.30
GATE_TUN=10.213.77.1 HOME_TUN=10.213.77.2 GAME_IP=10.213.80.2
WG_PORT=51820
PORT=25565      # the game's port, tcp + udp, published by Docker
IPERF=25566     # iperf3 in the game container, published by Docker
CLOSED=25567    # open in the game container, not published
SVC=2222        # listen.sh on home, nas and client

dc()   { docker compose -p homewarp-lab --progress quiet "$@"; }
in_()  { dc exec -T "$1" sh -s; }
vars() { local v; for v in "$@"; do printf "%s='%s'\n" "$v" "${!v}"; done; }

cmd_up() {
  local HOME_PUB GATE_PUB

  echo "== containers"
  dc build
  dc up -d
  for _ in $(seq 60); do dc exec -T home docker info >/dev/null 2>&1 && break; sleep 1; done
  docker save homewarp-lab-node | dc exec -T home docker load -q >/dev/null

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

  echo "== gate: tunnel endpoint and forwards"
  { vars HOME_PUB WG_PORT HOME_TUN GATE_TUN PORT IPERF; cat <<'EOF'; } | in_ gate
set -eu
ip link del homewarp0 2>/dev/null || true
ip link add homewarp0 type wireguard
wg set homewarp0 listen-port "$WG_PORT" private-key /run/hw/key \
  peer "$HOME_PUB" preshared-key /run/hw/psk allowed-ips "$HOME_TUN/32"
ip addr add "$GATE_TUN/30" dev homewarp0
ip link set homewarp0 mtu 1380 up

nft -f - <<NFT
table inet homewarp
delete table inet homewarp
table inet homewarp {
  map fwd_tcp { type inet_service : ipv4_addr . inet_service; elements = { $PORT : $HOME_TUN . $PORT, $IPERF : $HOME_TUN . $IPERF } }
  map fwd_udp { type inet_service : ipv4_addr . inet_service; elements = { $PORT : $HOME_TUN . $PORT, $IPERF : $HOME_TUN . $IPERF } }
  set newconn { type ipv4_addr; size 65535; flags dynamic,timeout; timeout 1m; }

  chain prerouting {
    type nat hook prerouting priority dstnat; policy accept;
    iifname "eth0" dnat ip to tcp dport map @fwd_tcp
    iifname "eth0" dnat ip to udp dport map @fwd_udp
  }
  # Empty in transparent mode; NAT mode adds one masquerade rule here.
  chain nat_mode {
    type nat hook postrouting priority srcnat; policy accept;
  }
  chain forward {
    type filter hook forward priority filter; policy accept;
    oifname "homewarp0" ct status dnat goto to_home
    oifname "homewarp0" counter drop                 # only forwarded ports enter the tunnel
    iifname "homewarp0" ct state new counter drop    # home does not use the gate as an exit
  }
  chain to_home {
    ct state new add @newconn { ip saddr limit rate over 30/second burst 60 packets } counter drop
    tcp flags syn tcp option maxseg size set rt mtu
  }
}
NFT
EOF

  echo "== home: game container behind a published port, tunnel, return path"
  { vars GATE_PUB GATE_IP GATE_TUN WG_PORT HOME_TUN GAME_IP PORT IPERF CLOSED SVC; cat <<'EOF'; } | in_ home
set -eu
docker rm -f game >/dev/null 2>&1 || true
docker network inspect homewarp-br >/dev/null 2>&1 ||
  docker network create -d bridge --subnet 10.213.80.0/24 \
    -o com.docker.network.bridge.name=homewarp-br \
    -o com.docker.network.bridge.enable_icc=false homewarp-br >/dev/null
# Hardened the way PLAN.md §5.6 says every server will be.
docker run -d --name game --network homewarp-br --ip "$GAME_IP" \
  --user 4857:4857 --cap-drop ALL --security-opt no-new-privileges \
  --read-only --tmpfs /tmp --pids-limit 256 --memory 256m \
  -e PORT="$PORT" -e IPERF="$IPERF" -e CLOSED="$CLOSED" \
  -p "$PORT:$PORT/tcp" -p "$PORT:$PORT/udp" -p "$IPERF:$IPERF/tcp" -p "$IPERF:$IPERF/udp" \
  homewarp-lab-node /lab/game.sh >/dev/null
SVC="$SVC" /lab/listen.sh </dev/null >/dev/null 2>&1 &

# The worst case for the return path: a host that filters reverse paths strictly.
sysctl -qw net.ipv4.conf.all.rp_filter=1 net.ipv4.conf.default.rp_filter=1

ip link del homewarp0 2>/dev/null || true
ip link add homewarp0 type wireguard
wg set homewarp0 private-key /run/hw/key peer "$GATE_PUB" preshared-key /run/hw/psk \
  endpoint "$GATE_IP:$WG_PORT" allowed-ips 0.0.0.0/0 persistent-keepalive 25
ip addr add "$HOME_TUN/30" dev homewarp0
ip link set homewarp0 mtu 1380 up
# Players arrive on this link from addresses that route elsewhere, so it alone goes loose.
sysctl -qw net.ipv4.conf.homewarp0.rp_filter=2

ip rule del fwmark 0x4857 lookup 4857 2>/dev/null || true
ip rule add fwmark 0x4857 lookup 4857
ip route replace default dev homewarp0 table 4857

nft -f - <<'NFT'
table inet homewarp
delete table inet homewarp
table inet homewarp {
  chain mark_in {
    type filter hook prerouting priority mangle; policy accept;
    iifname "homewarp0" ct state new ct mark set 0x4857           # this flow came from the gate
    iifname != "homewarp0" ct mark 0x4857 meta mark set ct mark   # so its replies go back that way
  }
  chain input {
    type filter hook input priority filter; policy accept;
    iifname { "homewarp0", "homewarp-br" } ct state established,related accept
    iifname "homewarp0" counter drop      # the gate may not reach host services
    iifname "homewarp-br" counter drop    # nor may a game container
  }
  chain forward {
    type filter hook forward priority filter - 1; policy accept;
    iifname "homewarp0" oifname "homewarp-br" ct status dnat accept
    iifname "homewarp0" counter drop      # from the tunnel: published game ports, nothing else
    iifname "homewarp-br" ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 } ct state new counter drop
    oifname "homewarp0" tcp flags syn tcp option maxseg size set rt mtu
  }
}
NFT

ping -c 2 -W 2 -q "$GATE_TUN" | tail -2
EOF
}

FAILED=0
ok()    { echo "ok    $1"; }
fail()  { echo "FAIL  $1"; FAILED=1; }
check() { if [ "$2" = "$3" ]; then ok "$1: ${2:-nothing}"; else fail "$1: got '$2', want '$3'"; fi; }

# The address the game server says a player connecting through the gate came from.
seen() {  # tcp | udp
  local addr="TCP:$GATE_IP:$PORT,connect-timeout=4"
  [ "$1" = udp ] && addr="UDP:$GATE_IP:$PORT"
  dc exec -T client sh -c "echo hi | socat -t 3 - $addr 2>/dev/null || true" | awk -v p="$1" '$1 == p { print $2 }'
}

# What answers when $1 connects to $2:$3; empty when nothing does.
reach() {
  local cmd="echo | socat -t 2 - TCP:$2:$3,connect-timeout=3 2>/dev/null || true"
  if [ "$1" = game ]; then dc exec -T home docker exec game sh -c "$cmd"; else dc exec -T "$1" sh -c "$cmd"; fi
}
blocked() {  # label, from, address, port
  if [ -z "$(reach "$2" "$3" "$4")" ]; then ok "$1"; else fail "$1"; fi
}

cmd_test() {
  local HOME_PUB
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

  echo "== NAT mode: with masquerade on the gate the server sees the gate (want $GATE_TUN)"
  dc exec -T gate nft add rule inet homewarp nat_mode oifname homewarp0 masquerade
  check "tcp source" "$(seen tcp)" "$GATE_TUN"
  check "udp source" "$(seen udp)" "$GATE_TUN"
  dc exec -T gate nft flush chain inet homewarp nat_mode

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
