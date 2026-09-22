#!/usr/bin/env bash
# Install/upgrade the resident on this node as the service user (no sudo).
# Run from a source checkout after install-root.sh has been run once:
#
#   deploy/resident/install-node.sh <pi|llm_master>
#
# Builds release binaries, installs them under /srv/kamimusuhi/runtime/bin,
# installs the node config (keeping an existing one unless --force-config),
# and on the Pi initializes the individual ONLY if no canonical store exists.
set -euo pipefail

NODE="${1:?usage: $0 <pi|llm_master> [--force-config]}"
FORCE_CONFIG="${2:-}"
ROOT=/srv/kamimusuhi
SRC="$(cd "$(dirname "$0")/../.." && pwd)"
case "$NODE" in pi|llm_master) ;; *) echo "node must be pi or llm_master" >&2; exit 64;; esac
[[ -w "$ROOT/runtime/bin" ]] || { echo "$ROOT not prepared; run install-root.sh first" >&2; exit 1; }

# shellcheck disable=SC1091
[[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
cd "$SRC"
echo "==> build (release)"
bins=(--bin kamimusuhi)
[[ "$NODE" == pi ]] && bins+=(--bin k-core --bin kamimusuhi-runtime)
pkgs=(-p kamimusuhi-resident)
[[ "$NODE" == pi ]] && pkgs+=(-p kamimusuhi-runtime)
cargo build --release "${pkgs[@]}" "${bins[@]}"

echo "==> binaries"
for b in kamimusuhi k-core kamimusuhi-runtime; do
  [[ -x "target/release/$b" ]] || continue
  install -m 755 "target/release/$b" "$ROOT/runtime/bin/$b.new"
  mv -f "$ROOT/runtime/bin/$b.new" "$ROOT/runtime/bin/$b"
done
for script in obsidian-git.sh; do
  install -m 755 "deploy/resident/$script" "$ROOT/runtime/bin/$script"
done
install -d "$ROOT/runtime/deploy"
cp -f deploy/resident/README.md "$ROOT/runtime/deploy/README.md" 2>/dev/null || true

echo "==> config"
if [[ ! -f "$ROOT/config/resident.json" || "$FORCE_CONFIG" == --force-config ]]; then
  install -m 644 "deploy/resident/$NODE.resident.json" "$ROOT/config/resident.json"
fi
"$ROOT/runtime/bin/kamimusuhi" check-config --config "$ROOT/config/resident.json"
if [[ ! -f "$ROOT/config/secrets.env" ]]; then
  echo "    WARNING: $ROOT/config/secrets.env missing (push it with push-secrets.sh)"
fi

if [[ "$NODE" == pi ]]; then
  IND="$ROOT/runtime/individual"
  if [[ -e "$IND/kamimusuhi.sqlite" || -e "$IND/runtime.json" ]]; then
    echo "==> individual exists at $IND (never reinitialized)"
  else
    echo "==> initializing NEW individual at $IND"
    "$ROOT/runtime/bin/kamimusuhi-runtime" init --dir "$IND"
  fi
fi

echo "==> restart"
if systemctl is-enabled --quiet kamimusuhi-resident.service 2>/dev/null; then
  sudo -n systemctl restart kamimusuhi-resident.service 2>/dev/null \
    || echo "    (no passwordless sudo) restart manually: sudo systemctl restart kamimusuhi-resident"
fi
echo "done"
