# The home server: a real Docker daemon, plus the tools Core will drive.
FROM docker:29-dind
RUN apk add --no-cache curl iproute2 nftables socat wireguard-tools-wg
COPY listen.sh /lab/
RUN chmod 755 /lab/*.sh
