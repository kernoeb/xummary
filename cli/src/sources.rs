//! Which posts each story of a briefing came from, so the reader can open them.
//!
//! Decided in code once the briefing is written, like the marks: the prompt
//! stays as it is, and a story can only ever point at a post we really fetched.

use crate::x::Tweet;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// At most this many posts under one story. An account the story names always
/// gets its post; the cap only limits the ones found by wording alone.
const MAX_SOURCES: usize = 5;

/// A post the story does not name must share this many rare words with it,
/// and they must make up this share of the post's own words. A long post
/// shares a few words with anything.
const MIN_SHARED: usize = 3;
const MIN_COVERAGE: f64 = 0.08;

/// At most this many posts by one account under one story.
const MAX_PER_ACCOUNT: usize = 2;

/// How much of a post the app shows when you hover its link.
const SNIPPET_CHARS: usize = 120;

/// A word in more than this share of the posts is too common to match on:
/// "pour", "avec" and "selon" otherwise tie a story to any long French post.
const RARE: f64 = 0.02;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub handle: String,
    pub id: String,
    pub snippet: String,
    /// The handle the story names this post by, lowercased. Usually its author,
    /// but it can be whoever retweeted it. Absent when found by wording alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named: Option<String>,
    /// For `Also` only: the line under the heading that names it, from 0. Each
    /// of its lines is a story of its own, so one handle can lead to several posts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sourced {
    pub heading: String,
    pub posts: Vec<Source>,
}

struct Candidate {
    source: Source,
    words: HashSet<String>,
    /// Lowercased handles a story may use to point at this post: its author,
    /// and whoever retweeted it into the feed.
    names: Vec<String>,
}

/// The posts behind each story of `text`, one entry per story in order, even
/// with no posts: two stories can share a heading, and the app tells them apart
/// by position. `Also` lines are matched one by one, and only by handle.
pub fn attribute(text: &str, posts: &[Tweet]) -> Vec<Sourced> {
    let candidates = candidates(posts);
    let idf = idf(&candidates);
    let mut out: Vec<Sourced> = Vec::new();
    // A story's body can span lines, so it is matched as one text at the end.
    let mut bodies: Vec<String> = Vec::new();
    let mut also = false;
    let mut below = 0;

    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(raw) = line.strip_prefix("## ") {
            let heading = crate::store::strip_marks(raw).to_string();
            also = heading.eq_ignore_ascii_case("also");
            out.push(Sourced {
                heading,
                posts: Vec::new(),
            });
            bodies.push(String::new());
            below = 0;
        } else if let (Some(story), Some(body)) = (out.last_mut(), bodies.last_mut()) {
            if also {
                pick_named(line, Some(below), &candidates, &idf, &mut story.posts);
            } else {
                body.push(' ');
                body.push_str(line);
            }
            below += 1;
        }
    }

    for (story, body) in out.iter_mut().zip(&bodies) {
        pick_named(body, None, &candidates, &idf, &mut story.posts);
        // The heading holds the story's most specific words, so it counts too.
        let whole = format!("{} {body}", story.heading);
        pick_unnamed(&whole, &candidates, &idf, &mut story.posts);
    }
    out
}

fn pick_named(
    text: &str,
    line: Option<usize>,
    candidates: &[Candidate],
    idf: &HashMap<&str, f64>,
    picked: &mut Vec<Source>,
) {
    let story = words(text);
    let mut done: Vec<String> = Vec::new();
    for sentence in text.split_inclusive(['.', '!', '?']) {
        let near = words(sentence);
        for handle in handles(sentence) {
            if done.contains(&handle) {
                continue;
            }
            done.push(handle.clone());
            let all: Vec<&Candidate> = candidates.iter().filter(|c| c.names.contains(&handle)).collect();
            let theirs: Vec<&Candidate> = all
                .iter()
                .copied()
                .filter(|c| !picked.iter().any(|p| p.id == c.source.id))
                .collect();
            // The sentence naming the account says which of its posts is meant;
            // the rest of the story breaks ties. With nothing in common, the
            // profile is a better link than a guess, unless it is their only post.
            let best = match (all.len(), theirs.as_slice()) {
                (1, [only]) => Some(*only),
                _ => theirs
                    .iter()
                    .map(|c| (*c, 2.0 * score(&near, c, idf).1 + score(&story, c, idf).1))
                    .filter(|(_, s)| *s > 0.0)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(c, _)| c),
            };
            if let Some(c) = best {
                picked.push(Source {
                    named: Some(handle),
                    line,
                    ..c.source.clone()
                });
            }
        }
    }
}

