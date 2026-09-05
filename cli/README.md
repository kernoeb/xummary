# xummary

One LLM briefing of the day's X timeline, in a terminal pane.

It reads your logged-in browser session, pulls a few pages of **For You** and
**Following**, and streams a single grouped summary. There is no feed to
scroll, which is the point.

```
xummary                      # both feeds, 10 pages each, last 24 hours
xummary --hours 48 --pages 5
xummary --lang English       # the default is French
xummary --print              # plain stdout, no pane
```

## How it works

1. **Session** — decrypts `auth_token` and `ct0` from a Chromium-family
   cookie database. On macOS the AES key comes from the *"&lt;Browser&gt; Safe
   Storage"* keychain entry, so the first run asks for permission.
2. **Fetch** — calls X's private web GraphQL API (`HomeTimeline` and
   `HomeLatestTimeline`) with the public web bearer, one page at a time.
3. **Filter** — drops anything older than `--hours`, dedupes across the two
   feeds, keeps the 600 most recent posts.
4. **Summarize** — pipes one compact line per post to `claude -p` and streams
   the answer back.

Nothing is written to disk and no post leaves your machine except in the
prompt sent to Claude.

## Requirements

- Rust 1.82+
- the `claude` CLI on your `PATH`, already logged in
- a Chromium-family browser logged into x.com: Chrome, Brave, Chromium, Edge,
  Vivaldi or Arc

## Install

```
cargo build --release
cp target/release/xummary ~/.local/bin/
```

## Options

| Flag | Default | What it does |
| --- | --- | --- |
| `--pages <n>` | 10 | ceiling on pages per feed; paging stops early once out of the window |
| `--hours <n>` | 24 | ignore posts older than this |
| `--lang <name>` | French (or `$XUMMARY_LANG`) | language to write the briefing in |
| `--model <id>` | `claude-sonnet-5` (or `$XUMMARY_MODEL`) | Claude model to use |
| `--browser <name>` | first one found | chrome, brave, chromium, edge, vivaldi, arc |
| `--for-you-only` | | skip the Following feed |
| `--following-only` | | skip the For You feed |
| `--print` | | write to stdout instead of opening the pane |

## Keys

`q` quit · `j`/`k` scroll · space and PageUp page · `g` top · `G` follow the stream

## When it breaks

**"HTTP 404" on a fetch.** X rotated the GraphQL operation id. Open x.com with
devtools on the network tab, find a `graphql/<id>/HomeTimeline` request, and
copy the id into `HOME_TIMELINE_ID` in `src/x.rs`.

**"found x.com cookies but not both auth_token and ct0".** Your session
expired. Log into x.com again in the browser.

**"could not read the browser's cookie password".** You denied the keychain
prompt. Re-run and allow it, or use `--browser` to pick another browser.

## Limits

- read-only, and it never posts anything
- Linux reads cookies only when the browser used the fallback password, not a
  desktop keyring
- X rate-limits the home timeline; roughly 15 pages a run is comfortable

Inspired by [unrager](https://github.com/guitaripod/unrager), which does the
much harder job of being a whole X client. This shares no code with it.
