#!/usr/bin/env bash
# Phase 0 tunnel spike against a real VPS (PLAN.md §5.3 and §11).
#
# Brings up kernel WireGuard between a throwaway container at home and the VPS,
# forwards two test ports with DNAT and no masquerade, and checks that the
# "game" side sees the client's real address.
#
# Everything it creates is runtime-only and removed by `down`:
#   VPS   wg + iperf3 copied to /dev/shm, link hwspike0, table inet hwspike,
#         iptables rules commented "hwspike" in ufw-user-input
#   home  containers hwspike-home and hwspike-client; the host's own network
#         namespace is not touched
#
# Inside hwspike-home the container's namespace plays the home host and a nested
# namespace "game" plays a game container behind a published port.
#
# Usage: real-vps-spike.sh up | test | mark <sketch|fixed> | bench | down
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

if [ -f "$(dirname "$0")/../scripts/dev.env" ]; then . "$(dirname "$0")/../scripts/dev.env"; fi
: "${HOME_SSH:?name the homelab, an ssh host: HOME_SSH=... (scripts/dev.env.example)}"
GATE_SSH=${GATE_SSH:-${VPS_SSH:?name the VPS, an ssh host: VPS_SSH=... (scripts/dev.env.example)}}
WG_PORT=${WG_PORT:-51900}
PORT=${PORT:-47777}       # echo service, tcp + udp
IPERF=${IPERF:-47778}     # iperf3 in the game namespace, reached through the VPS
DIRECT=${DIRECT:-47779}   # iperf3 on the VPS itself, for the with/without comparison
LAN_PROBE=${LAN_PROBE:-192.168.1.50}   # a home LAN address the VPS must never reach
IMAGE=${IMAGE:-alpine:3.20}

gate()      { ssh -o BatchMode=yes -o ConnectTimeout=10 "$GATE_SSH" "$@"; }
home()      { ssh -o BatchMode=yes -o ConnectTimeout=10 "$HOME_SSH" "$@"; }
in_home()   { home "docker exec -i hwspike-home sh -s"; }
in_client() { home "docker exec -i hwspike-client sh -s"; }
vars()      { local v; for v in "$@"; do printf "%s='%s'\n" "$v" "${!v}"; done; }
gate_ip()   { gate "ip -4 route get 1.1.1.1" | sed -n 's/.* src \([0-9.]*\).*/\1/p'; }

mark_rule() {
  case "$1" in
    sketch) echo 'ct mark 0x4857 meta mark set ct mark' ;;                        # PLAN.md as first written
    fixed)  echo 'iifname != "homewarp0" ct mark 0x4857 meta mark set ct mark' ;;
    *) echo "mark: sketch or fixed" >&2; exit 2 ;;
  esac
}

