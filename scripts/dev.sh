#!/usr/bin/env bash
# Dev loop (PLAN.md §10). The workstation only edits files: this syncs the working
# tree to the homelab and runs every build and test there, inside containers.
#
# Usage: dev.sh sync | check | test | fmt | gen | npm <args...> | build | gate | deploy
#               | scratch [down] | run <cmd...> | lab [cmd] | vps [leave] | paper [clean]
#               | du | prune
set -euo pipefail
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'

HOME_SSH=${HOME_SSH:-home}
REMOTE=${REMOTE:-/home/lance/homewarp}
CPUS=${CPUS:-1.5}          # the homelab has 2 vCPUs and other services to keep alive
BUILDER=homewarp-builder
NODE=node:24-alpine
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

# Runs "$*" in web/ inside a Node container, as the homelab user. node_modules
# and npm's cache live on named volumes, which start out owned by root.
in_node() {
  home "docker run --rm -v homewarp-node-modules:/work/web/node_modules -v homewarp-npm:/npm $NODE \
      chown \$(id -u):\$(id -g) /work/web/node_modules /npm
    docker run --rm -i --cpus $CPUS --user \$(id -u):\$(id -g) \
      -e HOME=/tmp -e npm_config_cache=/npm -e npm_config_update_notifier=false \
      -e npm_config_fund=false -e npm_config_audit=false \
      -v $REMOTE/src:/work -v homewarp-node-modules:/work/web/node_modules -v homewarp-npm:/npm \
      -w /work/web $NODE sh -euc '$*'"
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
  in_node 'npm install --no-save && npm run typecheck && npm run lint'
}

# Runs npm in web/ (`dev.sh npm install some-package`) and brings package.json
# and its lock file back, as `check` does for Cargo.lock.
cmd_npm() {
  cmd_sync
  in_node "npm $*"
  home "cd $REMOTE/src/web && tar czf - package.json package-lock.json" | tar xzf - -C "$ROOT/web"
}

# Builds the web interface, then Core with it inside, as the image homewarp:dev.
cmd_build() {
  cmd_sync && builder
  in_node 'npm install --no-save && npm run build'
  in_builder 'cargo build --release -p homewarp-core
              mkdir -p deploy/out && cp /target/release/homewarp deploy/out/'
  home "docker build -q -t homewarp:dev -f $REMOTE/src/deploy/core.Dockerfile $REMOTE/src/deploy/out >/dev/null"
}

# Builds, and (re)starts the staging deployment at /home/lance/homewarp
# (deploy/compose.yml), on port 3600. Servers it is running stay running.
cmd_deploy() {
  cmd_build
  home "set -e
    cd $REMOTE/src/deploy
    HOMEWARP_DATA_DIR=$REMOTE/data docker compose up -d
    for _ in \$(seq 30); do curl -fsS -o /dev/null http://127.0.0.1:3600/api/v1/health 2>/dev/null && break; sleep 1; done
    curl -fsS http://127.0.0.1:3600/api/v1/health; echo
    curl -fsS -o /dev/null -w 'page: %{http_code} %{content_type}, %{size_download} bytes\n' http://127.0.0.1:3600/
    docker logs homewarp 2>&1 | grep 'setup code' | tail -1 || true"
}

