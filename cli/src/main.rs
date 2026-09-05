//! digest — one LLM briefing of the day's X timeline.
//!
//! Reads your logged-in browser session, pulls a few pages of For You and
//! Following, and streams a single grouped summary into a terminal pane.
//! There is no feed to scroll, which is the point.

mod cookies;
mod llm;
mod store;
mod ui;
mod x;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use std::collections::HashSet;
use std::sync::Arc;
use store::{Briefing, Cached, Store};
use tokio::sync::mpsc;
use ui::Update;
use x::{Client, Feed, Tweet};

#[derive(Debug, Parser)]
#[command(name = "xummary", about = "One LLM briefing of today's X timeline", version)]
struct Args {
    #[arg(
        long,
        default_value_t = 10,
        help = "Ceiling on pages per feed; paging stops early once out of the window"
    )]
    pages: u32,

    #[arg(
        long,
        default_value_t = 24,
        help = "Only summarize posts from the last N hours"
    )]
    hours: i64,

    #[arg(
        long,
        env = "XUMMARY_LANG",
        default_value = "French",
        help = "Language to write the briefing in"
    )]
    lang: String,

    #[arg(
        long,
        env = "XUMMARY_EFFORT",
        default_value = "low",
        help = "Reasoning effort: low, medium, high, xhigh, max. Summarising is not \
                a reasoning task, so low is the default; claude's own default is high"
    )]
    effort: String,

    #[arg(
        long,
        env = "XUMMARY_MODEL",
        default_value = "claude-sonnet-5",
        help = "Claude model: sonnet is the balance, claude-haiku-4-5-20251001 is ~3x faster"
    )]
    model: String,

    #[arg(
        long,
        help = "Browser to read cookies from: chrome, brave, chromium, edge, vivaldi, arc"
    )]
    browser: Option<String>,

    #[arg(long, help = "Only the For You feed")]
    for_you_only: bool,

    #[arg(long, help = "Only the Following feed")]
    following_only: bool,

    #[arg(long, help = "Print to stdout instead of opening the pane")]
    print: bool,

    #[arg(
        long,
        help = "Ignore what past runs cached: walk every page and summarize the whole window"
    )]
    no_cache: bool,

    #[arg(long, help = "Print the briefings kept from past runs as JSON lines, then exit")]
    log: bool,

    #[arg(
        long,
        help = "Record that the newest briefing has been read, then exit. New-story \
                marks survive until this is called, so the app calls it once you have \
                actually had the briefing in front of you"
    )]
    mark_read: bool,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let feeds = feeds(&args)?;
    let store = Store::open()?;

    if args.log {
        return print_log(&store);
    }
    if args.mark_read {
        return store.mark_newest_read();
    }

    let (session, browser) = cookies::load(args.browser.as_deref())?;
    let client = Arc::new(Client::new(session)?);

    let (tx, rx) = mpsc::unbounded_channel();
    let _ = tx.send(Update::Status(format!("using your {browser} session")));

    let worker = tokio::spawn({
        let tx = tx.clone();
        let pages = args.pages;
        let hours = args.hours;
        let lang = args.lang.clone();
        let model = args.model.clone();
        let effort = args.effort.clone();
        let cache = !args.no_cache;
        async move {
            let run = produce(
                client, feeds, store, cache, pages, hours, &lang, &model, &effort, &tx,
            );
            if let Err(e) = run.await {
                let _ = tx.send(Update::Done(Some(format!("{e:#}"))));
            } else {
                let _ = tx.send(Update::Done(None));
            }
        }
    });
    drop(tx);

    let result = if args.print {
        print_plain(rx).await
    } else {
        ui::run(rx).await
    };
    worker.abort();
    result
}

/// `--log`: the stored briefings as JSON lines, oldest first. The app reads
/// this instead of knowing where the cache lives.
fn print_log(store: &Store) -> Result<()> {
    for briefing in store.briefings() {
        println!("{}", serde_json::to_string(&briefing)?);
    }
    Ok(())
}

fn feeds(args: &Args) -> Result<Vec<Feed>> {
    match (args.for_you_only, args.following_only) {
        (true, true) => anyhow::bail!("--for-you-only and --following-only cancel each other out"),
        (true, false) => Ok(vec![Feed::ForYou]),
        (false, true) => Ok(vec![Feed::Following]),
        (false, false) => Ok(vec![Feed::ForYou, Feed::Following]),
    }
}

/// A briefing built on fewer posts than this is noise, not news.
const MIN_POSTS: usize = 5;

/// A refresh rewrites the whole briefing, which costs a whole summary. Fewer
/// newly seen posts than this is not worth replacing what you are reading.
const MIN_NEW_POSTS: usize = 10;

