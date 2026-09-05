#!/usr/bin/env bash
# Compiles the real parser sources against tools/markdown-tests.swift and runs them.
set -euo pipefail
cd "$(dirname "$0")/.."

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# main.swift is the only file allowed top-level code.
cp Sources/Xummary/Markdown.swift Sources/Xummary/Theme.swift "$work/"
cp tools/markdown-tests.swift "$work/main.swift"

swiftc -O -o "$work/tests" "$work/Markdown.swift" "$work/Theme.swift" "$work/main.swift"
"$work/tests"
