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
use chrono::Utc;
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
        help = "Pages to pull from each feed (~40 posts a page)"
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

    #[arg(long, help = "Claude model to use (defaults to your CLI's)")]
    model: Option<String>,

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
        async move {
            if let Err(e) = produce(client, feeds, pages, hours, &lang, model.as_deref(), &tx).await
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
    model: Option<&str>,
    tx: &mpsc::UnboundedSender<Update>,
) -> Result<()> {
    let mut all = Vec::new();
    for feed in feeds {
        all.extend(collect(&client, feed, pages, tx).await?);
    }

    let cutoff = Utc::now() - chrono::Duration::hours(hours);
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

    let _ = tx.send(Update::Status(format!(
        "{} posts · last {hours}h · summarizing",
        all.len()
    )));

    let prompt = llm::build_prompt(&all, hours, lang);
    llm::stream(&prompt, model, |token| {
        let _ = tx.send(Update::Token(token.to_string()));
    })
    .await
    .context("summary failed")
}

/// Page through one feed, reporting progress as it goes.
async fn collect(
    client: &Client,
    feed: Feed,
    pages: u32,
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
        out.extend(fetched.tweets);
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
