#!/usr/bin/env bash
# Install Netdata for the resident's Netdata MCP (run once per node with sudo).
#
#   sudo deploy/resident/install-netdata.sh
#
# Installs the stable Netdata agent (telemetry off, no auto-updates, not
# claimed to Netdata Cloud), binds its web API to localhost only, and copies
# the MCP API key into /srv/kamimusuhi/config/secrets.env as NETDATA_MCP_KEY
# without printing it. Idempotent.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "run with sudo" >&2; exit 1; }
SECRETS=/srv/kamimusuhi/config/secrets.env
SERVICE_USER="${SERVICE_USER:-eightman}"

if ! command -v netdata >/dev/null 2>&1; then
  curl -fsSL https://get.netdata.cloud/kickstart.sh -o /tmp/netdata-kickstart.sh
  sh /tmp/netdata-kickstart.sh --non-interactive --disable-telemetry --stable-channel --no-updates
fi

conf=/etc/netdata/netdata.conf
if ! grep -q '^\[web\]' "$conf" 2>/dev/null; then
  [[ -f "$conf" ]] && cp -a "$conf" "$conf.bak.kamimusuhi"
  printf '\n[web]\n    bind to = 127.0.0.1 ::1\n' >> "$conf"
fi
systemctl enable --now netdata
systemctl restart netdata

key=/var/lib/netdata/mcp_dev_preview_api_key
for _ in $(seq 1 30); do [[ -s "$key" ]] && break; sleep 1; done
[[ -s "$key" ]] || { echo "Netdata MCP key not found at $key" >&2; exit 1; }
touch "$SECRETS"
tmp="$SECRETS.new"
{ grep -v '^NETDATA_MCP_KEY=' "$SECRETS" || true; printf 'NETDATA_MCP_KEY=%s\n' "$(cat "$key")"; } > "$tmp"
chown "$SERVICE_USER:$SERVICE_USER" "$tmp"; chmod 600 "$tmp"; mv -f "$tmp" "$SECRETS"
echo "netdata $(netdata -v 2>/dev/null) on 127.0.0.1:19999; NETDATA_MCP_KEY stored"
systemctl restart kamimusuhi-resident 2>/dev/null || true
