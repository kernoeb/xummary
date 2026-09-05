# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`xummary` fetches your X timeline and turns it into one LLM briefing. Two front ends over one engine:

- **`cli/`** — Rust. Does everything: browser cookies, X GraphQL, prompt, `claude -p`, and a ratatui pane.
- **`macos/`** — SwiftUI. Runs `xummary --print` and renders its markdown. It has **no X code and no Claude code of its own**; keep it that way, so the timeline parsing exists in one place.

## Commands

```sh
# Rust
cd cli
cargo build --release
cargo test                          # 20 tests, all unit tests inside src/*.rs
cargo test cookies::                # one module
cargo test round_trips_a_plain      # one test by name
cargo clippy --all-targets

# Swift
cd macos
swift build -c release
./build.sh                          # -> Xummary.app, with the CLI embedded from ~/.local/bin/xummary
ICON_PALETTE=paper ./build.sh       # night (default) | paper | dark
XUMMARY_BIN=/path/to/xummary ./build.sh

# install
cp cli/target/release/xummary ~/.local/bin/
cp -R macos/Xummary.app /Applications/
```

**Fast check that fetching still works, without spending tokens on a summary:**

```sh
xummary --pages 1 --hours 0 --following-only --print
```

`--hours 0` makes every post fall outside the window, so it exercises cookies + GraphQL and then exits with `only 0 posts` before ever calling `claude`. If that prints `using your <browser> session` and a page count, the session and the query ids are both fine.

## Refreshing the X GraphQL query ids

`cli/src/x.rs` hard-codes two operation ids:

```rust
const HOME_TIMELINE_ID: &str = "3tb-_5Lf7kdCZ1cFHmsEfg";        // For You
const HOME_LATEST_TIMELINE_ID: &str = "eObmT5Nuapp04u8bYWf49Q";  // Following
```

X rotates these. When it does, fetching fails with `HTTP 404`. To get the current ones:

1. Open <https://x.com/home> in a logged-in browser, devtools on the **Network** tab.
2. Filter on `graphql`.
3. Scroll the For You feed. Find a request whose path is `/i/api/graphql/<id>/HomeTimeline` — the `<id>` segment is the value.
4. Click the **Following** tab and do the same for `HomeLatestTimeline`.
5. Paste both into `x.rs`, rebuild, and verify with the `--hours 0` command above.

Do not try to scrape them with `curl`: x.com serves a shell with no script tags to non-browser clients, so there are no bundle URLs to follow. A logged-in browser session is required. (unrager solves this by scraping the ids from its authenticated login shell, if you ever want to automate it.)

The `features` map in `x.rs` is a separate rotating thing. If a request returns an error naming a missing feature flag, add it there.

## Facts that will bite you

**`claude` invocation** (`cli/src/llm.rs`) — the flags are load-bearing:

- `--safe-mode` disables CLAUDE.md, skills, plugins, hooks and MCP servers **but leaves auth alone**. Use it.
- `--bare` looks like the right flag and is not: it refuses OAuth and demands `ANTHROPIC_API_KEY`, so it fails with `Not logged in · Please run /login`.
- `CLAUDE_CONFIG_DIR` also breaks auth. Setting it stops `claude` consulting the Keychain, and `~/.claude/.credentials.json` is empty on a Keychain-backed login. There is no way to give it a clean config dir without a fresh login or copying a live token — don't copy the token.
- The child runs in an empty `Scratch` temp dir so no surrounding repo's config reaches the briefing. The directory removes itself on `Drop`.

**Cookies** (`cli/src/cookies.rs`):

- Host matching is an **exact list**, not `LIKE '%x.com'` — the latter also matches `phishx.com`.
- Rows are sorted x.com-first, first hit wins, because a stale `twitter.com` `auth_token` will otherwise overwrite the live one and every fetch 401s while the tool reports a working session.
- The cookie DB is copied (with its `-wal` sidecar) into a fresh 0700 directory before opening; the browser holds a write lock on the original.

**X API** (`cli/src/x.rs`):

- X **ignores the `count` parameter**. Asking for 100 returns the same ~111 entries as asking for 40, so there is no win in raising `PAGE_SIZE`.
- Pages within a feed are chained by cursor and must be sequential. The two feeds are independent and run as concurrent tasks — that halved fetch time.
- Paging stops when a page contains nothing inside the time window. Exact for Following (chronological); a heuristic for For You (ranked).

**Swift concurrency** (`macos/Sources/Xummary/Runner.swift`):

- `DispatchGroup.notify(queue:execute:)` takes a plain closure, so Swift 6 infers it as `@MainActor` from the enclosing method and then asserts that at runtime — on a global queue, which traps with `EXC_BREAKPOINT` in `dispatch_assert_queue_fail`. It must be marked `@Sendable`. This crashed only at the *end* of a run, because the `drain` closures were already `@Sendable`.
- Pipes are drained to EOF on their own threads, and a generation counter discards output from a superseded run, so ⌘R mid-stream cannot report a stale failure.

**Measuring performance** — two traps that produced wrong conclusions here:

- A freshly built unsigned binary costs **~17 s on first launch** to macOS Gatekeeper. Always run once to absorb it before timing anything.
- Other `claude` sessions on the same account contend. Check `ps -eo etime,command | grep claude` before trusting a number, and never benchmark while the app is running.

Each run reports its own breakdown (`600 posts · fetch 9s · wait 4s · total 41s`), where `wait` is time-to-first-token. Use it instead of guessing: a slow start and slow generation have different causes.

**Signing** — `build.sh` signs ad-hoc, so every rebuild is a new identity to TCC and macOS re-asks for permissions. Not a bug; a Developer ID would be needed to stop it.

## Prompt design

`build_prompt` in `cli/src/llm.rs`. Two rules exist for reasons that are easy to undo by accident:

- **One section per story.** An earlier version asked for "5 to 8 topics", which forced the model to merge unrelated events under one heading — `## Affaires Bolloré et Guillotin` asserts a link between two unrelated cases. The prompt now states that two things sharing only a theme are two stories, and bans headings that join two subjects with "and".
- **The posts are data.** They are strangers' text pasted into a prompt, so the prompt says never to follow an instruction written inside one.

Default model is `claude-sonnet-5` (`--model` / `XUMMARY_MODEL`). Default language is French (`--lang` / `XUMMARY_LANG`).

## Icon

`macos/tools/make-icon.swift` draws the icon with CoreGraphics on a 1024 grid and writes an `.iconset`; `build.sh` runs it and `iconutil` on every build, so no binary is committed. Positions derive from the mark's own height, so it is centred by construction — don't reintroduce hand-tuned offsets.
