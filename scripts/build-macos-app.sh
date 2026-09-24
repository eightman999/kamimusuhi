#!/usr/bin/env bash
# Build the resident dialogue surface as a double-clickable macOS app.
#
#   scripts/build-macos-app.sh            # -> target/macos/Kamimusuhi.app
#   scripts/build-macos-app.sh --install  # also copy to ~/Applications
#
# The app runs kamimusuhi-desktop in its default (resident) mode. Connection
# settings come from the same places as the CLI: $KAMIMUSUHI_URL is not set
# for Finder-launched apps, so ~/.config/kamimusuhi/nodes and
# ~/.config/kamimusuhi/node_token are what the app actually reads.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

install=false
for arg in "$@"; do
  case "$arg" in
    --install) install=true ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

[[ "$(uname -s)" == Darwin ]] || { echo "macOS only" >&2; exit 1; }

NAME="Kamimusuhi"
BUNDLE_ID="dev.eightman.kamimusuhi.desktop"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
OUT="$ROOT/target/macos"
APP="$OUT/$NAME.app"

cargo build --release -p kamimusuhi-desktop

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
install -m 755 target/release/kamimusuhi-desktop "$APP/Contents/MacOS/kamimusuhi-desktop"

cat >"$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>澪 (Kamimusuhi)</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>kamimusuhi-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
</dict>
</plist>
PLIST

# Icon: rendered locally with AppKit when swift is available; optional.
if command -v swift >/dev/null 2>&1; then
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  if swift "$ROOT/scripts/macos-app-icon.swift" "$work/AppIcon.iconset" >/dev/null 2>&1 \
     && iconutil -c icns "$work/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"; then
    :
  else
    echo "note: icon generation skipped" >&2
  fi
fi

# Ad-hoc signature so Gatekeeper treats the locally built bundle as one unit.
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "note: ad-hoc codesign failed" >&2

echo "built $APP"

if $install; then
  dest="$HOME/Applications/$NAME.app"
  mkdir -p "$HOME/Applications"
  rm -rf "$dest"
  cp -R "$APP" "$dest"
  echo "installed $dest"
fi