fn pick_unnamed(text: &str, candidates: &[Candidate], idf: &HashMap<&str, f64>, picked: &mut Vec<Source>) {
    let words = words(text);
    let mut ranked: Vec<(&Candidate, f64)> = candidates
        .iter()
        .map(|c| (c, score(&words, c, idf)))
        .filter(|(c, (shared, _))| {
            *shared >= MIN_SHARED && *shared as f64 >= MIN_COVERAGE * c.words.len() as f64
        })
        .map(|(c, (_, s))| (c, s))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Far below the best match is another story that happens to share words.
    let floor = ranked.first().map_or(0.0, |(_, s)| s / 2.0);
    for (c, s) in ranked {
        if picked.len() >= MAX_SOURCES || s < floor {
            break;
        }
        let theirs = picked.iter().filter(|p| p.handle == c.source.handle).count();
        // The same story posted twice, or reworded once, is one source.
        let repeat = picked.iter().any(|p| {
            p.id == c.source.id
                || candidates
                    .iter()
                    .find(|o| o.source.id == p.id)
                    .is_some_and(|o| overlap(&o.words, &c.words) >= SAME_POST)
        });
        if theirs < MAX_PER_ACCOUNT && !repeat {
            picked.push(c.source.clone());
        }
    }
}

/// Two posts sharing this much of their words say the same thing.
const SAME_POST: f64 = 0.7;

/// Words in common against all words of the two, from 0 to 1.
fn overlap(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 1.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// Rare words shared, and their weight: a word every post uses says nothing.
fn score(words: &HashSet<String>, c: &Candidate, idf: &HashMap<&str, f64>) -> (usize, f64) {
    let weights: Vec<f64> = c
        .words
        .intersection(words)
        .filter_map(|w| idf.get(w.as_str()).copied())
        .collect();
    (weights.len(), weights.iter().sum())
}

fn candidates(posts: &[Tweet]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut add = |tweet: &Tweet, text: String, names: Vec<String>| match at.get(&tweet.id) {
        Some(&i) => {
            let existing: &mut Candidate = &mut out[i];
            for name in names {
                if !existing.names.contains(&name) {
                    existing.names.push(name);
                }
            }
        }
        None => {
            at.insert(tweet.id.clone(), out.len());
            out.push(Candidate {
                source: Source {
                    handle: tweet.handle.clone(),
                    id: tweet.id.clone(),
                    snippet: snippet(&tweet.text),
                    named: None,
                    line: None,
                },
                words: words(&text),
                names,
            });
        }
    };

    for t in posts {
        let mut names = vec![t.handle.to_lowercase()];
        names.extend(t.retweeted_by.iter().map(|h| h.to_lowercase()));
        let mut text = t.text.clone();
        // A quote of someone else is commentary on it, so it shares its words.
        // A quote of oneself ("Update:") would only outscore the original.
        if let Some(q) = t.quoted.as_ref().filter(|q| q.handle != t.handle) {
            text.push(' ');
            text.push_str(&q.text);
        }
        add(t, text, names);
        if let Some(q) = &t.quoted {
            add(q, q.text.clone(), vec![q.handle.to_lowercase()]);
        }
    }
    out
}

/// The weight of each rare word. Common words are left out, so they never count.
fn idf(candidates: &[Candidate]) -> HashMap<&str, f64> {
    let mut df: HashMap<&str, usize> = HashMap::new();
    for c in candidates {
        for w in &c.words {
            *df.entry(w.as_str()).or_default() += 1;
        }
    }
    let n = candidates.len() as f64;
    let most = (n * RARE).max(3.0);
    df.into_iter()
        .filter(|(_, d)| *d as f64 <= most)
        .map(|(w, d)| (w, ((n + 1.0) / d as f64).ln()))
        .collect()
}

fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(SNIPPET_CHARS) {
        Some((end, _)) => format!("{}…", flat[..end].trim_end()),
        None => flat,
    }
}

