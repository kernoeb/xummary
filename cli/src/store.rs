//! What a run remembers between refreshes: the posts it fetched, and the
//! briefings it wrote.
//!
//! Without this, every refresh walks ten pages again and re-summarizes a day
//! you already read. With it, a refresh fetches what arrived since the last
//! briefing and summarizes only that.

use crate::x::Tweet;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Posts stay on disk at least this long whatever window the run asked for, so
/// switching from 24h to 48h is served from the cache instead of the network.
const MIN_RETENTION_HOURS: i64 = 72;

/// How many past briefings to keep.
const KEEP_BRIEFINGS: usize = 50;

/// A cached post, with the moment it first reached us.
///
/// For You is ranked, not chronological: it surfaces posts hours after they
/// were written. So "new to me" is `seen_at`, never `created_at` — filtering
/// on when a post was written drops everything the ranker showed you late.
#[derive(Debug, Clone)]
pub struct Cached {
    pub seen_at: DateTime<Utc>,
    pub tweet: Tweet,
}

/// A cache line. `seen_at` is missing from anything written before it existed;
/// those posts count as seen when they were written, which is what the old
/// behaviour assumed anyway.
#[derive(Serialize, Deserialize)]
struct Row {
    #[serde(skip_serializing_if = "Option::is_none")]
    seen_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    tweet: Tweet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Briefing {
    pub at: DateTime<Utc>,
    /// The window this briefing covered, in hours.
    pub hours: i64,
    pub posts: usize,
    pub text: String,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let dir = directories::BaseDirs::new()
            .context("no home directory")?
            .cache_dir()
            .join("xummary");
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn posts_path(&self) -> PathBuf {
        self.dir.join("posts.jsonl")
    }

    fn briefings_path(&self) -> PathBuf {
        self.dir.join("briefings.jsonl")
    }

    /// Cached posts no older than `cutoff`. A line that no longer parses is
    /// dropped: a cache written by an older build must never stop a refresh.
    pub fn posts(&self, cutoff: DateTime<Utc>) -> Vec<Cached> {
        read(&self.posts_path())
            .lines()
            .filter_map(|line| serde_json::from_str::<Row>(line).ok())
            .map(|row| Cached {
                seen_at: row.seen_at.unwrap_or(row.tweet.created_at),
                tweet: row.tweet,
            })
            .filter(|c| c.tweet.created_at >= cutoff)
            .collect()
    }

    /// Replaces the cache with `posts`, dropping anything past retention.
    pub fn save_posts(&self, posts: &[Cached], hours: i64) -> Result<()> {
        let keep = Utc::now() - chrono::Duration::hours(hours.max(MIN_RETENTION_HOURS));
        let body = join(
            posts
                .iter()
                .filter(|c| c.tweet.created_at >= keep)
                .filter_map(|c| {
                    serde_json::to_string(&Row {
                        seen_at: Some(c.seen_at),
                        tweet: c.tweet.clone(),
                    })
                    .ok()
                }),
        );
        write_atomic(&self.posts_path(), &body)
    }

    /// Past briefings, oldest first.
    pub fn briefings(&self) -> Vec<Briefing> {
        read(&self.briefings_path())
            .lines()
            .filter_map(|line| serde_json::from_str::<Briefing>(line).ok())
            .collect()
    }

    pub fn last_briefing(&self) -> Option<Briefing> {
        self.briefings().pop()
    }

    /// Headings from briefings no older than `cutoff`, newest first, so a
    /// refresh can be told which stories it has already reported.
    pub fn covered_since(&self, cutoff: DateTime<Utc>) -> Vec<String> {
        self.briefings()
            .iter()
            .rev()
            .filter(|b| b.at >= cutoff)
            .flat_map(|b| headings(&b.text))
            .collect()
    }

    pub fn add_briefing(&self, briefing: &Briefing) -> Result<()> {
        let mut all = self.briefings();
        all.push(briefing.clone());
        let start = all.len().saturating_sub(KEEP_BRIEFINGS);
        let body = join(all[start..].iter().filter_map(|b| serde_json::to_string(b).ok()));
        write_atomic(&self.briefings_path(), &body)
    }
}

/// The `## ` lines of a briefing. `Also` is the leftovers bin, not a story.
fn headings(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("## "))
        .filter(|heading| !heading.eq_ignore_ascii_case("also"))
        .map(str::to_string)
        .collect()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn join(lines: impl Iterator<Item = String>) -> String {
    lines.fold(String::new(), |mut acc, line| {
        acc.push_str(&line);
        acc.push('\n');
        acc
    })
}

/// Write through a temporary file, so a run killed mid-write — or the app and
/// the terminal refreshing at the same moment — leaves a whole file behind.
fn write_atomic(path: &Path, body: &str) -> Result<()> {
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temp, body).with_context(|| format!("write {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn store() -> (Store, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        (
            Store {
                dir: dir.path().to_path_buf(),
            },
            dir,
        )
    }

