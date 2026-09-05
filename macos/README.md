# Xummary.app

A small macOS window for the same briefing the [`xummary`](../cli) CLI
produces. It runs the CLI and renders its markdown with real typography:
coloured headings, bold, true italics, accent-coloured @handles.

```
./build.sh
open Xummary.app
```

`build.sh` embeds the `xummary` binary from `~/.local/bin/xummary` into the
bundle, so the app is self-contained. Override with `XUMMARY_BIN=/path/to/xummary`.

## What it does

1. Spawns `xummary --print --hours <n>`.
2. Reads the briefing from stdout as it streams, and the progress line from
   stderr.
3. Parses each line into a heading, a paragraph or a bullet, and each line into
   styled runs.
4. Draws it in one column, 660 points wide, in the system appearance.

The app has no X code and no Claude code of its own. Everything it shows comes
from the CLI.

## Controls

- **12 h / 24 h / 48 h** — how far back to look. Changing it refetches.
- **⌘R** or the refresh button — run it again.

## Design

| | |
| --- | --- |
| Headings | 21 pt semibold, 34 pt of space above |
| Body | 15 pt, 7 pt line spacing, 660 pt column |
| Handles | accent colour, medium weight |
| Quotes | true italic, dimmed |
| Dark | `#0F1115` ground, `#7AA2F7` accent |
| Light | `#FBFBFD` ground, `#2B5FD9` accent |

Colours live in `Theme.swift` rather than an asset catalog, so the whole app is
plain Swift with no Xcode project.

## Known gaps

- **No app icon.** It shows the generic placeholder in the Dock.
- **Ad-hoc signature only.** Fine on your own machine; another Mac will refuse
  to open it.
- **No auto-scroll.** The text fills downward while you read from the top,
  which is deliberate.
