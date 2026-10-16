#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# build-app.sh - build AZ-EMU.app (macOS) from source: az-preview, packaged as a
# double-clickable app with a generated icon. Needs Rust (rustup) and Xcode's
# command line tools (swift, sips, iconutil).
# Launches the whole emulator with no terminal commands: az-preview itself
# spawns QEMU headless (see start_if_needed in az-preview/src/boot.rs) and shows the
# screen-only view by default.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
command -v cargo >/dev/null || { echo "Rust is not installed: https://rustup.rs" >&2; exit 1; }

cargo build --release -p az-preview

APP="$REPO/AZ-EMU.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/az-preview "$APP/Contents/MacOS/AZ-EMU"

# Icon: regenerate from tools/icon/make_icon.swift (native Swift/AppKit
# drawing, no image-editing deps) each build so it never goes stale.
ICONSET="$REPO/target/AppIcon.iconset"
rm -rf "$ICONSET"
mkdir -p "$ICONSET"
swift "$REPO/tools/icon/make_icon.swift" "$REPO/target/icon-1024.png"
for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" "64 icon_32x32@2x" \
            "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" \
            "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
    set -- $spec
    sips -z "$1" "$1" "$REPO/target/icon-1024.png" --out "$ICONSET/$2.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>AZ-EMU</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.kyle-hosman.xdj-az-emu</string>
    <key>CFBundleName</key>
    <string>AZ-EMU</string>
    <key>CFBundleDisplayName</key>
    <string>AZ-EMU</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# Ad-hoc code-sign: an unsigned bundle can show its correct icon in Finder
# while closed but fall back to a generic icon in the Dock once it's an
# actually-running process: macOS's live-process icon resolution is
# pickier about signing than the static Finder lookup is.
codesign --force --deep -s - "$APP" 2>/dev/null || true

echo "Built: $APP"
echo "Open it: open '$APP'"
