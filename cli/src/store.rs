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
    /// When the reader actually had this briefing in front of them. Absent
    /// until they do, which is what keeps a new-story mark alive until it has
    /// been seen rather than until the text is replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at: Option<DateTime<Utc>>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        // `XUMMARY_CACHE_DIR` points the store somewhere else, so a run can be
        // replayed against a frozen cache without touching the real one.
        let dir = match std::env::var_os("XUMMARY_CACHE_DIR") {
            Some(path) => PathBuf::from(path),
            None => directories::BaseDirs::new()
                .context("no home directory")?
                .cache_dir()
                .join("xummary"),
        };
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

    /// The newest briefing by its own timestamp, not by where it sits in the
    /// file — two runs finishing at once can append out of order.
    pub fn last_briefing(&self) -> Option<Briefing> {
        self.briefings().into_iter().max_by_key(|b| b.at)
    }

    /// The newest briefing the reader has actually seen.
    pub fn last_read(&self) -> Option<Briefing> {
        self.briefings()
            .into_iter()
            .filter(|b| b.read_at.is_some())
            .max_by_key(|b| b.at)
    }

    /// Records that the reader has seen the newest briefing. The app calls
    /// this rather than writing the file itself, so the format stays in one
    /// place. Nothing stored yet is not an error.
    pub fn mark_newest_read(&self) -> Result<()> {
        let mut all = self.briefings();
        let Some(newest) = all
            .iter_mut()
            .max_by_key(|b| b.at)
        else {
            return Ok(());
        };
        newest.read_at = Some(Utc::now());
        let body = join(all.iter().filter_map(|b| serde_json::to_string(b).ok()));
        write_atomic(&self.briefings_path(), &body)
    }

    pub fn add_briefing(&self, briefing: &Briefing) -> Result<()> {
        let mut all = self.briefings();
        all.push(briefing.clone());
        let start = all.len().saturating_sub(KEEP_BRIEFINGS);
        let body = join(all[start..].iter().filter_map(|b| serde_json::to_string(b).ok()));
        write_atomic(&self.briefings_path(), &body)
    }
}

/// One story out of a briefing.
pub struct Section {
    pub heading: String,
    pub body: String,
}

impl Section {
    /// The first sentence of the section — enough to say where the story stood,
    /// without handing over a paragraph to copy.
    pub fn gist(&self) -> String {
        let end = self
            .body
            .match_indices(". ")
            .map(|(i, _)| i + 1)
            .next()
            .unwrap_or(self.body.len());
        self.body[..end].trim().to_string()
    }
}

/// Splits a briefing into its stories. `Also` is the leftovers bin, not a
/// story, and any new-story mark is taken off the heading.
pub fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading
                .trim_end_matches(crate::llm::NEW_MARK)
                .trim_end_matches(crate::llm::UPDATED_MARK)
                .trim_end()
                .to_string();
            if heading.eq_ignore_ascii_case("also") {
                break;
            }
            out.push(Section {
                heading,
                body: String::new(),
            });
        } else if let Some(current) = out.last_mut() {
            if !line.is_empty() {
                if !current.body.is_empty() {
                    current.body.push(' ');
                }
                current.body.push_str(line);
            }
        }
    }
    out
}

#[cfg(test)]
fn headings(text: &str) -> Vec<String> {
    sections(text).into_iter().map(|s| s.heading).collect()
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
            read_at: None,
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
    fn the_newest_briefing_wins_even_written_out_of_order() {
        let (store, _dir) = store();
        store.add_briefing(&briefing(at(18), "evening")).unwrap();
        store.add_briefing(&briefing(at(9), "morning")).unwrap();
        assert_eq!(store.last_briefing().unwrap().text, "evening");
    }

    #[test]
    fn marking_read_applies_to_the_newest_briefing_only() {
        let (store, _dir) = store();
        assert!(store.mark_newest_read().is_ok(), "nothing stored is not an error");
        store.add_briefing(&briefing(at(9), "morning")).unwrap();
        store.add_briefing(&briefing(at(18), "evening")).unwrap();
        store.mark_newest_read().unwrap();

        assert_eq!(store.last_read().unwrap().text, "evening");
        let morning = store.briefings().into_iter().find(|b| b.text == "morning").unwrap();
        assert!(morning.read_at.is_none());
    }

    #[test]
    fn an_unread_briefing_is_not_the_last_read() {
        let (store, _dir) = store();
        store.add_briefing(&briefing(at(9), "morning")).unwrap();
        store.mark_newest_read().unwrap();
        store.add_briefing(&briefing(at(18), "evening")).unwrap();
        // The evening one is newer but has not been looked at.
        assert_eq!(store.last_read().unwrap().text, "morning");
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
    fn headings_skip_also_and_drop_the_new_mark() {
        let text = "## Morning story\n\nText.\n\n## Fresh one [new]\n\nText.\n\n## Moved on [updated]\n\nText.\n\n## Also\n- a leftover";
        assert_eq!(headings(text), vec!["Morning story", "Fresh one", "Moved on"]);
    }

    #[test]
    fn a_gist_is_the_first_sentence_only() {
        let text = "## Zevent\n\nLa cagnotte passe 12 millions. Puis @a critique. Et @b repond.";
        let sections = sections(text);
        assert_eq!(sections[0].gist(), "La cagnotte passe 12 millions.");
        assert!(sections[0].body.contains("@b repond"));
    }

    #[test]
    fn a_section_with_one_sentence_gists_to_all_of_it() {
        let sections = sections("## Solo\n\nRien qu une phrase");
        assert_eq!(sections[0].gist(), "Rien qu une phrase");
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