/// Fetch what is new, then rewrite the briefing of the whole window.
///
/// Only the fetch is incremental: what arrived since the last briefing is all
/// that needs downloading, and the rest of the window is already on disk. The
/// briefing itself is always the whole window, so there is one thing to read
/// rather than a stack of slices, with the stories you have not seen marked.
#[allow(clippy::too_many_arguments)]
async fn produce(
    client: Arc<Client>,
    feeds: Vec<Feed>,
    store: Store,
    cache: bool,
    pages: u32,
    hours: i64,
    lang: &str,
    model: &str,
    effort: &str,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<()> {
    let started = std::time::Instant::now();
    let now = Utc::now();
    let window = now - chrono::Duration::hours(hours);

    // Posts are kept longer than the window asked for, so widening the window
    // later is free. Load everything retained, prune on the way back out.
    let cached = if cache {
        store.posts(now - chrono::Duration::hours(hours.max(72)))
    } else {
        Vec::new()
    };

    // A briefing that covered a narrower window than this one answered a
    // different question, so it does not spare us the work.
    let previous = if cache { store.last_briefing() } else { None }
        .filter(|b| b.hours >= hours && b.at >= window);
    let since = previous.as_ref().map_or(window, |b| b.at);

    // What to mark is judged against the last briefing the reader actually
    // read, not the last one written. Otherwise a refresh they never looked at
    // clears the marks, and stories go by unseen and unmarked. Until anything
    // has been read, the last briefing written is the best guess.
    let baseline = if cache { store.last_read() } else { None }
        .filter(|b| b.at >= window)
        .or_else(|| previous.clone());

    // Pages within a feed are chained by cursor and must be walked in order, but
    // the two feeds are independent — so they run side by side and the whole
    // fetch costs one feed's worth of time instead of two.
    // Cached ids only tell paging to stop on a refresh. A run that widens the
    // window has to walk the pages again, even though its first page is all
    // posts we already hold — the ones it needs are behind them.
    let known: Arc<HashSet<String>> = Arc::new(if previous.is_some() {
        cached.iter().map(|c| c.tweet.id.clone()).collect()
    } else {
        HashSet::new()
    });
    let mut tasks = Vec::new();
    for feed in feeds {
        let client = Arc::clone(&client);
        let known = Arc::clone(&known);
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            collect(&client, feed, pages, window, &known, &tx).await
        }));
    }

    let mut merged = cached;
    let mut seen: HashSet<String> = merged.iter().map(|c| c.tweet.id.clone()).collect();
    for task in tasks {
        for tweet in task.await.context("fetch task panicked")?? {
            if seen.insert(tweet.id.clone()) {
                merged.push(Cached { seen_at: now, tweet });
            }
        }
    }
    merged.sort_by_key(|c| std::cmp::Reverse(c.tweet.created_at));
    // Save before trimming to the window: what falls outside it today may be
    // inside a wider one tomorrow.
    if cache {
        store.save_posts(&merged, hours)?;
    }

    // What counts as new is what reached you since the last briefing, not what
    // was written since. For You surfaces posts hours late, and judging them by
    // their timestamp throws every one of them away.
    let fresh = merged
        .iter()
        .filter(|c| c.tweet.created_at >= window && c.seen_at > since)
        .count();

    let mut all: Vec<Tweet> = merged
        .into_iter()
        .filter(|c| c.tweet.created_at >= window)
        .map(|c| c.tweet)
        .collect();
    all.truncate(llm::MAX_TWEETS);

    if all.len() < MIN_POSTS {
        anyhow::bail!(
            "only {} posts in the last {hours}h — try --hours 48 or more --pages",
            all.len()
        );
    }

    // Refreshing twice in a row is normal and should cost nothing. Leave the
    // briefing on screen, marks and all, rather than rewrite it for six posts.
    let previous_at = previous.as_ref().map(|b| clock(b.at));
    if let Some(at) = &previous_at {
        if fresh < MIN_NEW_POSTS {
            let _ = tx.send(Update::Status(match fresh {
                0 => format!("nothing new since {at}"),
                n => format!("only {n} new posts since {at}"),
            }));
            return Ok(());
        }
    }

    let fetched = started.elapsed();
    let posts = all.len();
    let _ = tx.send(Update::Status(format!(
        "{posts} posts · last {hours}h · {fresh} new · summarizing"
    )));

    // Most of the generation is thinking, and thinking is not streamed, so
    // nothing arrives for a long stretch. Tick the elapsed seconds meanwhile,
    // otherwise a working run is indistinguishable from a hung one.
    let ticker = {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut seconds = 0u64;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                seconds += 1;
                let _ = tx.send(Update::Status(format!(
                    "{posts} posts · thinking · {seconds}s"
                )));
            }
        })
    };

    // Where a run actually spends its time, reported every run rather than
    // guessed at. A slow start and a slow finish have different causes.
    let mut first_token: Option<std::time::Duration> = None;
    let mut ticker = Some(ticker);
    let mut text = String::new();
    // The briefing the reader has already seen: each story and where it stood.
    // The first sentence is enough to tell a story that moved from one that
    // did not, and short enough that there is no prose to copy.
    let covered: Vec<String> = baseline.as_ref().map_or_else(Vec::new, |b| {
        store::sections(&b.text)
            .iter()
            .map(|s| format!("{} — {}", s.heading, s.gist()))
            .collect()
    });
    let read_at = baseline.as_ref().map(|b| clock(b.at));
    let seen = read_at.as_ref().map(|at| llm::Previous {
        at,
        stories: &covered,
    });
    let prompt = llm::build_prompt(&all, hours, seen, lang);
    let outcome = llm::stream(&prompt, model, effort, |token| {
        if let Some(handle) = ticker.take() {
            handle.abort();
        }
        if first_token.is_none() {
            first_token = Some(started.elapsed());
        }
        text.push_str(token);
        let _ = tx.send(Update::Token(token.to_string()));
    })
    .await;
    // A run that fails before its first token leaves the ticker running.
    if let Some(handle) = ticker.take() {
        handle.abort();
    }
    outcome.context("summary failed")?;

    if cache && !text.trim().is_empty() {
        store.add_briefing(&Briefing {
            at: now,
            hours,
            posts,
            text: text.trim().to_string(),
            read_at: None,
        })?;
    }

    let wait = first_token.map_or(0.0, |t| (t - fetched).as_secs_f64());
    let _ = tx.send(Update::Status(format!(
        "{posts} posts · fetch {:.0}s · wait {wait:.0}s · total {:.0}s",
        fetched.as_secs_f64(),
        started.elapsed().as_secs_f64()
    )));
    Ok(())
}

