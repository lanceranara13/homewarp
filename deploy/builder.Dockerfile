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
# And for ARM64, which the cheapest VPS tiers often are, and which a home
# machine may be as well. The compiler is the cross one, made to read musl's
# own headers and the kernel's, and none of glibc's: what is compiled against
# one C library's idea of a file or a lock and linked with another's is wrong
# in ways that show late. SQLite, compiled the other way, did not link at all.
RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends musl-dev:arm64 \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /opt/arm64-kernel-headers \
 && for part in linux asm asm-generic; do ln -s /usr/aarch64-linux-gnu/include/$part /opt/arm64-kernel-headers/$part; done \
 && printf '%s\n' '#!/bin/sh' \
      'exec aarch64-linux-gnu-gcc -nostdinc -isystem /usr/include/aarch64-linux-musl -isystem "$(aarch64-linux-gnu-gcc -print-file-name=include)" -isystem /opt/arm64-kernel-headers "$@"' \
      > /usr/local/bin/aarch64-linux-musl-gcc \
 && chmod 755 /usr/local/bin/aarch64-linux-musl-gcc
# The linker is still the cross one: Rust brings the musl it links against.
ENV CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc
WORKDIR /work
