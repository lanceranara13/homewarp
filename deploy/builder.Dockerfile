# Toolchain for scripts/dev.sh. Nothing built from this image ships.
FROM rust:1-slim-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl musl-tools pkg-config \
      gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
 && rm -rf /var/lib/apt/lists/*

# musl, for the Gate: one static binary that asks nothing of the VPS it lands on.
RUN rustup component add clippy rustfmt \
 && rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
 && curl -LsSf https://get.nexte.st/latest/linux | tar zxf - -C /usr/local/bin

# dev.sh mounts named volumes here and runs as the homelab user, not root. A new
# volume takes its mode from the image, so make both writable by any uid.
RUN mkdir -p /cargo /target && chmod 1777 /cargo /target
ENV CARGO_HOME=/cargo CARGO_TARGET_DIR=/target CARGO_TERM_COLOR=never HOME=/tmp
# The WireGuard library carries a userspace fallback whose cryptography is partly C.
ENV CC_x86_64_unknown_linux_musl=musl-gcc
# And for ARM64, which the cheapest VPS tiers often are: a C compiler for that
# same part, which also links the binary against the musl that Rust brings.
ENV CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
WORKDIR /work
