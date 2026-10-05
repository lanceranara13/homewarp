# Toolchain for scripts/dev.sh. Nothing built from this image ships.
FROM rust:1-slim-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl pkg-config \
 && rm -rf /var/lib/apt/lists/*

RUN rustup component add clippy rustfmt \
 && curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C /usr/local/bin

# dev.sh mounts named volumes here and runs as the homelab user, not root. A new
# volume takes its mode from the image, so make both writable by any uid.
RUN mkdir -p /cargo /target && chmod 1777 /cargo /target
ENV CARGO_HOME=/cargo CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never HOME=/tmp
WORKDIR /work