cmd_up() {
  local HOME_PUB GATE_PUB GATE_IP MARK_LINE
  MARK_LINE=$(mark_rule "${MARK_RULE:-fixed}")

  echo "== home: start containers"
  home "bash -s" <<EOF
set -eu
docker run -d --rm --name hwspike-home --privileged $IMAGE sleep 7200 >/dev/null
docker run -d --rm --name hwspike-client $IMAGE sleep 7200 >/dev/null
docker exec hwspike-home apk add -q --no-cache wireguard-tools-wg iproute2 nftables socat iperf3
docker exec hwspike-client apk add -q --no-cache socat iperf3
EOF

  echo "== VPS: copy wg + iperf3 into tmpfs"
  in_home <<'EOF' | gate "umask 077; mkdir -p /dev/shm/hwspike && tr -d '\r' | base64 -d | tar xz -C /dev/shm/hwspike"
set -eu
mkdir -p /x
cp -L /usr/bin/wg /usr/bin/iperf3 /lib/ld-musl-x86_64.so.1 /x/
ldd /usr/bin/iperf3 | awk '$2 == "=>" && $3 ~ /^\// { print $3 }' | xargs -r -I{} cp -L {} /x/
tar cz -C /x . | base64
EOF

  echo "== keys: each side makes its own, only public keys and the preshared key travel"
  HOME_PUB=$(in_home <<'EOF'
set -eu
umask 077
mkdir -p /run/hw
wg genkey > /run/hw/home.key
wg genpsk > /run/hw/psk
wg pubkey < /run/hw/home.key
EOF
)
  home "docker exec hwspike-home cat /run/hw/psk" | gate "umask 077; cat > /dev/shm/hwspike/psk"
  GATE_PUB=$(gate "bash -s" <<'EOF'
set -eu
umask 077
D=/dev/shm/hwspike
"$D/ld-musl-x86_64.so.1" "$D/wg" genkey > "$D/gate.key"
"$D/ld-musl-x86_64.so.1" "$D/wg" pubkey < "$D/gate.key"
EOF
)
  GATE_IP=$(gate_ip)

  echo "== VPS: wireguard link, firewall opening, DNAT table"
  { vars HOME_PUB WG_PORT PORT IPERF; cat <<'EOF'; } | gate "bash -s"
set -eu
D=/dev/shm/hwspike
wg() { "$D/ld-musl-x86_64.so.1" "$D/wg" "$@"; }
WAN=$(ip -4 route show default | sed -n 's/.* dev \([^ ]*\).*/\1/p' | head -1)

ip link add hwspike0 type wireguard
wg set hwspike0 listen-port "$WG_PORT" private-key "$D/gate.key" \
  peer "$HOME_PUB" preshared-key "$D/psk" allowed-ips 10.213.77.2/32
ip addr add 10.213.77.1/30 dev hwspike0
ip link set hwspike0 mtu 1380 up

# ufw's INPUT chain drops by default, and an accept in another table cannot undo that.
iptables -I ufw-user-input 1 -p udp --dport "$WG_PORT" -m comment --comment hwspike -j ACCEPT

ruleset() {
cat <<NFT
table inet hwspike {
  map fwd_tcp { type inet_service : ipv4_addr . inet_service; elements = { $PORT : 10.213.77.2 . $PORT, $IPERF : 10.213.77.2 . $IPERF } }
  map fwd_udp { type inet_service : ipv4_addr . inet_service; elements = { $PORT : 10.213.77.2 . $PORT, $IPERF : 10.213.77.2 . $IPERF } }
  set newconn { type ipv4_addr; size 65535; flags dynamic,timeout; timeout 1m; }

  chain prerouting {
    type nat hook prerouting priority dstnat - 5; policy accept;
    iifname "$WAN" $1 tcp dport map @fwd_tcp
    iifname "$WAN" $1 udp dport map @fwd_udp
  }
  chain forward {
    type filter hook forward priority filter - 1; policy accept;
    oifname "hwspike0" ct state new ct status dnat add @newconn { ip saddr limit rate over 30/second burst 60 packets } counter drop
    oifname "hwspike0" tcp flags syn tcp option maxseg size set rt mtu
    iifname "hwspike0" tcp flags syn tcp option maxseg size set 1340
    oifname "hwspike0" counter
    iifname "hwspike0" counter
  }
}
NFT
}
if err=$(ruleset "dnat ip to" | nft -f - 2>&1); then
  echo "nft $(nft --version | awk '{print $2}'): accepted 'dnat ip to <port> map @m' as in PLAN.md"
else
  echo "nft rejected the PLAN.md form: $(echo "$err" | head -2 | tr '\n' ' ')"
  ruleset "dnat ip addr . port to" | nft -f -
  echo "accepted 'dnat ip addr . port to <port> map @m'"
fi
EOF

  echo "== home: wireguard link, game namespace, return path"
  { vars GATE_PUB GATE_IP WG_PORT PORT IPERF MARK_LINE; cat <<'EOF'; } | in_home
set -eu
ip link add homewarp0 type wireguard
wg set homewarp0 private-key /run/hw/home.key peer "$GATE_PUB" preshared-key /run/hw/psk \
  endpoint "$GATE_IP:$WG_PORT" allowed-ips 0.0.0.0/0 persistent-keepalive 25
ip addr add 10.213.77.2/30 dev homewarp0
ip link set homewarp0 mtu 1380 up
sysctl -qw net.ipv4.ip_forward=1 net.ipv4.conf.all.rp_filter=2 net.ipv4.conf.homewarp0.rp_filter=2

ip netns add game
ip link add homewarp-br type veth peer name eth0 netns game
ip addr add 10.213.80.1/24 dev homewarp-br
ip link set homewarp-br up
ip -n game link set lo up
ip -n game addr add 10.213.80.2/24 dev eth0
ip -n game link set eth0 up
ip -n game route add default via 10.213.80.1

ip rule add fwmark 0x4857 lookup 4857
ip route add default dev homewarp0 table 4857

nft -f - <<NFT
table inet homewarp {
  chain mark_in {
    type filter hook prerouting priority mangle; policy accept;
    iifname "homewarp0" ct state new ct mark set 0x4857
    $MARK_LINE
  }
  # Stands in for Docker: a published port, and masquerade for the bridge's own outbound traffic.
  chain published {
    type nat hook prerouting priority dstnat; policy accept;
    iifname "homewarp0" tcp dport { $PORT, $IPERF } dnat ip to 10.213.80.2
    iifname "homewarp0" udp dport { $PORT, $IPERF } dnat ip to 10.213.80.2
  }
  chain bridge_out {
    type nat hook postrouting priority srcnat; policy accept;
    oifname "eth0" ip saddr 10.213.80.0/24 masquerade
  }
  chain input {
    type filter hook input priority filter; policy accept;
    iifname "homewarp0" ct state established,related accept
    iifname "homewarp0" counter drop
  }
  chain forward {
    type filter hook forward priority filter - 1; policy accept;
    iifname "homewarp0" oifname != "homewarp-br" counter drop
    iifname "homewarp-br" ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 } ct state new counter drop
    oifname "homewarp0" tcp flags syn tcp option maxseg size set rt mtu
  }
}
NFT

