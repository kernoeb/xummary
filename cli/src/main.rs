//! digest — one LLM briefing of the day's X timeline.
//!
//! Reads your logged-in browser session, pulls a few pages of For You and
//! Following, and streams a single grouped summary into a terminal pane.
//! There is no feed to scroll, which is the point.

mod cookies;
mod llm;
mod ui;
mod x;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use std::collections::HashSet;
use std::sync::Arc;
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
        async move {
            if let Err(e) = produce(client, feeds, pages, hours, &lang, &model, &effort, &tx).await
            {
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

fn feeds(args: &Args) -> Result<Vec<Feed>> {
    match (args.for_you_only, args.following_only) {
        (true, true) => anyhow::bail!("--for-you-only and --following-only cancel each other out"),
        (true, false) => Ok(vec![Feed::ForYou]),
        (false, true) => Ok(vec![Feed::Following]),
        (false, false) => Ok(vec![Feed::ForYou, Feed::Following]),
    }
}

/// Fetch every requested feed, keep what is recent, then stream one summary.
async fn produce(
    client: Arc<Client>,
    feeds: Vec<Feed>,
    pages: u32,
    hours: i64,
    lang: &str,
    model: &str,
    effort: &str,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<()> {
    let started = std::time::Instant::now();

    // Pages within a feed are chained by cursor and must be walked in order, but
    // the two feeds are independent — so they run side by side and the whole
    // fetch costs one feed's worth of time instead of two.
    let cutoff = Utc::now() - chrono::Duration::hours(hours);
    let mut tasks = Vec::new();
    for feed in feeds {
        let client = Arc::clone(&client);
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            collect(&client, feed, pages, cutoff, &tx).await
        }));
    }

    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.context("fetch task panicked")??);
    }

    let mut seen = HashSet::new();
    all.retain(|t: &Tweet| t.created_at >= cutoff && seen.insert(t.id.clone()));
    all.sort_by_key(|t| std::cmp::Reverse(t.created_at));
    all.truncate(llm::MAX_TWEETS);

    if all.len() < 5 {
        anyhow::bail!(
            "only {} posts in the last {hours}h — try --hours 48 or more --pages",
            all.len()
        );
    }

    let fetched = started.elapsed();
    let posts = all.len();
    let _ = tx.send(Update::Status(format!(
        "{posts} posts · last {hours}h · summarizing"
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
    let prompt = llm::build_prompt(&all, hours, lang);
    let outcome = llm::stream(&prompt, model, effort, |token| {
        if let Some(handle) = ticker.take() {
            handle.abort();
        }
        if first_token.is_none() {
            first_token = Some(started.elapsed());
        }
        let _ = tx.send(Update::Token(token.to_string()));
    })
    .await;
    // A run that fails before its first token leaves the ticker running.
    if let Some(handle) = ticker.take() {
        handle.abort();
    }
    outcome.context("summary failed")?;

    let wait = first_token.map_or(0.0, |t| (t - fetched).as_secs_f64());
    let _ = tx.send(Update::Status(format!(
        "{posts} posts · fetch {:.0}s · wait {wait:.0}s · total {:.0}s",
        fetched.as_secs_f64(),
        started.elapsed().as_secs_f64()
    )));
    Ok(())
}

/// Page through one feed, reporting progress as it goes.
async fn collect(
    client: &Client,
    feed: Feed,
    pages: u32,
    cutoff: DateTime<Utc>,
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
        let in_window = fetched.tweets.iter().filter(|t| t.created_at >= cutoff).count();
        out.extend(fetched.tweets);

        // A whole page with nothing recent means we have paged out of the
        // window. Following is chronological so that is exact; For You is
        // ranked, but a page of 40 with nothing from the window is a good
        // enough signal that digging deeper will not help either.
        if count > 0 && in_window == 0 {
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
async fn print_plain(mut rx: mpsc::UnboundedReceiver<Update>) -> Result<()> {
    use std::io::Write;
    while let Some(update) = rx.recv().await {
        match update {
            Update::Status(s) => eprintln!("{s}"),
            Update::Token(t) => {
                print!("{t}");
                let _ = std::io::stdout().flush();
            }
            Update::Done(Some(e)) => {
                println!();
                anyhow::bail!(e);
            }
            Update::Done(None) => println!(),
        }
    }
    Ok(())
}
