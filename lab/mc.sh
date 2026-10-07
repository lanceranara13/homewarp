#!/bin/sh
# Stands in for a Minecraft server. It answers what a game's list of servers
# asks, as the protocol frames it: nobody is on it, or one player is while the
# file `someone` is among its files. What it is sent is not read; whoever
# connects is given the same answer. Told to stop, it stops.
if [ "${1:-}" = answer ]; then
  online=0
  [ -e /home/container/someone ] && online=1
  status="{\"players\":{\"max\":20,\"online\":$online}}"
  # The packet's length, its number, the text's length, the text.
  printf "\\$(printf %o $((${#status} + 2)))\\000\\$(printf %o ${#status})%s" "$status"
  sleep 1
  exit 0
fi
trap 'exit 0' INT TERM
socat TCP4-LISTEN:"$PORT",fork,reuseaddr,backlog=64 SYSTEM:'/lab/mc.sh answer' &
echo ready
wait