/// Words too common to tell one story from another, in the two languages the
/// timeline mostly speaks. Words under four letters are dropped anyway.
const COMMON: &[&str] = &[
    "aussi", "selon", "avoir", "sans", "avec", "pour", "dans", "plus", "moins", "après", "avant",
    "depuis", "encore", "alors", "ainsi", "comme", "mais", "donc", "dont", "très", "tout", "tous",
    "toute", "toutes", "leur", "leurs", "cette", "celle", "ceux", "elle", "elles", "nous", "vous",
    "sont", "était", "être", "fait", "faire", "chez", "entre", "sous", "vers", "quand", "quel",
    "quelle", "notamment", "même", "autre", "autres", "deux", "trois", "quatre", "fois", "jour",
    "jours", "désormais", "environ", "aujourd", "affirme", "annonce", "indique", "rapporte",
    "précise", "relaie", "estime", "juge", "explique", "ajoute", "dit", "écrit", "réagit",
    "that", "this", "with", "from", "have", "been", "were", "will", "would", "about", "after",
    "their", "there", "what", "when", "which", "just", "more", "than", "they", "them", "your",
    "also", "into", "only", "over", "like", "some", "says", "said", "here", "being", "very",
];

/// Lowercased words worth matching on. Short words are mostly grammar; a short
/// number is not — "15 millions" is the whole story.
fn words(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4 || (w.len() >= 2 && w.chars().any(|c| c.is_ascii_digit())))
        .filter(|w| !COMMON.contains(w))
        .map(str::to_string)
        .collect()
}

