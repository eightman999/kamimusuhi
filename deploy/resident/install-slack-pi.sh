#!/usr/bin/env bash
# Install a native Pi Slack binary; never modifies resident/runtime/individual state.
# Usage: install-slack-pi.sh /absolute/path/to/kamimusuhi-slack [--prepare-only]
set -euo pipefail
umask 077
BINARY="${1:?native Slack binary path required}"
MODE="${2:-}"
ROOT=/srv/kamimusuhi
SOURCE_DIR="$(cd "$(dirname "$0")" && pwd)"
[[ -x "$BINARY" && -f "$SOURCE_DIR/kamimusuhi-slack.service" ]]
[[ -f "$ROOT/config/slack.env" && ! -L "$ROOT/config/slack.env" ]]
[[ "$(stat -c '%u:%a' "$ROOT/config/slack.env")" == "$(id -u):600" ]] || {
  echo 'slack.env must belong to the service user and have mode 600' >&2; exit 1;
}
systemctl is-active --quiet kamimusuhi-resident.service
[[ -z "$MODE" || "$MODE" == --prepare-only ]]
BACKUP="$ROOT/backups/slack-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -m 700 "$BACKUP"
for path in "$ROOT/runtime/bin/kamimusuhi-slack" /etc/systemd/system/kamimusuhi-slack.service; do
  if [[ -e "$path" ]]; then sudo -n cp -p "$path" "$BACKUP/$(basename "$path")"; fi
done
install -m 755 "$BINARY" "$ROOT/runtime/bin/kamimusuhi-slack.new"
mv -f "$ROOT/runtime/bin/kamimusuhi-slack.new" "$ROOT/runtime/bin/kamimusuhi-slack"
sudo -n install -m 644 "$SOURCE_DIR/kamimusuhi-slack.service" /etc/systemd/system/kamimusuhi-slack.service
sudo -n systemctl daemon-reload
sudo -n systemd-analyze verify /etc/systemd/system/kamimusuhi-slack.service
if [[ "$MODE" != --prepare-only ]]; then
  sudo -n systemctl enable --now kamimusuhi-slack.service
fi
printf 'Slack gateway installed; mode=%s; backup=%s\n' "${MODE:-enabled}" "$BACKUP"
