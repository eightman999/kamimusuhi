#!/usr/bin/env bash
# One-time root setup for a Kamimusuhi resident node (Pi or llm_master).
#
#   sudo ./install-root.sh <pi|llm_master> [share-export] [kamimusuhi-export]
#
# Idempotent. It:
#   1. creates /srv/kamimusuhi/{runtime/bin,config,current_state,cache,spool}
#      owned by the service user (default: eightman);
#   2. adds NFS entries to /etc/fstab: the whole share (/mnt/nas-share) and
#      the kamimusuhi dataset (/mnt/kamimusuhi), soft/nofail/automount — a NAS
#      outage never blocks boot or hangs the daemon;
#   3. installs and enables kamimusuhi-resident.service (system unit).
# It does NOT write secrets, initialize an individual or start the service
# before a binary and config exist (install-node.sh does that as the user).
set -euo pipefail

NODE="${1:?usage: sudo $0 <pi|llm_master> [share-export] [kamimusuhi-export]}"
NAS_EXPORT="${2:-192.168.40.124:/mnt/Share}"
SERVICE_USER="${SERVICE_USER:-eightman}"
ROOT=/srv/kamimusuhi
MOUNTPOINT=/mnt/nas-share
KAMI_EXPORT="${3:-192.168.40.124:/mnt/Share/kamimusuhi}"
KAMI_MOUNTPOINT=/mnt/kamimusuhi
HERE="$(cd "$(dirname "$0")" && pwd)"

case "$NODE" in pi|llm_master) ;; *) echo "node must be pi or llm_master" >&2; exit 64;; esac
[[ $EUID -eq 0 ]] || { echo "run with sudo" >&2; exit 1; }
id "$SERVICE_USER" >/dev/null

echo "==> directories under $ROOT"
install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 755 \
  "$ROOT" "$ROOT/runtime" "$ROOT/runtime/bin" "$ROOT/cache" "$ROOT/current_state"
install -d -o "$SERVICE_USER" -g "$SERVICE_USER" -m 700 "$ROOT/config" "$ROOT/spool"

echo "==> NFS client"
if ! command -v mount.nfs >/dev/null 2>&1; then
  apt-get install -y nfs-common
fi

OPTS="nfsvers=3,soft,timeo=100,retrans=3,_netdev,nofail,noatime,x-systemd.automount,x-systemd.mount-timeout=20"
add_fstab() {  # <export> <mountpoint>
  local export="$1" mp="$2"
  echo "==> fstab: $export -> $mp"
  install -d -m 755 "$mp"
  if grep -qE "[[:space:]]$mp[[:space:]]" /etc/fstab; then
    echo "    fstab already has $mp; leaving it unchanged"
  else
    cp -a /etc/fstab "/etc/fstab.bak.kamimusuhi.$(date +%Y%m%d%H%M%S)"
    printf '# Kamimusuhi NAS (added by install-root.sh)\n%s %s nfs %s 0 0\n' "$export" "$mp" "$OPTS" >> /etc/fstab
  fi
}
# NFSv3 does not cross ZFS dataset boundaries, so the kamimusuhi dataset is
# its own export and mount; the whole share is mounted separately for browsing.
add_fstab "$NAS_EXPORT" "$MOUNTPOINT"
add_fstab "$KAMI_EXPORT" "$KAMI_MOUNTPOINT"
systemctl daemon-reload
systemctl restart remote-fs.target || true
for mp in "$MOUNTPOINT" "$KAMI_MOUNTPOINT"; do
  if timeout 25 ls "$mp" >/dev/null 2>&1; then
    echo "    reachable: $mp"
  else
    echo "    WARNING: $mp not reachable now; the resident will spool until it is"
  fi
done

echo "==> systemd unit"
install -m 644 "$HERE/kamimusuhi-resident.service" /etc/systemd/system/kamimusuhi-resident.service
if [[ "$SERVICE_USER" != eightman ]]; then
  sed -i "s/^User=eightman/User=$SERVICE_USER/; s/^Group=eightman/Group=$SERVICE_USER/" \
    /etc/systemd/system/kamimusuhi-resident.service
fi
systemctl daemon-reload
systemctl enable kamimusuhi-resident.service

echo "==> CLI on PATH"
ln -sfn "$ROOT/runtime/bin/kamimusuhi" /usr/local/bin/kamimusuhi

if [[ -x "$ROOT/runtime/bin/kamimusuhi" && -f "$ROOT/config/resident.json" ]]; then
  systemctl restart kamimusuhi-resident.service
  echo "    service (re)started"
else
  echo "    service enabled; it starts once install-node.sh has placed binary and config"
fi
echo "done ($NODE)"