/// The @handles in a line, lowercased, in order, each once.
fn handles(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in text.split('@').skip(1) {
        let handle: String = part
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let handle = handle.to_lowercase();
        if (1..=15).contains(&handle.len()) && !out.contains(&handle) {
            out.push(handle);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn post(id: &str, handle: &str, text: &str) -> Tweet {
        Tweet {
            id: id.into(),
            handle: handle.into(),
            created_at: Utc::now(),
            text: text.into(),
            likes: 0,
            retweets: 0,
            replies: 0,
            is_reply: false,
            has_media: false,
            retweeted_by: None,
            quoted: None,
        }
    }

    fn ids(s: &Sourced) -> Vec<&str> {
        s.posts.iter().map(|p| p.id.as_str()).collect()
    }

    #[test]
    fn a_named_handle_links_to_its_post_closest_to_the_story() {
        let posts = [
            post("1", "Alice", "Mon café ce matin était froid"),
            post("2", "Alice", "Le ZEVENT dépasse les 15 millions d'euros"),
        ];
        let text = "## Le ZEVENT franchit les 15 millions [new]\nLa collecte du ZEVENT passe 15 millions, salue @alice.";
        let out = attribute(text, &posts);
        assert_eq!(out[0].heading, "Le ZEVENT franchit les 15 millions");
        assert_eq!(ids(&out[0]), ["2"]);
    }

    #[test]
    fn a_named_handle_with_nothing_in_common_gets_no_post() {
        let posts = [post("1", "bob", "Recette de crêpes"), post("2", "bob", "Météo pluvieuse demain")];
        let out = attribute("## Fusée\nLancement réussi selon @bob.", &posts);
        assert!(out[0].posts.is_empty());
    }

    #[test]
    fn a_retweeter_named_in_the_story_links_to_what_they_retweeted() {
        let mut rt = post("9", "nasa", "Artemis launch window opens");
        rt.retweeted_by = Some("carol".into());
        let out = attribute("## Artemis\nLa fenêtre de tir s'ouvre, relaie @Carol.", &[rt]);
        assert_eq!(ids(&out[0]), ["9"]);
        assert_eq!(out[0].posts[0].handle, "nasa");
        assert_eq!(out[0].posts[0].named.as_deref(), Some("carol"));
    }

    #[test]
    fn unnamed_posts_fill_in_only_when_they_share_enough_words() {
        let posts = [
            post("1", "dan", "Grève SNCF demain, trafic perturbé partout"),
            post("2", "eve", "Retards prévus : grève SNCF demain matin, trafic très perturbé sur les lignes TER"),
            post("3", "fay", "Demain il fera beau"),
        ];
        let text = "## Grève SNCF demain\nLe trafic sera perturbé demain, prévient @dan.";
        let out = attribute(text, &posts);
        assert_eq!(ids(&out[0]), ["1", "2"]);
    }

    #[test]
    fn a_post_sent_twice_is_one_source() {
        let posts = [
            post("1", "dan", "Grève SNCF demain, trafic perturbé partout en France"),
            post("2", "eve", "Grève SNCF demain : trafic perturbé partout en France"),
            post("3", "eve", "Grève SNCF demain, trafic perturbé partout en France !"),
        ];
        let out = attribute("## Grève SNCF demain\nLe trafic sera perturbé partout.", &posts);
        assert_eq!(ids(&out[0]), ["1"]);
    }

    #[test]
    fn each_also_line_is_matched_on_its_own_handle() {
        let posts = [post("1", "gus", "Nouveau MacBook annoncé"), post("2", "hal", "Résultats du match")];
        let text = "## Also\n- Nouveau MacBook — @gus\n- Le match de ce soir — @hal";
        let out = attribute(text, &posts);
        assert_eq!(out[0].heading, "Also");
        assert_eq!(ids(&out[0]), ["1", "2"]);
    }

    #[test]
    fn an_account_named_on_two_also_lines_leads_to_a_post_per_line() {
        let posts = [
            post("1", "afpfr", "Omar Sy sera jugé le 30 novembre"),
            post("2", "afpfr", "Alcaraz remporte le Masters de Paris"),
        ];
        let text = "## Also\n- Omar Sy jugé le 30 novembre. @afpfr\n- Alcaraz remporte le Masters. @afpfr";
        let out = attribute(text, &posts);
        assert_eq!(ids(&out[0]), ["1", "2"]);
        let lines: Vec<_> = out[0].posts.iter().map(|p| p.line).collect();
        assert_eq!(lines, [Some(0), Some(1)]);
    }

    #[test]
    fn a_leftover_post_sharing_nothing_is_not_a_guess() {
        let posts = [
            post("1", "afpfr", "Omar Sy sera jugé le 30 novembre"),
            post("2", "afpfr", "Recette de crêpes"),
        ];
        let text = "## Also\n- Omar Sy jugé le 30 novembre. @afpfr\n- Le Rhum gagné en record. @afpfr";
        let out = attribute(text, &posts);
        assert_eq!(ids(&out[0]), ["1"]);
    }

    #[test]
    fn every_story_keeps_its_place_even_with_no_post() {
        let posts = [post("1", "gus", "Nouveau MacBook annoncé")];
        let out = attribute("## Fusée\nRien.\n## Nouveau MacBook\nAnnoncé par @gus.", &posts);
        assert_eq!(out.len(), 2);
        assert!(out[0].posts.is_empty());
        assert_eq!(ids(&out[1]), ["1"]);
    }
}
