#!/usr/bin/env bash
# Prints the notes for a GitHub release: the changes since the previous tag, then how to install.
#   scripts/release-notes.sh v0.2.0 > notes.md
# The changes come from conventional commit subjects. docs, test, ci and chore are left out.
set -euo pipefail

tag="${1:?usage: release-notes.sh <tag>}"
repo="${GITHUB_REPOSITORY:-kernoeb/xummary}"

if previous="$(git describe --tags --abbrev=0 --match 'v*' "$tag^" 2>/dev/null)"; then
    range="$previous..$tag"
else
    previous=""
    range="$tag"
fi

# Prints one list for the commits of the given type, with "type(scope): " removed.
section() {
    local title="$1" type="$2" lines
    lines="$(git log --no-merges --format='%h %s' "$range" \
        | perl -ne 'if (/^(\w+) '"$type"'(?:\([^)]*\))?!?: (.*)$/) { print "- \u$2 ($1)\n" }')"
    if [ -n "$lines" ]; then
        printf '### %s\n\n%s\n\n' "$title" "$lines"
    fi
}

echo "## What changed"
echo
if [ -z "$previous" ]; then
    echo "First release."
    echo
fi
section "New" feat
section "Faster" perf
section "Fixes" fix

if [ -n "$previous" ]; then
    echo "All commits: https://github.com/$repo/compare/$previous...$tag"
else
    echo "All commits: https://github.com/$repo/commits/$tag"
fi

cat <<EOF

## Install

You need:

- a Mac with Apple silicon and macOS 14 or later
- the [\`claude\` CLI](https://claude.com/claude-code), logged in
- Chrome, Brave, Arc, Edge, Vivaldi or Chromium, logged into x.com

Run this in a terminal:

\`\`\`sh
curl -fsSL https://raw.githubusercontent.com/$repo/main/install.sh | sh
\`\`\`

It installs the app in \`/Applications\` and the \`xummary\` command in \`~/.local/bin\`. It checks both against \`SHA256SUMS\` first, and installs nothing if they do not match.

Other options:

- to update, run the same command again
- to install the command only, without the app, end it with \`sh -s -- --cli-only\`
- to install this version rather than the latest, end it with \`XUMMARY_VERSION=$tag sh\`

### The first time you run it

macOS asks if xummary can use the key your browser keeps in the Keychain. xummary needs it to read your x.com session. Click **Always Allow**.

Every release is signed with the same certificate, so macOS does not ask again after an update.

### Downloading the files by hand

The files are below this text:

- \`Xummary-macos-arm64.zip\`: the app, with the \`xummary\` command inside
- \`xummary-cli-macos-arm64.tar.gz\`: the \`xummary\` command on its own
- \`SHA256SUMS\`: checksums for both

xummary is not signed with a paid Apple Developer ID, so macOS blocks files downloaded in a browser. The install command above avoids this. If you downloaded them by hand, run this once after you move them into place:

\`\`\`sh
xattr -dr com.apple.quarantine /Applications/Xummary.app ~/.local/bin/xummary
\`\`\`
EOF
