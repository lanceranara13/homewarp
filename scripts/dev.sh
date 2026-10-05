#!/usr/bin/env bash
# Dev loop (PLAN.md §10). The workstation only edits files: this syncs the working
# tree to the homelab and runs every build and test there, inside containers.
#
# Usage: dev.sh sync | check | test | fmt | run <cmd...> | lab [cmd] | du | prune
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

HOME_SSH=${HOME_SSH:-home}
REMOTE=${REMOTE:-/home/lance/homewarp}
CPUS=${CPUS:-1.5}          # the homelab has 2 vCPUs and other services to keep alive
BUILDER=homewarp-builder
ROOT=$(cd "$(dirname "$0")/.." && pwd)

home() { ssh -o BatchMode=yes -o ConnectTimeout=10 "$HOME_SSH" "$@"; }

# Tracked and untracked-but-not-ignored files, as they are on disk right now.
cmd_sync() {
  cd "$ROOT"
  git ls-files -co --exclude-standard -z |
    while IFS= read -r -d '' f; do [ -e "$f" ] && printf '%s\0' "$f"; done |
    tar --null -T - -czf - |
    home "set -e
      mkdir -p $REMOTE/data
      rm -rf $REMOTE/src.new && mkdir $REMOTE/src.new
      tar xzf - -C $REMOTE/src.new
      rm -rf $REMOTE/src && mv $REMOTE/src.new $REMOTE/src"
}

builder() {
  home "docker build -q -t $BUILDER -f $REMOTE/src/deploy/builder.Dockerfile $REMOTE/src/deploy >/dev/null"
}

# Runs "$*" in the builder as the homelab user, with the caches on named volumes.
in_builder() {
  home "docker run --rm -i --cpus $CPUS --user \$(id -u):\$(id -g) \
    -v homewarp-cargo:/cargo -v homewarp-target:/target \
    -v $REMOTE/src:/work $BUILDER bash -euc '$*'"
}

# Cargo writes the lock file on the homelab; it is committed from here.
pull_lock() {
  home "cat $REMOTE/src/Cargo.lock" > "$ROOT/Cargo.lock.new"
  mv "$ROOT/Cargo.lock.new" "$ROOT/Cargo.lock"
}

cmd_check() {
  cmd_sync && builder
  in_builder 'time cargo check --workspace --all-targets
              cargo clippy --workspace --all-targets -- -D warnings
              cargo fmt --all --check'
  pull_lock
}

cmd_test() {
  cmd_sync && builder
  in_builder 'cargo nextest run --workspace --no-tests=warn'
  pull_lock
}

cmd_fmt() {
  cmd_sync && builder
  in_builder 'cargo fmt --all'
  home "cd $REMOTE/src && find crates -name '*.rs' -print0 | tar --null -T - -czf -" | tar xzf - -C "$ROOT"
}

cmd_run() {
  cmd_sync && builder
  in_builder "$*"
}

# The simulated VPS, internet and home (lab/run.sh). HOME_FW=nftables switches
# the home side to Docker's nftables firewall backend.
cmd_lab() {
  cmd_sync
  home "cd $REMOTE/src/lab && HOME_FW=${HOME_FW:-iptables} bash run.sh ${*:-all}"
}

cmd_du() {
  home "docker run --rm -v homewarp-cargo:/cargo -v homewarp-target:/target alpine:3.20 du -sh /cargo /target
        docker image ls --format '{{.Repository}}:{{.Tag}}  {{.Size}}' | grep -E '^(homewarp|rust)' || true
        df -h / | tail -1"
}

# Drops compiled output; the downloaded crates stay.
cmd_prune() {
  home "docker volume rm homewarp-target"
}

case "${1:-}" in
  sync)  cmd_sync ;;
  check) cmd_check ;;
  test)  cmd_test ;;
  fmt)   cmd_fmt ;;
  run)   shift; cmd_run "$@" ;;
  lab)   shift; cmd_lab "$@" ;;
  du)    cmd_du ;;
  prune) cmd_prune ;;
  *) echo "usage: $0 sync | check | test | fmt | run <cmd...> | lab [cmd] | du | prune" >&2; exit 2 ;;
esac
