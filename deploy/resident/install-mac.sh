#!/bin/zsh
# Install (or update) a Kamimusuhi resident node on this Mac as a
# LaunchAgent. The Mac node is a cognition node for the task plane: it runs
# the agent harnesses installed here (Devin / OpenCode / Command Code) and
# never holds the individual's canonical state.
#
#   deploy/resident/install-mac.sh            build, install, (re)start
#   deploy/resident/install-mac.sh --uninstall
#
# Idempotent. Existing config/resident.json and config/agent-executors.json
# are kept; the node token is read at start from
# ~/.config/kamimusuhi/node_token and never written anywhere else.
set -euo pipefail

LABEL=com.eightman.kamimusuhi-resident
ROOT="${KAMIMUSUHI_MAC_ROOT:-$HOME/Library/Application Support/Kamimusuhi/node}"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
DOMAIN="gui/$(id -u)"

if [[ "${1:-}" == "--uninstall" ]]; then
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL (data kept in $ROOT)"
  exit 0
fi

echo "building kamimusuhi (release) from $REPO"
cargo build --release --manifest-path "$REPO/Cargo.toml" -p kamimusuhi-resident

mkdir -p "$ROOT"/{runtime/bin,config,current_state,cache,spool,logs,worktrees}
install -m 755 "$REPO/target/release/kamimusuhi" "$ROOT/runtime/bin/kamimusuhi.new"
mv -f "$ROOT/runtime/bin/kamimusuhi.new" "$ROOT/runtime/bin/kamimusuhi"

if [[ ! -f "$ROOT/config/resident.json" ]]; then
  # Site-specific hosts live in deploy/resident/local/ (not in git).
  TEMPLATE="$REPO/deploy/resident/local/mac.resident.json"
  [[ -f "$TEMPLATE" ]] || TEMPLATE="$REPO/deploy/resident/mac.resident.example.json"
  sed -e "s|@ROOT@|$ROOT|g" -e "s|@HOME@|$HOME|g" "$TEMPLATE" > "$ROOT/config/resident.json"
  echo "wrote $ROOT/config/resident.json"
fi
if [[ ! -f "$ROOT/config/agent-executors.json" ]]; then
  cp "$REPO/deploy/resident/agent-executors.example.json" "$ROOT/config/agent-executors.json"
  echo "wrote $ROOT/config/agent-executors.json"
fi
"$ROOT/runtime/bin/kamimusuhi" check-config --config "$ROOT/config/resident.json"

cat > "$ROOT/runtime/bin/run-resident.sh" <<'RUN'
#!/bin/zsh
# LaunchAgent entry point: harness binaries on PATH, node token from its
# mode-600 file (value never logged), then the resident.
set -eu
ROOT="${0:A:h:h:h}"
export PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
if [[ -r "$HOME/.config/kamimusuhi/node_token" ]]; then
  export KAMIMUSUHI_NODE_TOKEN="$(tr -d '[:space:]' < "$HOME/.config/kamimusuhi/node_token")"
fi
exec "$ROOT/runtime/bin/kamimusuhi" serve --config "$ROOT/config/resident.json"
RUN
chmod 755 "$ROOT/runtime/bin/run-resident.sh"

mkdir -p "$(dirname "$PLIST")"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>/bin/zsh</string><string>$ROOT/runtime/bin/run-resident.sh</string></array>
  <key>WorkingDirectory</key><string>$ROOT</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>$ROOT/logs/resident.log</string>
  <key>StandardErrorPath</key><string>$ROOT/logs/resident.log</string>
</dict>
</plist>
PLIST

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "started $LABEL"
for _ in {1..30}; do
  if curl -fsS -m 2 http://127.0.0.1:7860/health >/dev/null 2>&1; then
    curl -fsS http://127.0.0.1:7860/health; echo
    exit 0
  fi
  sleep 1
done
echo "resident did not answer on :7860; see $ROOT/logs/resident.log" >&2
exit 1
