#!/usr/bin/env bash
# Dev loop (PLAN.md §10). The workstation only edits files: this syncs the working
# tree to the homelab and runs every build and test there, inside containers.
#
# Usage: dev.sh sync | check | test | fmt | gen | npm <args...> | build | gate | deploy
#               | scratch [down] | run <cmd...> | lab [cmd] | vps [leave] | paper [clean]
#               | release | du | prune
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

# A throwaway copy of the last build on port 3601, SFTP on 2023, with data of its own, for
# trying what needs an account without touching staging's. It runs as the
# deployment does, behind a door of its own: it shares the Docker daemon, so a
# server made in it is a real container, and the machine's network, so a VPS
# connected in it is a real tunnel. A Homewarp leaves alone the tunnels that are
# not its own, so the copy can be started beside one that has a VPS; but the
# two number their tunnels alike, so connect a VPS here only while no other
# Homewarp on the machine has one. `scratch down` removes the copy, the
# containers of its servers, its data and, if it had a VPS, its end of the tunnel.
#
# It is served over TLS as well, behind a third door, on SCRATCH_TLS_PORT (8444):
# the port a VPS connected to it forwards once the panel is given a name. The
# certificate for that name is asked of SCRATCH_ACME, which is the staging
# authority of Lets Encrypt unless another is named: a throwaway copy has no
# business using up what the real one allows a name in a week.
SCRATCH_TLS_PORT=${SCRATCH_TLS_PORT:-8444}
SCRATCH_ACME=${SCRATCH_ACME:-https://acme-staging-v02.api.letsencrypt.org/directory}
cmd_scratch() {
  local data=$REMOTE/scratch tls=$SCRATCH_TLS_PORT
  if [ "${1:-up}" = down ]; then
    home "ours=\$(cat $data/tunnels 2>/dev/null || true)
          docker rm -f homewarp-scratch homewarp-scratch-door homewarp-scratch-sftp homewarp-scratch-tls >/dev/null 2>&1 || true
          for id in \$(ls $data/servers 2>/dev/null); do
            docker rm -f homewarp-\$id homewarp-\$id-install homewarp-\$id-chown homewarp-\$id-standin >/dev/null 2>&1 || true
          done
          if [ -n \"\$ours\" ]; then
            docker run --rm --network host --cap-drop ALL --cap-add NET_ADMIN --entrypoint sh homewarp:dev -c \
              \"for n in \$ours; do ip link del homewarp\\\$n; ip rule del fwmark \\\$((0x4857 + n)) lookup \\\$((4857 + n)); done
               ls /sys/class/net | grep -q '^homewarp[0-9]' || nft delete table inet homewarp; true\" 2>/dev/null
          fi
          docker run --rm -v $REMOTE:/homewarp alpine:3.20 rm -rf /homewarp/scratch"
    return
  fi
  home "docker rm -f homewarp-scratch homewarp-scratch-door homewarp-scratch-sftp homewarp-scratch-tls >/dev/null 2>&1 || true"
  home "set -e
    mkdir -p $data
    docker run -d --name homewarp-scratch --read-only --cap-drop ALL --cap-add CHOWN --cap-add DAC_OVERRIDE \
      --cap-add NET_ADMIN --security-opt no-new-privileges:true --network host \
      -e HOMEWARP_DATA=$data -e HOMEWARP_LISTEN=unix:$data/run/panel.sock \
      -e HOMEWARP_SFTP=unix:$data/run/sftp.sock -e HOMEWARP_SFTP_PORT=2023       -e HOMEWARP_TLS=unix:$data/run/tls.sock -e HOMEWARP_TLS_PORT=$tls -e HOMEWARP_ACME=$SCRATCH_ACME -v $data:$data \
      -v /var/run/docker.sock:/var/run/docker.sock homewarp:dev >/dev/null
    docker run -d --name homewarp-scratch-door --read-only --cap-drop ALL --security-opt no-new-privileges:true \
      -v $data/run:/run/homewarp -p 3601:3600 homewarp:dev door 0.0.0.0:3600 /run/homewarp/panel.sock >/dev/null
    docker run -d --name homewarp-scratch-sftp --read-only --cap-drop ALL --security-opt no-new-privileges:true \
      -v $data/run:/run/homewarp -p 2023:2022 homewarp:dev door 0.0.0.0:2022 /run/homewarp/sftp.sock >/dev/null
    docker run -d --name homewarp-scratch-tls --read-only --cap-drop ALL --security-opt no-new-privileges:true       -v $data/run:/run/homewarp -p $tls:$tls homewarp:dev door 0.0.0.0:$tls /run/homewarp/tls.sock >/dev/null
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

# Builds what a release holds, for both kinds of processor, and puts it together
# on the homelab in $REMOTE/release (scripts/release.sh): the programs, a signed
# list of their checksums, and the two install scripts.
#
#   RELEASES=https://example.com/homewarp bash scripts/dev.sh release
#   RELEASES=https://example.com/homewarp CHANNEL=beta bash scripts/dev.sh release
#
# RELEASES is where that folder will be served from: it is written into the
# install scripts and is what an installed Homewarp tells a VPS. CHANNEL is
# stable unless it is said to be beta: a beta is numbered as one in Cargo.toml
# (1.2.0-beta.1), and only a Homewarp that takes betas is offered it. RELEASE_KEY is
# the Ed25519 key that signs, kept on the homelab and nowhere in this
# repository ($REMOTE/release.key unless said otherwise). Whoever installs
# trusts that key and nothing else, so it is its owner's to make and to keep:
#   ssh home 'openssl genpkey -algorithm ed25519 -out /home/lance/homewarp/release.key'
cmd_release() {
  : "${RELEASES:?say where the release will be served from: RELEASES=https://...}"
  local key=${RELEASE_KEY:-$REMOTE/release.key} version public
  # The public half of the key, as Core is built with it: what an installed
  # Homewarp holds the list of releases, and a newer release, against.
  public=$(home "openssl pkey -in $key -pubout -outform DER | tail -c 32 | base64")
  [ -n "$public" ] || { echo "there is no key at $key on the homelab (docs/releasing.md)" >&2; exit 1; }
  cmd_sync && builder
  # The web interface first: Core carries it inside, as it is when Core is compiled.
  in_node 'npm install --no-save && npm run build'
  in_builder "export HOMEWARP_RELEASE_KEY=$public; "'for target in x86_64 aarch64; do
                cargo build --release -p homewarp-gate -p homewarp-core --target $target-unknown-linux-musl
              done
              mkdir -p deploy/out && cd /target
              cp x86_64-unknown-linux-musl/release/homewarp-gate /work/deploy/out/
              cp x86_64-unknown-linux-musl/release/homewarp /work/deploy/out/homewarp-static
              cp aarch64-unknown-linux-musl/release/homewarp-gate /work/deploy/out/homewarp-gate-arm64
              cp aarch64-unknown-linux-musl/release/homewarp /work/deploy/out/homewarp-static-arm64'
  # Nothing here is an ARM machine: an emulator says whether those two run, and
  # whether Core gets as far as making its database there.
  home "docker run --rm -v $REMOTE/src/deploy/out:/out:ro alpine:3.20 sh -c \
    'apk add -q --no-cache qemu-aarch64 >/dev/null 2>&1 && printf \"on ARM64: \" && qemu-aarch64 /out/homewarp-gate-arm64 version &&
     mkdir /tmp/data && printf \"on ARM64, Core: \" && HOMEWARP_DATA=/tmp/data qemu-aarch64 /out/homewarp-static-arm64 two-steps-off nobody'"
  version=$(home "$REMOTE/src/deploy/out/homewarp-gate version | cut -d' ' -f2")
  home "sh $REMOTE/src/scripts/release.sh $REMOTE/src/deploy/out $REMOTE/release $version '$RELEASES' $key ${CHANNEL:-stable}"
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
# the home side to Docker's nftables firewall backend, and GATE_FW=firewalld gives the VPS a firewall.
cmd_lab() {
  cmd_gate
  home "cd $REMOTE/src/lab && HOME_FW=${HOME_FW:-iptables} GATE_FW=${GATE_FW:-none} bash run.sh ${*:-all}"
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
  release) cmd_release ;;
  paper) shift; cmd_paper "$@" ;;
  du)    cmd_du ;;
  prune) cmd_prune ;;
  *) echo "usage: $0 sync | check | test | fmt | gen | npm <args...> | build | gate | deploy | scratch [down] | run <cmd...> | lab [cmd] | vps [leave] | paper [clean] | release | du | prune" >&2; exit 2 ;;
esac
