#!/usr/bin/env bash
# Install the pinned Linux-only MCP into its own venv; never apt install/restart.
set -euo pipefail

REVISION=193d6b256dbe12a3612e418d118e479ee3082d44
REPOSITORY=https://github.com/kuraneko1/chrome-web-mcp.git
PREFIX=/srv/kamimusuhi/mcp/chrome-web
CHECK_ONLY=false
LOCAL_DEPS=false
LOCAL_PIP=false
LOCAL_PACKAGE_SPECS=()
usage() {
  cat <<'USAGE'
Usage: install-chrome-web-mcp.sh [--check] [--prefix DIRECTORY] [--with-local-deps]
Run as the resident service user on Linux. --check only inspects prerequisites.
Set CW_CHROME to reuse an existing Chrome/Chromium executable.
Requires Python 3.10+ with venv/ensurepip, git, Xvfb, and xdpyinfo.
--with-local-deps downloads/extracts Ubuntu/Debian Xvfb and same-version
xserver-common, plus x11-utils/pip-wheel when absent, inside the release only.
Does not install system packages, edit resident/runtime config, or restart services.
USAGE
}
while (($#)); do
  case "$1" in
    --check) CHECK_ONLY=true; shift ;;
    --with-local-deps) LOCAL_DEPS=true; shift ;;
    --prefix) [[ $# -ge 2 ]] || { usage >&2; exit 64; }; PREFIX=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 64 ;;
  esac
done
[[ "$PREFIX" == /* && "$PREFIX" != / ]] || { echo 'prefix must be an absolute directory other than /' >&2; exit 64; }
[[ "$(uname -s)" == Linux ]] || { echo 'chrome-web-mcp requires a Linux node.' >&2; exit 1; }
[[ "$(id -u)" != 0 ]] || { echo 'Run as the non-root resident service user (Chrome sandbox stays enabled).' >&2; exit 1; }

missing=0
dependencies=(python3 git)
if $LOCAL_DEPS; then
  dependencies+=(apt-get apt-cache dpkg-deb ldd)
else
  dependencies+=(Xvfb xdpyinfo)
fi
for dependency in "${dependencies[@]}"; do
  if ! command -v "$dependency" >/dev/null; then
    echo "Missing prerequisite: $dependency" >&2
    missing=1
  fi
done
if command -v python3 >/dev/null; then
  if ! python3 - "$LOCAL_DEPS" <<'PY'
import importlib.util
import sys
if sys.version_info < (3, 10):
    raise SystemExit('Python 3.10+ is required')
for module in (('venv',) if sys.argv[1] == 'true' else ('venv', 'ensurepip')):
    if importlib.util.find_spec(module) is None:
        raise SystemExit(f'Missing Python module: {module} (Debian/Ubuntu: python3-venv)')
PY
  then
    missing=1
  fi
fi
if $LOCAL_DEPS; then
  local_packages=(xvfb)
  if ! command -v xdpyinfo >/dev/null; then
    local_packages+=(x11-utils)
    # xdpyinfo links this library even though most headless hosts do not need it.
    python3 -c 'import ctypes; ctypes.CDLL("libXxf86dga.so.1")' >/dev/null 2>&1 \
      || local_packages+=(libxxf86dga1)
  fi
  if ! python3 -c 'import ensurepip' >/dev/null 2>&1; then
    LOCAL_PIP=true
    local_packages+=(python3-pip-whl)
  fi
  for package in "${local_packages[@]}"; do
    candidate_version=$(apt-cache policy "$package" | awk '/Candidate:/ {print $2; exit}')
    if [[ -z "$candidate_version" || "$candidate_version" == '(none)' ]]; then
      echo "No configured apt candidate: $package" >&2
      missing=1
    else
      LOCAL_PACKAGE_SPECS+=("$package=$candidate_version")
      if [[ "$package" == xvfb ]]; then
        # Keep matching common data beside the extracted binary even when the
        # host's system-wide xserver-common package is an older revision.
        if apt-cache show "xserver-common=$candidate_version" >/dev/null 2>&1; then
          LOCAL_PACKAGE_SPECS+=("xserver-common=$candidate_version")
        else
          echo "No matching xserver-common for xvfb $candidate_version" >&2
          missing=1
        fi
      fi
    fi
  done
fi
CHROME=${CW_CHROME:-}
if [[ -z "$CHROME" ]]; then
  for candidate in /usr/bin/google-chrome /usr/bin/google-chrome-stable /usr/bin/chromium /usr/bin/chromium-browser; do
    if [[ -x "$candidate" ]]; then CHROME=$candidate; break; fi
  done
fi
if [[ "$CHROME" != /* || ! -x "$CHROME" ]]; then
  echo 'Chrome/Chromium not found; set CW_CHROME to an absolute executable path.' >&2
  missing=1
fi
[[ "$missing" == 0 ]] || exit 1
printf 'Prerequisites PASS; Chrome=%s; revision=%s\n' "$CHROME" "$REVISION"
if $LOCAL_DEPS; then printf 'Local dependency: %s\n' "${LOCAL_PACKAGE_SPECS[@]}"; fi
$CHECK_ONLY && exit 0

umask 077
RELEASE="$PREFIX/releases/$REVISION"
if [[ -e "$RELEASE" ]]; then
  if [[ ! -f "$RELEASE/INSTALLED" || ! -x "$RELEASE/venv/bin/python" ]]; then
    echo "Incomplete installation retained at $RELEASE; inspect it before retrying." >&2
    exit 1
  fi
  [[ "$(cat "$RELEASE/INSTALLED")" == "$REVISION" ]] || { echo 'Revision marker mismatch' >&2; exit 1; }
else
  mkdir -p "$PREFIX/releases"
  mkdir "$RELEASE"
  trap 'echo "Installation interrupted; partial files retained at $RELEASE (no service changed)." >&2' ERR
  git init -q "$RELEASE/source"
  git -C "$RELEASE/source" remote add origin "$REPOSITORY"
  git -C "$RELEASE/source" fetch --depth 1 origin "$REVISION"
  git -C "$RELEASE/source" checkout -q --detach FETCH_HEAD
  [[ "$(git -C "$RELEASE/source" rev-parse HEAD)" == "$REVISION" ]]
  if $LOCAL_DEPS; then
    # apt-get download authenticates against existing apt metadata. No apt update,
    # dpkg installation, sudo, system Python mutation, or maintainer scripts.
    mkdir -p "$RELEASE/dependencies/debs"
    (
      cd "$RELEASE/dependencies/debs"
      apt-get download "${LOCAL_PACKAGE_SPECS[@]}"
      for archive in ./*.deb; do dpkg-deb -x "$archive" ..; done
      sha256sum ./*.deb > SHA256SUMS
    )
    multiarch=$(python3 -c 'import sysconfig; print(sysconfig.get_config_var("MULTIARCH"))')
    local_library_dir="$RELEASE/dependencies/usr/lib/$multiarch"
    for executable in Xvfb xdpyinfo; do
      candidate="$RELEASE/dependencies/usr/bin/$executable"
      [[ -x "$candidate" ]] || candidate=$(command -v "$executable")
      LD_LIBRARY_PATH="$local_library_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
        ldd "$candidate" > "$RELEASE/dependencies/ldd-$executable.txt"
      if grep -q 'not found' "$RELEASE/dependencies/ldd-$executable.txt"; then
        cat "$RELEASE/dependencies/ldd-$executable.txt" >&2
        echo "$executable shared libraries are missing; no system dependency was changed." >&2
        exit 1
      fi
    done
    command -v xkbcomp >/dev/null || { echo 'Missing system xkbcomp (x11-xkb-utils)' >&2; exit 1; }
  fi
  if $LOCAL_PIP; then
    python3 -m venv --without-pip "$RELEASE/venv"
    pip_wheels=("$RELEASE"/dependencies/usr/share/python-wheels/pip-*.whl)
    [[ ${#pip_wheels[@]} == 1 && -f "${pip_wheels[0]}" ]] || { echo 'Expected exactly one packaged pip wheel' >&2; exit 1; }
    PYTHONPATH="${pip_wheels[0]}" "$RELEASE/venv/bin/python" -m pip --isolated install \
      --ignore-installed --no-index --disable-pip-version-check "${pip_wheels[0]}"
  else
    python3 -m venv "$RELEASE/venv"
  fi
  "$RELEASE/venv/bin/python" -m pip --isolated install --disable-pip-version-check "$RELEASE/source"
  "$RELEASE/venv/bin/python" -m pip check
  "$RELEASE/venv/bin/python" -m pip freeze > "$RELEASE/packages.txt"
  printf '%s\n' "$REVISION" > "$RELEASE/INSTALLED"
  trap - ERR
fi

# Use exclusive creation for config, and atomic replacement for the tiny launcher.
# The venv stays at its original absolute path (moving it breaks entry points).
python3 - "$PREFIX" "$RELEASE" "$CHROME" <<'PY'
import json
import os
from pathlib import Path
import shlex
import sys

prefix, release, chrome = sys.argv[1:]
config = Path(prefix) / 'config.json'
try:
    with config.open('x') as stream:
        json.dump({'show_browser': False, 'hl': 'ja', 'gl': 'jp', 'limit': 5,
                   'char_limit': 6000, 'format': 'markdown',
                   'min_delay': 1.0, 'max_delay': 2.5}, stream, indent=2)
        stream.write('\n')
except FileExistsError:
    pass
data = json.loads(config.read_text())
if not isinstance(data, dict) or data.get('show_browser') is not False:
    raise SystemExit(f'{config}: show_browser must be false; existing config was preserved')
bindir = Path(prefix) / 'bin'
bindir.mkdir(exist_ok=True)
launcher = bindir / 'chrome-web-mcp'
lines = [
    '#!/usr/bin/env bash', 'set -euo pipefail',
    'unset CW_PROFILE_DIR CW_LOCK_PATH',
    'export CW_DISPLAY_MODE=xvfb CW_XPRA_EXPOSE=0',
    f'export CW_CONFIG={shlex.quote(str(config))}',
    f'export CW_CHROME={shlex.quote(chrome)}',
]
local_bin = Path(release) / 'dependencies/usr/bin'
if (local_bin / 'Xvfb').is_file():
    lines.append(f'export PATH={shlex.quote(str(local_bin))}:"$PATH"')
    # Only xdpyinfo needs the optional local Xxf86dga library. Keep that loader
    # path out of the MCP/Chrome environment, preserving Chrome's sandbox setup.
    local_libs = list((Path(release) / 'dependencies/usr/lib').glob('*/libXxf86dga.so.1'))
    if local_libs:
        if len(local_libs) != 1:
            raise SystemExit('Expected one architecture of local libXxf86dga')
        wrappers = Path(release) / 'dependencies/wrappers'
        wrappers.mkdir(exist_ok=True)
        probe = wrappers / 'xdpyinfo'
        probe.write_text('\n'.join([
            '#!/usr/bin/env bash', 'set -euo pipefail',
            f'export LD_LIBRARY_PATH={shlex.quote(str(local_libs[0].parent))}',
            f'exec {shlex.quote(str(local_bin / "xdpyinfo"))} "$@"', '',
        ]))
        probe.chmod(0o700)
        lines.append(f'export PATH={shlex.quote(str(wrappers))}:"$PATH"')
lines += [f'exec {shlex.quote(str(Path(release) / "venv/bin/python"))} -m chrome_web_mcp "$@"', '']
text = '\n'.join(lines)
if launcher.exists() and launcher.read_text() != text:
    raise SystemExit(f'{launcher}: refusing to overwrite a different launcher; review the change first')
temporary = bindir / f'.chrome-web-mcp.{os.getpid()}'
with temporary.open('x') as stream:
    stream.write(text)
temporary.chmod(0o700)
temporary.replace(launcher)
print(f'Installed: {launcher}')
print(f'Dependency manifest: {Path(release) / "packages.txt"}')
PY
echo 'Resident/runtime configuration and service state were not changed.'