# A throwaway copy of the last build on port 3601, with data of its own, for
# trying what needs an account without touching staging's. It runs as the
# deployment does: it shares the Docker daemon, so a server made in it is a real
# container, and the machine's network, so a VPS connected in it is a real
# tunnel. There is one tunnel on a machine: connect a VPS here only while
# staging has none. `scratch down` removes the copy, the containers of its
# servers, its data and, if it had a VPS, its end of the tunnel.
cmd_scratch() {
  local data=$REMOTE/scratch
  if [ "${1:-up}" = down ]; then
    home "had=\$(docker exec homewarp-scratch sh -c 'test -e /sys/class/net/homewarp0 && echo tunnel' 2>/dev/null || true)
          docker rm -f homewarp-scratch >/dev/null 2>&1 || true
          for id in \$(ls $data/servers 2>/dev/null); do
            docker rm -f homewarp-\$id homewarp-\$id-install homewarp-\$id-chown >/dev/null 2>&1 || true
          done
          if [ -n \"\$had\" ]; then
            docker run --rm --network host --cap-drop ALL --cap-add NET_ADMIN --entrypoint sh homewarp:dev -c \
              'ip link del homewarp0; nft delete table inet homewarp; ip rule del fwmark 0x4857 lookup 4857; true' 2>/dev/null
          fi
          docker run --rm -v $REMOTE:/homewarp alpine:3.20 rm -rf /homewarp/scratch"
    return
  fi
  home "docker rm -f homewarp-scratch >/dev/null 2>&1 || true"
  home "set -e
    mkdir -p $data
    docker run -d --name homewarp-scratch --read-only --cap-drop ALL --cap-add CHOWN --cap-add DAC_OVERRIDE \
      --cap-add NET_ADMIN --security-opt no-new-privileges:true --network host \
      -e HOMEWARP_DATA=$data -e HOMEWARP_LISTEN=0.0.0.0:3601 -v $data:$data \
      -v /var/run/docker.sock:/var/run/docker.sock homewarp:dev >/dev/null
    for _ in \$(seq 30); do curl -fsS -o /dev/null http://127.0.0.1:3601/api/v1/health 2>/dev/null && break; sleep 1; done
    docker logs homewarp-scratch 2>&1 | grep -E 'setup code|cannot run servers' | tail -2"
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

# Rewrites web/openapi.json from the handlers. The web client's types come from
# that file, and a test fails for as long as it is stale.
cmd_gen() {
  cmd_sync && builder
  mkdir -p "$ROOT/web"
  in_builder 'cargo run -q -p homewarp-core -- openapi' > "$ROOT/web/openapi.json.new"
  mv "$ROOT/web/openapi.json.new" "$ROOT/web/openapi.json"
}

# Builds the Gate as a VPS will run it: one static binary that needs nothing installed,
# for x86_64 and for ARM64. Core is built the same way beside it, for the lab, whose
# home has no glibc.
cmd_gate() {
  cmd_sync && builder
  in_builder 'cargo build --release -p homewarp-gate -p homewarp-core --target x86_64-unknown-linux-musl
              cargo build --release -p homewarp-gate --target aarch64-unknown-linux-musl
              mkdir -p deploy/out && cd /target
              cp x86_64-unknown-linux-musl/release/homewarp-gate /work/deploy/out/
              cp x86_64-unknown-linux-musl/release/homewarp /work/deploy/out/homewarp-static
              cp aarch64-unknown-linux-musl/release/homewarp-gate /work/deploy/out/homewarp-gate-arm64
              ls -l /work/deploy/out | cut -d" " -f5- '
  # Nothing here is an ARM machine, so an emulator says whether that one runs at all.
  home "docker run --rm -v $REMOTE/src/deploy/out:/out:ro alpine:3.20 sh -c \
    'apk add -q --no-cache qemu-aarch64 >/dev/null 2>&1 && printf \"on ARM64: \" && qemu-aarch64 /out/homewarp-gate-arm64 version'"
}

# Puts the Gate just built on a VPS, by way of this machine: the homelab and the
# VPS need not know each other. Enrolling it is then one command, which the
# panel gives. `vps leave` takes the Gate off the VPS again, with all it made.
VPS_SSH=${VPS_SSH:-server1}
cmd_vps() {
  local vps=(ssh -o BatchMode=yes -o ConnectTimeout=15 "$VPS_SSH")
  if [ "${1:-}" = leave ]; then
    "${vps[@]}" 'if [ -x /usr/local/bin/homewarp-gate ]; then homewarp-gate leave; else echo "There is no Gate on this machine."; fi'
    return
  fi
  cmd_gate
  home "cat $REMOTE/src/deploy/out/homewarp-gate" |
    "${vps[@]}" 'cat > /usr/local/bin/homewarp-gate.new && chmod 755 /usr/local/bin/homewarp-gate.new &&
      mv /usr/local/bin/homewarp-gate.new /usr/local/bin/homewarp-gate && homewarp-gate version'
}

# The simulated VPS, internet and home (lab/run.sh), with the Gate just built. HOME_FW=nftables switches
# the home side to Docker's nftables firewall backend.
cmd_lab() {
  cmd_gate
  home "cd $REMOTE/src/lab && HOME_FW=${HOME_FW:-iptables} bash run.sh ${*:-all}"
}

# Phase 0 runtime spike (crates/homewarp-runtime/examples/paper.rs): the real
# Paper egg from start to finish, on the homelab's own Docker. The runner is
# root with the Docker socket, as Core will be. HOMEWARP_ACCEPT_EULA=1 agrees to
# Mojang's EULA (https://aka.ms/MinecraftEULA) for this one test server; without
# it the server stops and asks. `paper clean` removes everything but the images.
EGG_URL=https://raw.githubusercontent.com/pelican-eggs/minecraft/refs/heads/main/java/paper/egg-paper.yaml
cmd_paper() {
  local spike=$REMOTE/data/spike
  if [ "${1:-}" = clean ]; then
    home "docker rm -f homewarp-spike-paper homewarp-spike-paper-install homewarp-spike-paper-chown >/dev/null 2>&1
          docker network rm homewarp-br >/dev/null 2>&1
          docker run --rm -v $REMOTE/data:/data alpine:3.20 rm -rf /data/spike"
    return
  fi
  cmd_sync && builder
  in_builder 'cargo build -q -p homewarp-runtime --example paper'
  home "set -e
    mkdir -p $spike
    curl -fsSL -o $spike/egg-paper.yaml $EGG_URL
    docker run --rm --network host \
      -v /var/run/docker.sock:/var/run/docker.sock \
      -v homewarp-target:/target:ro -v $spike:$spike \
      -e HOMEWARP_DATA=$spike -e HOMEWARP_EGG=$spike/egg-paper.yaml \
      -e HOMEWARP_PORT=25600 -e HOMEWARP_MEMORY=2048 \
      -e HOMEWARP_USER_AGENT='homewarp/0.0.0 (https://github.com/lanceranara13/homewarp)' \
      -e HOMEWARP_ACCEPT_EULA=${HOMEWARP_ACCEPT_EULA:-0} \
      $BUILDER /target/debug/examples/paper"
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
  gen)   cmd_gen ;;
  npm)   shift; cmd_npm "$@" ;;
  build) cmd_build ;;
  gate)  cmd_gate ;;
  deploy) cmd_deploy ;;
  scratch) shift; cmd_scratch "$@" ;;
  lab)   shift; cmd_lab "$@" ;;
  vps)   shift; cmd_vps "$@" ;;
  paper) shift; cmd_paper "$@" ;;
  du)    cmd_du ;;
  prune) cmd_prune ;;
  *) echo "usage: $0 sync | check | test | fmt | gen | npm <args...> | build | gate | deploy | scratch [down] | run <cmd...> | lab [cmd] | vps [leave] | paper [clean] | du | prune" >&2; exit 2 ;;
esac
