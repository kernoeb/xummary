#!/usr/bin/env bash
# Builds Xummary.app, with the `xummary` CLI embedded so the app is self-contained.
set -euo pipefail
cd "$(dirname "$0")"

APP="Xummary.app"
CLI="${XUMMARY_BIN:-$HOME/.local/bin/xummary}"

swift build -c release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/Xummary "$APP/Contents/MacOS/Xummary"

# The icon is drawn from source; ICON_VARIANT picks the mark (distil, stack, bold).
ICONSET="$(mktemp -d)/AppIcon.iconset"
swift tools/make-icon.swift "$ICONSET" "${ICON_VARIANT:-distil}" >/dev/null
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

if [ -x "$CLI" ]; then
    cp "$CLI" "$APP/Contents/Resources/xummary"
    echo "embedded CLI from $CLI"
else
    echo "warning: no xummary CLI at $CLI — the app will look in ~/.local/bin at runtime" >&2
fi

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Xummary</string>
    <key>CFBundleDisplayName</key><string>Xummary</string>
    <key>CFBundleIdentifier</key><string>com.kernoeb.xummary</string>
    <key>CFBundleExecutable</key><string>Xummary</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticTermination</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc signature: enough to run locally, not enough to hand to anyone else.
if ! codesign --force --sign - "$APP"; then
    echo "warning: ad-hoc signing failed — macOS may refuse to launch the app" >&2
fi

echo "built $PWD/$APP"