/// A timestamp as the reader's own wall clock, for a prompt or a status line.
fn clock(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local).format("%H:%M").to_string()
}

/// Page through one feed until it stops bringing posts we do not already have.
async fn collect(
    client: &Client,
    feed: Feed,
    pages: u32,
    cutoff: DateTime<Utc>,
    known: &HashSet<String>,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<Vec<Tweet>> {
    let label = feed.label();
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;

    for page in 1..=pages {
        let _ = tx.send(Update::Status(format!(
            "fetching {label} page {page}/{pages} · {} posts",
            out.len()
        )));

        let fetched = match client.home_page(feed, cursor.as_deref()).await {
            Ok(fetched) => fetched,
            // A first page that fails is fatal: usually an expired session.
            Err(e) => {
                if page == 1 {
                    return Err(e).with_context(|| format!("{label} page {page}"));
                }
                // Later pages usually fail on a rate limit. Keep what we have.
                let _ = tx.send(Update::Status(format!(
                    "{label} stopped at page {page}: {e:#}"
                )));
                break;
            }
        };

        let count = fetched.tweets.len();
        let fresh = fetched
            .tweets
            .iter()
            .filter(|t| t.created_at >= cutoff && !known.contains(&t.id))
            .count();
        out.extend(fetched.tweets);

        // A whole page with nothing we still need means we have caught up:
        // either paged out of the window, or reached posts the last refresh
        // already saw. Following is chronological so that is exact; For You is
        // ranked, but a page of 40 with nothing new is a good enough signal
        // that digging deeper will not help either.
        if count > 0 && fresh == 0 {
            break;
        }
        // Enough on its own — the global cap will trim the rest anyway.
        if out.len() >= llm::MAX_TWEETS {
            break;
        }
        match fetched.next_cursor {
            Some(c) if count > 0 => cursor = Some(c),
            _ => break,
        }
    }
    Ok(out)
}

/// `--print`: no terminal takeover, just the briefing on stdout.
///
/// A run with nothing new writes nothing at all, not even the closing newline.
/// The app treats any output as the new briefing arriving, so a stray newline
/// makes it throw away the briefing you were reading.
async fn print_plain(mut rx: mpsc::UnboundedReceiver<Update>) -> Result<()> {
    use std::io::Write;
    let mut wrote = false;
    while let Some(update) = rx.recv().await {
        match update {
            Update::Status(s) => eprintln!("{s}"),
            Update::Token(t) => {
                wrote = true;
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            Update::Done(outcome) => {
                if wrote {
                    println!();
                }
                if let Some(e) = outcome {
                    anyhow::bail!(e);
                }
            }
        }
    }
    Ok(())
}
