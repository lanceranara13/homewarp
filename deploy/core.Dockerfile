# Core's image for the staging deployment on the homelab. `scripts/dev.sh deploy`
# compiles the binary in the builder container, which is the same Debian release
# as this, and builds this image with that binary as its whole context.
FROM debian:bookworm-slim
COPY homewarp /usr/local/bin/homewarp
ENV HOMEWARP_DATA=/data HOMEWARP_LISTEN=0.0.0.0:3600
EXPOSE 3600
ENTRYPOINT ["/usr/local/bin/homewarp"]