ip netns exec game socat TCP4-LISTEN:$PORT,fork,reuseaddr 'SYSTEM:echo tcp $SOCAT_PEERADDR $SOCAT_PEERPORT' </dev/null >/dev/null 2>&1 &
ip netns exec game socat UDP4-RECVFROM:$PORT,fork 'SYSTEM:echo udp $SOCAT_PEERADDR $SOCAT_PEERPORT' </dev/null >/dev/null 2>&1 &
ip netns exec game iperf3 -s -p $IPERF -D </dev/null >/dev/null 2>&1

ping -c 3 -W 2 -q 10.213.77.1 | tail -2
EOF
}

cmd_mark() {
  local MARK_LINE
  MARK_LINE=$(mark_rule "${1:-}")
  { vars MARK_LINE; cat <<'EOF'; } | in_home
set -eu
nft flush chain inet homewarp mark_in
nft add rule inet homewarp mark_in iifname '"homewarp0"' ct state new ct mark set 0x4857
nft add rule inet homewarp mark_in $MARK_LINE
nft list chain inet homewarp mark_in | sed -n '3,6p'
EOF
}

FAILED=0
check() {  # label, got, want
  if [ "$2" = "$3" ]; then echo "ok    $1: $2"; else echo "FAIL  $1: got '$2', want '$3'"; FAILED=1; fi
}

