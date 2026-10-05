#!/bin/sh
# Stands in for a game server: answers with the address it sees the player as.
socat TCP4-LISTEN:"$PORT",fork,reuseaddr SYSTEM:'echo tcp $SOCAT_PEERADDR' &
socat UDP4-RECVFROM:"$PORT",fork SYSTEM:'echo udp $SOCAT_PEERADDR' &
socat TCP4-LISTEN:"$CLOSED",fork,reuseaddr SYSTEM:'echo closed' &
exec iperf3 -s -p "$IPERF"
