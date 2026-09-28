#!/usr/bin/env bash
# Install the computer-use MCP (computer-mcp.py + a private Xvfb display and
# xdotool/scrot backend) as the non-root resident service user on Linux.
#
#   deploy/resident/install-computer-mcp.sh [--check] [--prefix DIRECTORY] [--refresh]
#
# Like install-chrome-web-mcp.sh, X dependencies are downloaded with
# `apt-get download` and extracted under the prefix — no apt install, no
# sudo, no dpkg database change, no service restart. The launcher it writes
# at $PREFIX/bin/computer-mcp is what a resident `mcp_servers[]` entry
# points at; it spawns and owns a persistent Xvfb on :99 at first use.
set -euo pipefail

PREFIX=/srv/kamimusuhi/mcp/computer
CHECK_ONLY=false
REFRESH=false
SRC="$(cd "$(dirname "$0")/../.." && pwd)"
usage() {
  cat <<'USAGE'
Usage: install-computer-mcp.sh [--check] [--prefix DIRECTORY] [--refresh]
Run as the resident service user on Linux. --check only inspects
prerequisites. Package payloads come from existing apt metadata
(apt-get download + dpkg-deb -x); nothing is installed system-wide and no
service is restarted. --refresh re-downloads the X packages.
USAGE
}
while (($#)); do
  case "$1" in
    --check) CHECK_ONLY=true; shift ;;
    --refresh) REFRESH=true; shift ;;
    --prefix) [[ $# -ge 2 ]] || { usage >&2; exit 64; }; PREFIX=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 64 ;;
  esac
done
[[ "$PREFIX" == /* && "$PREFIX" != / ]] || { echo 'prefix must be an absolute directory other than /' >&2; exit 64; }
[[ "$(uname -s)" == Linux ]] || { echo 'install-computer-mcp.sh is for Linux nodes (macOS uses install-mac.sh).' >&2; exit 1; }
[[ "$(id -u)" != 0 ]] || { echo 'Run as the non-root resident service user.' >&2; exit 1; }
[[ -f "$SRC/deploy/resident/computer-mcp.py" ]] || { echo "missing $SRC/deploy/resident/computer-mcp.py" >&2; exit 1; }

# pick_package: print the first name that has an apt candidate.
pick_package() {
  local name version
  for name in "$@"; do
    version=$(apt-cache policy "$name" 2>/dev/null | awk '/Candidate:/ {print $2; exit}')
    if [[ -n "$version" && "$version" != '(none)' ]]; then
      printf '%s=%s' "$name" "$version"
      return 0
    fi
  done
  return 1
}

missing=0
for dependency in python3 apt-get apt-cache dpkg-deb ldd sha256sum; do
  if ! command -v "$dependency" >/dev/null; then
    echo "Missing prerequisite: $dependency" >&2
    missing=1
  fi
done
if command -v python3 >/dev/null; then
  python3 -c 'import sys; assert sys.version_info >= (3, 9), "Python 3.9+ required"' || missing=1
fi
(( missing == 0 )) || exit 1

SPECS=()
xvfb_spec=$(pick_package xvfb) || { echo 'No apt candidate: xvfb' >&2; exit 1; }
SPECS+=("$xvfb_spec")
xvfb_version=${xvfb_spec#*=}
if apt-cache show "xserver-common=$xvfb_version" >/dev/null 2>&1; then
  SPECS+=("xserver-common=$xvfb_version")
else
  echo "No matching xserver-common for xvfb $xvfb_version" >&2
  exit 1
fi
for alternatives in 'x11-xkb-utils' 'x11-utils' 'xdotool' 'libxdo3 libxdo3t64' 'wmctrl' 'scrot' 'libimlib2 libimlib2t64'; do
  if spec=$(pick_package $alternatives); then
    SPECS+=("$spec")
  else
    echo "No apt candidate for any of: $alternatives" >&2
    exit 1
  fi
done
# xdpyinfo links libXxf86dga even on headless hosts; extract it locally when
# the system does not provide it (same handling as install-chrome-web-mcp.sh).
if ! python3 -c 'import ctypes; ctypes.CDLL("libXxf86dga.so.1")' >/dev/null 2>&1; then
  if spec=$(pick_package libxxf86dga1 libxxf86dga1t64); then
    SPECS+=("$spec")
  else
    echo 'No apt candidate: libxxf86dga1 (needed by xdpyinfo)' >&2
    exit 1
  fi
fi
printf 'Prerequisites PASS. Packages: %s\n' "${SPECS[*]}"
$CHECK_ONLY && exit 0

umask 077
DEPS="$PREFIX/dependencies"
BIN="$PREFIX/bin"
mkdir -p "$DEPS/debs" "$BIN" "$PREFIX/state"

marker="$DEPS/INSTALLED"
wanted=$(printf '%s\n' "${SPECS[@]}" | sort)
if $REFRESH || [[ ! -x "$DEPS/usr/bin/Xvfb" ]] || [[ ! -f "$marker" ]] \
   || [[ "$(cat "$marker")" != "$wanted" ]]; then
  rm -f "$DEPS/debs"/*.deb 2>/dev/null || true
  (
    cd "$DEPS/debs"
    apt-get download "${SPECS[@]}"
    sha256sum ./*.deb > SHA256SUMS
  )
  for archive in "$DEPS"/debs/*.deb; do dpkg-deb -x "$archive" "$DEPS"; done
  printf '%s\n' "$wanted" > "$marker"
fi

multiarch=$(python3 -c 'import sysconfig; print(sysconfig.get_config_var("MULTIARCH") or "")')
libdirs=()
for d in "$DEPS/usr/lib/$multiarch" "$DEPS/lib/$multiarch" "$DEPS/usr/lib" "$DEPS/lib"; do
  [[ -d "$d" ]] && libdirs+=("$d")
done
ld_path=$(IFS=:; printf '%s' "${libdirs[*]}")

unresolved=0
for exe in Xvfb xdotool scrot wmctrl xdpyinfo xkbcomp; do
  for base in "$DEPS/usr/bin" "$DEPS/usr/sbin" "$DEPS/bin"; do
    candidate="$base/$exe"
    [[ -x "$candidate" ]] || continue
    if LD_LIBRARY_PATH="$ld_path" ldd "$candidate" 2>/dev/null | grep -q 'not found'; then
      echo "$candidate: unresolved shared libraries:" >&2
      LD_LIBRARY_PATH="$ld_path" ldd "$candidate" | grep 'not found' >&2
      unresolved=1
    fi
  done
done
[[ "$unresolved" == 0 ]] || { echo 'Local X binaries have missing libraries; no system change was made.' >&2; exit 1; }

install -m 644 "$SRC/deploy/resident/computer-mcp.py" "$PREFIX/computer-mcp.py"

# Launcher: dependencies on PATH, local libs for ldd resolution, a dedicated
# state dir, and the managed display default. Exclusive-create a temp file
# then atomically replace, like the chrome-web launcher.
launcher="$BIN/computer-mcp"
imlib_loaders=$(IFS=:; printf '%s' "${libdirs[*]/%//imlib2/loaders}")
text=$(cat <<EOF
#!/usr/bin/env bash
set -euo pipefail
export PATH="$DEPS/usr/bin:\$PATH"
export LD_LIBRARY_PATH="$ld_path\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
export IMLIB2_LOADER_PATH="$imlib_loaders"
export COMPUTER_XVFB="$DEPS/usr/bin/Xvfb"
export COMPUTER_DIR="$PREFIX/state"
export COMPUTER_DISPLAY="\${COMPUTER_DISPLAY:-:99}"
exec python3 "$PREFIX/computer-mcp.py" "\$@"
EOF
)
if [[ -f "$launcher" && "$(cat "$launcher")" != "$text" ]]; then
  echo "$launcher differs; replacing with the new launcher." >&2
fi
tmp="$BIN/.computer-mcp.$$"
printf '%s\n' "$text" > "$tmp"
chmod 700 "$tmp"
mv -f "$tmp" "$launcher"

# Smoke test: initialize + health_check starts the managed Xvfb. The
# resident runs under systemd PrivateTmp, so it cannot see this Xvfb's
# socket — stop it afterwards and let the resident spawn its own inside its
# namespace on the first computer_* call.
if ! printf '%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"health_check","arguments":{}}}' \
    | "$launcher" | grep -q '"platform": "linux"'; then
  echo 'computer-mcp smoke test failed (health_check did not report linux).' >&2
  exit 1
fi
if [[ -f "$PREFIX/state/xvfb.pid" ]]; then
  kill "$(cat "$PREFIX/state/xvfb.pid")" 2>/dev/null || true
  rm -f "$PREFIX/state/xvfb.pid"
  echo 'Smoke test passed; stopped the test Xvfb (resident will spawn its own).'
fi
echo "Installed: $launcher"
echo 'Resident/runtime configuration and service state were not changed.'