cmd_test() {
  local GATE_IP SEEN TCP UDP HOME_PUB
  GATE_IP=$(gate_ip)
  SEEN=$(gate "bash -s" <<'EOF'
D=/dev/shm/hwspike
"$D/ld-musl-x86_64.so.1" "$D/wg" show hwspike0 endpoints | awk '{ sub(/:[0-9]+$/, "", $2); print $2 }'
EOF
)
  echo "VPS $GATE_IP sees the home's public address as: $SEEN"

  echo "== client address as the game server sees it (want $SEEN, not 10.213.77.1)"
  TCP=$({ vars GATE_IP PORT; cat <<'EOF'; } | in_client
echo hi | socat -t 4 - "TCP:$GATE_IP:$PORT,connect-timeout=6" 2>/dev/null || true
EOF
)
  UDP=$({ vars GATE_IP PORT; cat <<'EOF'; } | in_client
echo hi | socat -t 4 - "UDP:$GATE_IP:$PORT" 2>/dev/null || true
EOF
)
  check "tcp source" "$(echo "$TCP" | awk '$1 == "tcp" { print $2 }')" "$SEEN"
  check "udp source" "$(echo "$UDP" | awk '$1 == "udp" { print $2 }')" "$SEEN"

  echo "== containment: what the VPS can reach at home"
  HOME_PUB=$(home "docker exec hwspike-home wg show homewarp0 public-key")
  { vars HOME_PUB LAN_PROBE; cat <<'EOF'; } | gate "bash -s"
D=/dev/shm/hwspike
wg() { "$D/ld-musl-x86_64.so.1" "$D/wg" "$@"; }
if ping -c 1 -W 2 10.213.77.2 >/dev/null 2>&1; then echo "FAIL  VPS can ping the home tunnel address"; else echo "ok    VPS cannot reach the home tunnel address"; fi
# Act as a taken-over VPS: widen its own allowed-ips and route a LAN address into the tunnel.
wg set hwspike0 peer "$HOME_PUB" allowed-ips "10.213.77.2/32,$LAN_PROBE/32"
ip route add "$LAN_PROBE/32" dev hwspike0
if timeout 3 bash -c "exec 3<>/dev/tcp/$LAN_PROBE/22" 2>/dev/null; then echo "FAIL  VPS reached $LAN_PROBE:22 through the tunnel"; else echo "ok    VPS cannot reach $LAN_PROBE:22 through the tunnel"; fi
ip route del "$LAN_PROBE/32" dev hwspike0
wg set hwspike0 peer "$HOME_PUB" allowed-ips 10.213.77.2/32
EOF

  echo "== containment: what the game namespace can reach"
  in_home <<'EOF'
if ip netns exec game ping -c 1 -W 2 1.1.1.1 >/dev/null 2>&1; then echo "ok    game reaches the internet directly (not through the VPS)"; else echo "FAIL  game has no outbound internet"; fi
gw=$(ip -4 route show default | awk '{ print $3 }')
if ip netns exec game ping -c 1 -W 2 "$gw" >/dev/null 2>&1; then echo "FAIL  game reached private address $gw"; else echo "ok    game cannot reach private address $gw"; fi
echo "-- home drop counters"
nft list table inet homewarp | grep -E 'counter packets [0-9]+ bytes [0-9]+ drop' | sed 's/^[[:space:]]*/   /'
EOF
  return $FAILED
}

# Runs "$@" while sampling the VPS; prints the average of the four busiest seconds.
with_gate_cpu() {
  local f=/dev/shm/hwspike/vm.$RANDOM$RANDOM   # one file per run: the sampler outlives the run
  gate "nohup vmstat 1 10 > $f 2>&1 </dev/null &"
  "$@"
  gate "cat $f" | awk 'NR > 3 && NF >= 17 { print $15, $13, $14, $17 }' | sort -n | head -4 |
    awk '{ id += $1; us += $2; sy += $3; st += $4; n++ }
         END { if (n) printf "   VPS cpu, busiest 4 s: user %d%%  system %d%%  steal %d%%  idle %d%%\n", us/n, sy/n, st/n, id/n }'
}

iperf_client() {  # where, host, port, extra iperf3 args...
  local where=$1 host=$2 port=$3
  shift 3
  { echo "iperf3 -c $host -p $port -t 5 -f m $* 2>&1 | grep -E 'sender|receiver|error' | sed 's/^/   /'"; } |
    if [ "$where" = home ]; then in_home; else in_client; fi
}