    fn briefing(at: DateTime<Utc>, text: &str) -> Briefing {
        Briefing {
            at,
            hours: 24,
            posts: 100,
            text: text.into(),
        }
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 6, hour, 0, 0).unwrap()
    }

    #[test]
    fn the_last_briefing_is_the_newest_one_written() {
        let (store, _dir) = store();
        assert!(store.last_briefing().is_none());
        store.add_briefing(&briefing(at(9), "morning")).unwrap();
        store.add_briefing(&briefing(at(18), "evening")).unwrap();
        assert_eq!(store.last_briefing().unwrap().text, "evening");
        assert_eq!(store.briefings().len(), 2);
    }

    #[test]
    fn only_the_last_fifty_briefings_survive() {
        let (store, _dir) = store();
        for i in 0..KEEP_BRIEFINGS + 10 {
            store.add_briefing(&briefing(at(1), &i.to_string())).unwrap();
        }
        let kept = store.briefings();
        assert_eq!(kept.len(), KEEP_BRIEFINGS);
        assert_eq!(kept[0].text, "10");
    }

    #[test]
    fn a_corrupt_line_is_skipped_not_fatal() {
        let (store, _dir) = store();
        std::fs::write(store.briefings_path(), "not json\n{\"at\":\"2026-09-06T09:00:00Z\",\"hours\":24,\"posts\":1,\"text\":\"ok\"}\n").unwrap();
        let kept = store.briefings();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text, "ok");
    }

    #[test]
    fn posts_older_than_the_cutoff_are_not_returned() {
        let (store, _dir) = store();
        let old = cached("old", at(1), at(1));
        let fresh = cached("fresh", at(20), at(20));
        store.save_posts(&[old, fresh], 24).unwrap();
        let kept = store.posts(at(10));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].tweet.id, "fresh");
    }

    #[test]
    fn saving_drops_posts_past_retention() {
        let now = Utc::now();
        let (store, _dir) = store();
        let ancient = cached(
            "ancient",
            now - chrono::Duration::hours(MIN_RETENTION_HOURS + 1),
            now,
        );
        let recent = cached("recent", now, now);
        store.save_posts(&[ancient, recent], 24).unwrap();
        let kept = store.posts(Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].tweet.id, "recent");
    }

    #[test]
    fn covered_stories_are_the_headings_newest_first() {
        let (store, _dir) = store();
        store
            .add_briefing(&briefing(at(9), "## Morning story\n\nText.\n\n## Also\n- a leftover"))
            .unwrap();
        store
            .add_briefing(&briefing(at(18), "## Evening story\n\nText."))
            .unwrap();
        assert_eq!(
            store.covered_since(at(1)),
            vec!["Evening story", "Morning story"]
        );
    }

    #[test]
    fn a_briefing_older_than_the_window_is_not_covered() {
        let (store, _dir) = store();
        store.add_briefing(&briefing(at(2), "## Old story")).unwrap();
        store.add_briefing(&briefing(at(20), "## New story")).unwrap();
        assert_eq!(store.covered_since(at(10)), vec!["New story"]);
    }

    #[test]
    fn when_a_post_reached_us_survives_a_round_trip() {
        let (store, _dir) = store();
        // Written at 09:00, but For You only showed it to us at 20:00.
        store.save_posts(&[cached("late", at(9), at(20))], 24).unwrap();
        let kept = store.posts(at(1));
        assert_eq!(kept[0].tweet.created_at, at(9));
        assert_eq!(kept[0].seen_at, at(20));
    }

    #[test]
    fn a_line_without_seen_at_counts_as_seen_when_written() {
        let (store, _dir) = store();
        let row = serde_json::to_string(&Row {
            seen_at: None,
            tweet: tweet("old-build"),
        })
        .unwrap();
        std::fs::write(store.posts_path(), format!("{row}\n")).unwrap();
        let kept = store.posts(at(1));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].seen_at, kept[0].tweet.created_at);
    }

    fn cached(id: &str, created_at: DateTime<Utc>, seen_at: DateTime<Utc>) -> Cached {
        let mut tweet = tweet(id);
        tweet.created_at = created_at;
        Cached { seen_at, tweet }
    }

    fn tweet(id: &str) -> Tweet {
        Tweet {
            id: id.into(),
            handle: "alice".into(),
            created_at: at(12),
            text: "hi".into(),
            likes: 0,
            retweets: 0,
            replies: 0,
            is_reply: false,
            has_media: false,
            retweeted_by: None,
            quoted: None,
        }
    }

    /// A directory that removes itself, so the tests never touch the real cache.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("xummary-store-test-{}-{n}", std::process::id()));
                let _ = std::fs::remove_dir_all(&path);
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
