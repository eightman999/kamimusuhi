#!/bin/zsh
# Install the Slack gateway as a login LaunchAgent. Credentials never enter the plist.
set -euo pipefail
umask 077
LABEL=com.eightman.kamimusuhi-slack
ROOT="$HOME/Library/Application Support/Kamimusuhi/slack"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
DOMAIN="gui/$(id -u)"

if [[ "${1:-}" == "--uninstall" ]]; then
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  echo "removed $LABEL (credentials retained in $ROOT/config)"
  exit 0
fi

if [[ ! -f "$ROOT/config/slack.env" || -L "$ROOT/config/slack.env" ]]; then
  echo "Create the dedicated mode-600 config/slack.env before installation" >&2
  exit 1
fi
if [[ "$(stat -f '%u:%Lp' "$ROOT/config/slack.env")" != "$(id -u):600" ]]; then
  echo "Slack configuration must be owned by this user with mode 600" >&2
  exit 1
fi
cargo build --release --locked --manifest-path "$REPO/Cargo.toml" -p kamimusuhi-slack
mkdir -p "$ROOT/runtime/bin" "$ROOT/logs" "$(dirname "$PLIST")"
chmod 700 "$ROOT" "$ROOT/config" "$ROOT/logs"
install -m 700 "$REPO/target/release/kamimusuhi-slack" "$ROOT/runtime/bin/kamimusuhi-slack.new"
mv -f "$ROOT/runtime/bin/kamimusuhi-slack.new" "$ROOT/runtime/bin/kamimusuhi-slack"
cat > "$ROOT/runtime/bin/run-slack.sh" <<'RUN'
#!/bin/zsh
set -eu
umask 077
ROOT="${0:A:h:h:h}"
CONFIG="$ROOT/config/slack.env"
if [[ -L "$CONFIG" || "$(stat -f '%u:%Lp' "$CONFIG")" != "$(id -u):600" ]]; then
  echo "unsafe Slack configuration permissions" >&2
  exit 1
fi
set -a
. "$CONFIG"
set +a
exec "$ROOT/runtime/bin/kamimusuhi-slack"
RUN
chmod 700 "$ROOT/runtime/bin/run-slack.sh"
# plistlib escapes spaces and XML characters in paths; no credentials are embedded.
python3 - "$ROOT" "$PLIST" "$LABEL" <<'PY'
import os, plistlib, sys
root, path, label = sys.argv[1:]
value = dict(Label=label, ProgramArguments=['/bin/zsh',root+'/runtime/bin/run-slack.sh'],
 WorkingDirectory=root, RunAtLoad=True, KeepAlive=True, ThrottleInterval=15,
 ProcessType='Background', Umask=0o077,
 StandardOutPath=root+'/logs/slack.log', StandardErrorPath=root+'/logs/slack.log')
with open(path,'wb') as f: plistlib.dump(value,f)
os.chmod(path,0o600)
PY
plutil -lint "$PLIST"
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl enable "$DOMAIN/$LABEL"
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "installed $LABEL; starts automatically at login and restarts after exit"
