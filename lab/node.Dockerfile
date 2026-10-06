# Every lab party except home: the simulated VPS, the player, the NAS, and the
# game container that home's own Docker runs.
FROM alpine:3.20
RUN apk add --no-cache busybox-extras curl iperf3 iproute2 nftables openssl socat wireguard-tools-wg
COPY game.sh listen.sh /lab/
RUN chmod 755 /lab/*.sh
CMD ["sleep", "infinity"]
