# Core's image for the staging deployment on the homelab. `scripts/dev.sh deploy`
# compiles the binary in the builder container, which is the same Debian release
# as this, and builds this image with that binary as its whole context.
FROM debian:bookworm-slim
# Core sets up its end of the tunnel with these two, and with nothing else.
RUN apt-get update \
 && apt-get install -y --no-install-recommends iproute2 nftables \
 && rm -rf /var/lib/apt/lists/*
COPY homewarp /usr/local/bin/homewarp
ENV HOMEWARP_DATA=/data HOMEWARP_LISTEN=0.0.0.0:3600
EXPOSE 3600
ENTRYPOINT ["/usr/local/bin/homewarp"]