cmd_bench() {
  local GATE_IP SEEN
  GATE_IP=$(gate_ip)
  SEEN=$(gate "bash -s" <<'EOF'
D=/dev/shm/hwspike
"$D/ld-musl-x86_64.so.1" "$D/wg" show hwspike0 endpoints | awk '{ sub(/:[0-9]+$/, "", $2); print $2 }'
EOF
)
  { vars DIRECT SEEN; cat <<'EOF'; } | gate "bash -s"
set -eu
D=/dev/shm/hwspike
pgrep -f "$D/iperf3" >/dev/null || "$D/ld-musl-x86_64.so.1" --library-path "$D" "$D/iperf3" -s -p "$DIRECT" -D
for proto in tcp udp; do
  for from in "-i hwspike0" "-s $SEEN"; do
    iptables -C ufw-user-input $from -p $proto --dport "$DIRECT" -m comment --comment hwspike -j ACCEPT >/dev/null 2>&1 ||
      iptables -I ufw-user-input 1 $from -p $proto --dport "$DIRECT" -m comment --comment hwspike -j ACCEPT
  done
done
EOF

  echo "== round trip home -> VPS"
  { vars GATE_IP; cat <<'EOF'; } | in_home
ping -c 10 -q "$GATE_IP" | tail -1 | sed 's/^/   direct  /'
ping -c 10 -q 10.213.77.1 | tail -1 | sed 's/^/   tunnel  /'
EOF

  echo "== throughput home <-> VPS, same two machines, without and with the tunnel (TCP, 5 s)"
  echo " direct, home -> VPS";        with_gate_cpu iperf_client client "$GATE_IP" "$DIRECT"
  echo " direct, VPS -> home";        with_gate_cpu iperf_client client "$GATE_IP" "$DIRECT" -R
  echo " tunnel, home -> VPS";        with_gate_cpu iperf_client home 10.213.77.1 "$DIRECT"
  echo " tunnel, VPS -> home";        with_gate_cpu iperf_client home 10.213.77.1 "$DIRECT" -R

  echo "== UDP loss at 10 Mbit/s, home -> VPS, without and with the tunnel"
  echo " direct";                     iperf_client client "$GATE_IP" "$DIRECT" -u -b 10M
  echo " tunnel";                     iperf_client home 10.213.77.1 "$DIRECT" -u -b 10M

  echo "== full player path: client -> VPS -> tunnel -> game (leaves and re-enters the home line)"
  echo " tcp";                        with_gate_cpu iperf_client client "$GATE_IP" "$IPERF"
  echo " udp, 10 Mbit/s";             with_gate_cpu iperf_client client "$GATE_IP" "$IPERF" -u -b 10M
  echo " udp, 2 Mbit/s";              iperf_client client "$GATE_IP" "$IPERF" -u -b 2M

  echo "== traffic the spike has used on the VPS"
  gate "bash -s" <<'EOF'
D=/dev/shm/hwspike
"$D/ld-musl-x86_64.so.1" "$D/wg" show hwspike0 transfer | awk '{ printf "   tunnel: %.0f MB received, %.0f MB sent\n", $2/1e6, $3/1e6 }'
EOF
}

cmd_down() {
  echo "== VPS: remove everything the spike added"
  gate "bash -s" <<'EOF'
D=/dev/shm/hwspike
pkill -f "$D/iperf3" 2>/dev/null
nft delete table inet hwspike 2>/dev/null
iptables -L ufw-user-input -n --line-numbers | awk '/hwspike/ { print $1 }' | sort -rn | xargs -r -n1 iptables -D ufw-user-input
ip link del hwspike0 2>/dev/null
rm -rf "$D"
modprobe -r wireguard 2>/dev/null
echo "left behind: $( { ip -br link show hwspike0 2>/dev/null; nft list tables | grep hwspike; iptables -S ufw-user-input | grep hwspike; ls -d "$D" 2>/dev/null; } | wc -l) items"
EOF
  echo "== home: remove containers"
  home "docker rm -f hwspike-home hwspike-client >/dev/null 2>&1; docker ps -a --format '{{.Names}}' | grep -c hwspike || true"
}

case "${1:-}" in
  up)    cmd_up ;;
  test)  cmd_test ;;
  mark)  cmd_mark "${2:-}" ;;
  bench) cmd_bench ;;
  down)  cmd_down ;;
  *) echo "usage: $0 up | test | mark <sketch|fixed> | bench | down" >&2; exit 2 ;;
esac
