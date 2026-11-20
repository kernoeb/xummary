#!/bin/sh
# Installs the latest xummary release: the app in /Applications, the CLI in ~/.local/bin.
#   curl -fsSL https://raw.githubusercontent.com/kernoeb/xummary/main/install.sh | sh
#   ... | sh -s -- --cli-only
# curl sets no quarantine flag, so macOS opens what this installs without a Gatekeeper block.
set -eu

REPO="kernoeb/xummary"
BIN_DIR="$HOME/.local/bin"
APP_NAME="Xummary.app"
APP_ZIP="Xummary-macos-arm64.zip"
CLI_TAR="xummary-cli-macos-arm64.tar.gz"

die() { echo "xummary: $*" >&2; exit 1; }

with_app=1
for arg in "$@"; do
    case "$arg" in
        --cli-only) with_app=0 ;;
        *) die "unknown option $arg (usage: install.sh [--cli-only])" ;;
    esac
done

[ "$(uname -s)" = Darwin ] || die "this release is for macOS only"
[ "$(uname -m)" = arm64 ] || die "this release is for Apple silicon only, build from source instead"

# XUMMARY_VERSION=v0.2.0 installs that tag instead of the latest one.
if [ -n "${XUMMARY_VERSION:-}" ]; then
    base="https://github.com/$REPO/releases/download/$XUMMARY_VERSION"
else
    base="https://github.com/$REPO/releases/latest/download"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fetch() {
    curl -fsSL --proto '=https' --tlsv1.2 -o "$tmp/$1" "$base/$1" || die "could not download $base/$1"
}

files="$CLI_TAR"
[ "$with_app" = 1 ] && files="$files $APP_ZIP"

fetch SHA256SUMS
for f in $files; do
    fetch "$f"
    grep "  $f\$" "$tmp/SHA256SUMS" >> "$tmp/wanted" || die "$f is missing from SHA256SUMS"
done
(cd "$tmp" && shasum -a 256 -c -s wanted) || die "checksum mismatch, nothing was installed"

tar -xzf "$tmp/$CLI_TAR" -C "$tmp"
mkdir -p "$BIN_DIR"
# Copying over a signed binary in place makes macOS kill it at launch.
rm -f "$BIN_DIR/xummary"
cp "$tmp/xummary" "$BIN_DIR/xummary"
echo "installed $BIN_DIR/xummary ($("$BIN_DIR/xummary" --version))"

if [ "$with_app" = 1 ]; then
    apps="/Applications"
    [ -w "$apps" ] || apps="$HOME/Applications"
    mkdir -p "$apps"
    if pgrep -x Xummary > /dev/null; then
        osascript -e 'quit app "Xummary"' > /dev/null 2>&1 || true
        sleep 1
    fi
    ditto -x -k "$tmp/$APP_ZIP" "$tmp/app"
    rm -rf "${apps:?}/$APP_NAME"
    ditto "$tmp/app/$APP_NAME" "$apps/$APP_NAME"
    echo "installed $apps/$APP_NAME"
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "note: $BIN_DIR is not on your PATH, add it to run xummary from a terminal" ;;
esac
command -v claude > /dev/null 2>&1 || echo "note: xummary needs the claude CLI, logged in: https://claude.com/claude-code"
