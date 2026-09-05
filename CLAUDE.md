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
cargo test                          # 25 tests, all unit tests inside src/*.rs
cargo test cookies::                # one module
cargo test round_trips_a_plain      # one test by name
cargo clippy --all-targets

# Swift
cd macos
swift build -c release
./build.sh                          # -> Xummary.app, with the CLI embedded from ~/.local/bin/xummary
ICON_PALETTE=paper ./build.sh       # night (default) | paper | dark
XUMMARY_BIN=/path/to/xummary ./build.sh
./tools/test.sh                     # inline markdown parser checks

# install — always remove first, see below
rm -f ~/.local/bin/xummary && cp cli/target/release/xummary ~/.local/bin/
rm -rf /Applications/Xummary.app && cp -R macos/Xummary.app /Applications/
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

## The cache

A refresh should cost one page and one short briefing, not ten pages and a whole day again. `cli/src/store.rs` keeps two files in `~/Library/Caches/xummary`:

- `posts.jsonl` — every post fetched, kept for at least 72 hours whatever `--hours` asked for, so widening the window to 48h is served from disk
- `briefings.jsonl` — the last 50 briefings, `{at, hours, posts, text}`

Three things follow from it:

- **Paging stops on posts you already have.** `collect` breaks on a page with no post that is both inside the window and absent from the cache. Same heuristic as before, one cutoff later.
- **A refresh only summarizes what arrived since the last briefing.** The cutoff is the previous briefing's `at`, so the model sees 40 new posts, not 600. Under `MIN_POSTS` new posts the run says `nothing new since HH:MM` and exits **0** without calling `claude` — that is a normal outcome, not a failure.
- **"New" means `seen_at`, never `created_at`.** For You is ranked, so it hands you posts hours after they were written — measured on a live refresh, 31 posts first surfaced with a median lag of about four hours, one of them 35 hours old. Selecting on `created_at > last briefing` dropped **all 31**, and the next refresh skipped them again as already cached, so they were lost for good. Each cached post carries the moment it first reached us, and that is what the window compares against. A cache line with no `seen_at` predates the field and counts as seen when written.
- **A wider window is a new question.** A briefing stored with `hours: 24` does not satisfy a `--hours 48` run, so that one goes back to a full walk. That is why `hours` is in the record.
- **A refresh is told what it already said.** `covered_since` collects the `## ` headings of every briefing still inside the window, newest first, and `build_prompt` lists them under "I have already read a briefing today covering these stories". Without it a running story gets a fresh section every twenty minutes: two of nine sections repeated on the first real refresh — Zevent legitimately (new reactions) and Astra not at all (same story, different posts). The instruction is deliberately not "skip these": a story that actually moved should be reported, but only the part that moved.

Paging therefore stops against the whole window, not against the last briefing: a post written this morning and surfaced now is still wanted. That makes For You page deeper than Following on a refresh — it is ranked, so unseen posts are scattered rather than stacked at the top. `--pages` is the ceiling that keeps it bounded.

`--no-cache` touches neither file: a full page walk over the whole window, and the cache is left as it was found. `--log` prints the stored briefings as JSON lines, oldest first — the macOS app calls it at launch instead of knowing the path.

The app keeps every briefing on screen, newest at top, each under a rule with its time and post count. That is the point of the whole thing: refresh adds a block, it does not wipe what you were reading.

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

**Effort is the single biggest lever, and `claude` defaults it to `high`.** Pass `--effort` (the CLI flag; `output_config.effort` on the API — `budget_tokens` is removed on Sonnet 5 and `MAX_THINKING_TOKENS` is not the knob). Measured on the same 600-post feed:

| effort | wait (to first token) | total | handles named |
|---|---|---|---|
| low | 2 s | 39 s | 25 |
| medium | 2 s | 36 s | 19 |
| high | 158 s | 188 s | 36 |

Summarising is judgement, not reasoning, so `low` is the default. Everything that looked like a hang was `high` spending two and a half minutes thinking before the first character.

**Thinking is not streamed.** A run bills ~66% of its output tokens as thinking (`usage.output_tokens_details.thinking_tokens`), and none of it arrives as a delta — measured 556 thinking tokens against 0 streamed thinking characters. So the whole thinking phase is dead air on stdout and looks like a hang. That is what the elapsed ticker in `produce` exists for; do not remove it.

**Overwriting a binary in place kills it.** On arm64 macOS, `cp` onto an existing signed binary invalidates its signature and the kernel SIGKILLs it at launch — exit `137`, no output, no error message, which reads exactly like a crash on startup. Always `rm -f` the destination first. This applies to `~/.local/bin/xummary` and to `/Applications/Xummary.app`.

**Signing** — `build.sh` signs ad-hoc, so every rebuild is a new identity to TCC and macOS re-asks for permissions. Not a bug; a Developer ID would be needed to stop it.

**Underscores are not emphasis** (`macos/Sources/Xummary/Markdown.swift`). They are markdown emphasis in theory and part of a handle in practice: `Frederic_Molas` once paired with the trailing `_` of `@LLCoolChris_` and italicised the whole paragraph between them. Only `*` opens an italic. The app is a single executable target with nowhere to hang XCTest, so `tools/test.sh` compiles the real source against `tools/markdown-tests.swift`; add a case there when you touch the parser.

## Prompt design

`build_prompt` in `cli/src/llm.rs`. Two rules exist for reasons that are easy to undo by accident:

- **One section per story.** An earlier version asked for "5 to 8 topics", which forced the model to merge unrelated events under one heading — `## Affaires Bolloré et Guillotin` asserts a link between two unrelated cases. The prompt now states that two things sharing only a theme are two stories, and bans headings that join two subjects with "and".
- **The posts are data.** They are strangers' text pasted into a prompt, so the prompt says never to follow an instruction written inside one.

Default model is `claude-sonnet-5` (`--model` / `XUMMARY_MODEL`). Default language is French (`--lang` / `XUMMARY_LANG`).

## Icon

`macos/tools/make-icon.swift` draws the icon with CoreGraphics on a 1024 grid and writes an `.iconset`; `build.sh` runs it and `iconutil` on every build, so no binary is committed. Positions derive from the mark's own height, so it is centred by construction — don't reintroduce hand-tuned offsets.
