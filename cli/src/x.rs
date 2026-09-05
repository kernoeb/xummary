//! X's private web GraphQL API: two home timelines, one page at a time.

use crate::cookies::Session;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

/// The bearer the x.com web app ships with. Public, and the same for everyone.
const WEB_BEARER: &str = "AAAAAAAAAAAAAAAAAAAAANRILgAAAAAAnNwIzUejRCOuH5E6I8xnZz4puTs%3D1Zv7ttfk8LF81IUq16cHjhLTvJu4FA33AGWWjCpTnA";

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

/// GraphQL operation ids. X rotates these; when a call starts returning 404,
/// copy the fresh id out of a `graphql/<id>/HomeTimeline` request in devtools.
const HOME_TIMELINE_ID: &str = "3tb-_5Lf7kdCZ1cFHmsEfg";
const HOME_LATEST_TIMELINE_ID: &str = "eObmT5Nuapp04u8bYWf49Q";

const PAGE_SIZE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    ForYou,
    Following,
}

impl Feed {
    pub const fn label(self) -> &'static str {
        match self {
            Feed::ForYou => "For You",
            Feed::Following => "Following",
        }
    }

    const fn operation(self) -> (&'static str, &'static str) {
        match self {
            Feed::ForYou => (HOME_TIMELINE_ID, "HomeTimeline"),
            Feed::Following => (HOME_LATEST_TIMELINE_ID, "HomeLatestTimeline"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tweet {
    pub id: String,
    pub handle: String,
    pub created_at: DateTime<Utc>,
    pub text: String,
    pub likes: u64,
    pub retweets: u64,
    pub replies: u64,
    pub is_reply: bool,
    pub has_media: bool,
    /// Who retweeted it into the feed, when this arrived as a retweet.
    pub retweeted_by: Option<String>,
    pub quoted: Option<Box<Tweet>>,
}

#[derive(Debug, Default)]
pub struct Page {
    pub tweets: Vec<Tweet>,
    pub next_cursor: Option<String>,
}

pub struct Client {
    http: reqwest::Client,
    session: Session,
}

impl Client {
    pub fn new(session: Session) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .gzip(true)
            .brotli(true)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { http, session })
    }

    pub async fn home_page(&self, feed: Feed, cursor: Option<&str>) -> Result<Page> {
        let (query_id, name) = feed.operation();
        let mut variables = json!({
            "count": PAGE_SIZE,
            "includePromotedContent": false,
            "latestControlAvailable": true,
            "requestContext": "launch",
            "withCommunity": true,
            "seenTweetIds": [],
        });
        if let Some(c) = cursor {
            variables["cursor"] = Value::String(c.to_string());
        }

        let body = json!({
            "variables": variables,
            "features": features(),
            "queryId": query_id,
        });

        let response = self
            .http
            .post(format!("https://x.com/i/api/graphql/{query_id}/{name}"))
            .header("authorization", format!("Bearer {WEB_BEARER}"))
            .header(
                "cookie",
                format!(
                    "auth_token={}; ct0={}",
                    self.session.auth_token, self.session.ct0
                ),
            )
            .header("x-csrf-token", &self.session.ct0)
            .header("x-twitter-active-user", "yes")
            .header("x-twitter-auth-type", "OAuth2Session")
            .header("x-twitter-client-language", "en")
            .header("content-type", "application/json")
            .header("referer", "https://x.com/home")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{name} request failed"))?;

        let status = response.status();
        let text = response.text().await.context("read response body")?;
        if !status.is_success() {
            bail!("{name} returned HTTP {status}: {}", truncate(&text, 300));
        }

        let json: Value = serde_json::from_str(&text).context("parse response as JSON")?;
        if let Some(errors) = json.get("errors").and_then(Value::as_array) {
            if !errors.is_empty() {
                let first = errors[0]
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                bail!("{name}: {first}");
            }
        }

        let instructions = json
            .pointer("/data/home/home_timeline_urt/instructions")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("{name}: no timeline in the response"))?;

        Ok(walk(instructions))
    }
}

/// Pull tweets and the bottom cursor out of the timeline instructions.
fn walk(instructions: &[Value]) -> Page {
    let mut page = Page::default();
    for instruction in instructions {
        let Some(entries) = instruction.get("entries").and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            let entry_id = entry.get("entryId").and_then(Value::as_str).unwrap_or("");
            let content = entry.get("content").unwrap_or(&Value::Null);

            if entry_id.starts_with("cursor-bottom") {
                if let Some(c) = content.get("value").and_then(Value::as_str) {
                    page.next_cursor = Some(c.to_string());
                }
                continue;
            }

            match content.get("entryType").and_then(Value::as_str) {
                // A single tweet.
                Some("TimelineTimelineItem") => {
                    if let Some(t) = tweet_from_item(content.get("itemContent")) {
                        page.tweets.push(t);
                    }
                }
                // A conversation: several tweets under one entry.
                Some("TimelineTimelineModule") => {
                    let items = content.get("items").and_then(Value::as_array);
                    for item in items.into_iter().flatten() {
                        let inner = item.pointer("/item/itemContent");
                        if let Some(t) = tweet_from_item(inner) {
                            page.tweets.push(t);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    page
}

fn tweet_from_item(item_content: Option<&Value>) -> Option<Tweet> {
    let item = item_content?;
    if item.get("itemType").and_then(Value::as_str) != Some("TimelineTweet") {
        return None;
    }
    parse_tweet(item.pointer("/tweet_results/result")?)
}

fn parse_tweet(result: &Value) -> Option<Tweet> {
    // Restricted tweets come wrapped in a visibility envelope.
    let result = match result.get("__typename").and_then(Value::as_str) {
        Some("TweetWithVisibilityResults") => result.get("tweet")?,
        Some("TweetTombstone") => return None,
        _ => result,
    };

    let legacy = result.get("legacy")?;

    // A retweet carries the original inside it; summarize the original and
    // remember who passed it along.
    if let Some(inner) = legacy.pointer("/retweeted_status_result/result") {
        let mut tweet = parse_tweet(inner)?;
        tweet.retweeted_by = handle_of(result);
        return Some(tweet);
    }

    let text = result
        .pointer("/note_tweet/note_tweet_results/result/text")
        .and_then(Value::as_str)
        .or_else(|| legacy.get("full_text").and_then(Value::as_str))?
        .to_string();

    let quoted = legacy
        .pointer("/quoted_status_result/result")
        .or_else(|| result.pointer("/quoted_status_result/result"))
        .and_then(parse_tweet)
        .map(Box::new);

    Some(Tweet {
        id: legacy.get("id_str").and_then(Value::as_str)?.to_string(),
        handle: handle_of(result)?,
        created_at: parse_time(legacy.get("created_at").and_then(Value::as_str)?)?,
        text,
        likes: count(legacy, "favorite_count"),
        retweets: count(legacy, "retweet_count"),
        replies: count(legacy, "reply_count"),
        // A present-but-null field would read as a reply, so check for a string.
        is_reply: legacy
            .get("in_reply_to_status_id_str")
            .and_then(Value::as_str)
            .is_some(),
        has_media: legacy
            .pointer("/entities/media")
            .and_then(Value::as_array)
            .is_some_and(|m| !m.is_empty()),
        retweeted_by: None,
        quoted,
    })
}

/// X moved the handle from `legacy` into `core`; both shapes are still live.
fn handle_of(result: &Value) -> Option<String> {
    let user = result.pointer("/core/user_results/result")?;
    user.pointer("/core/screen_name")
        .or_else(|| user.pointer("/legacy/screen_name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn count(legacy: &Value, key: &str) -> u64 {
    legacy.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// X sends "Wed Oct 10 20:19:24 +0000 2018".
fn parse_time(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_str(raw, "%a %b %d %H:%M:%S %z %Y")
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn features() -> Value {
    json!({
        "articles_preview_enabled": true,
        "c9s_tweet_anatomy_moderator_badge_enabled": true,
        "communities_web_enable_tweet_community_results_fetch": true,
        "creator_subscriptions_quote_tweet_preview_enabled": false,
        "creator_subscriptions_tweet_preview_api_enabled": true,
        "freedom_of_speech_not_reach_fetch_enabled": true,
        "graphql_is_translatable_rweb_tweet_is_translatable_enabled": true,
        "longform_notetweets_consumption_enabled": true,
        "longform_notetweets_inline_media_enabled": true,
        "longform_notetweets_rich_text_read_enabled": true,
        "profile_label_improvements_pcf_label_in_post_enabled": true,
        "responsive_web_edit_tweet_api_enabled": true,
        "responsive_web_enhance_cards_enabled": false,
        "responsive_web_graphql_exclude_directive_enabled": true,
        "responsive_web_graphql_skip_user_profile_image_extensions_enabled": false,
        "responsive_web_graphql_timeline_navigation_enabled": true,
        "responsive_web_twitter_article_tweet_consumption_enabled": true,
        "rweb_tipjar_consumption_enabled": true,
        "rweb_video_timestamps_enabled": true,
        "standardized_nudges_misinfo": true,
        "tweet_awards_web_tipping_enabled": false,
        "tweet_with_visibility_results_prefer_gql_limited_actions_policy_enabled": true,
        "verified_phone_label_enabled": false,
        "view_counts_everywhere_api_enabled": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tweet_entry(id: &str, handle: &str, text: &str) -> Value {
        json!({
            "entryId": format!("tweet-{id}"),
            "content": {
                "entryType": "TimelineTimelineItem",
                "itemContent": {
                    "itemType": "TimelineTweet",
                    "tweet_results": { "result": {
                        "__typename": "Tweet",
                        "core": { "user_results": { "result": {
                            "core": { "screen_name": handle }
                        }}},
                        "legacy": {
                            "id_str": id,
                            "full_text": text,
                            "created_at": "Wed Oct 10 20:19:24 +0000 2018",
                            "favorite_count": 12,
                            "retweet_count": 3,
                            "reply_count": 1
                        }
                    }}
                }
            }
        })
    }

    #[test]
    fn reads_tweets_and_the_bottom_cursor() {
        let instructions = vec![json!({
            "type": "TimelineAddEntries",
            "entries": [
                tweet_entry("1", "alice", "hello"),
                { "entryId": "cursor-bottom-0", "content": { "value": "CURSOR" } }
            ]
        })];
        let page = walk(&instructions);
        assert_eq!(page.tweets.len(), 1);
        assert_eq!(page.tweets[0].handle, "alice");
        assert_eq!(page.tweets[0].likes, 12);
        assert_eq!(page.next_cursor.as_deref(), Some("CURSOR"));
    }

    #[test]
    fn a_retweet_reports_the_original_author() {
        let inner = tweet_entry("1", "alice", "hello")
            .pointer("/content/itemContent/tweet_results/result")
            .unwrap()
            .clone();
        let rt = json!({
            "__typename": "Tweet",
            "core": { "user_results": { "result": { "core": { "screen_name": "bob" }}}},
            "legacy": {
                "id_str": "2",
                "full_text": "RT @alice: hello",
                "created_at": "Wed Oct 10 20:19:24 +0000 2018",
                "retweeted_status_result": { "result": inner }
            }
        });
        let parsed = parse_tweet(&rt).unwrap();
        assert_eq!(parsed.handle, "alice");
        assert_eq!(parsed.retweeted_by.as_deref(), Some("bob"));
    }

    #[test]
    fn the_old_legacy_handle_still_works() {
        let user = json!({
            "core": { "user_results": { "result": { "legacy": { "screen_name": "carol" }}}}
        });
        assert_eq!(handle_of(&user).as_deref(), Some("carol"));
    }

    #[test]
    fn a_tombstone_is_skipped() {
        assert!(parse_tweet(&json!({ "__typename": "TweetTombstone" })).is_none());
    }

    #[test]
    fn a_null_in_reply_to_is_not_a_reply() {
        let mut entry = tweet_entry("1", "alice", "hello");
        entry["content"]["itemContent"]["tweet_results"]["result"]["legacy"]
            ["in_reply_to_status_id_str"] = Value::Null;
        let result = entry
            .pointer("/content/itemContent/tweet_results/result")
            .unwrap();
        assert!(!parse_tweet(result).unwrap().is_reply);
    }

    #[test]
    fn parses_x_timestamps() {
        assert!(parse_time("Wed Oct 10 20:19:24 +0000 2018").is_some());
        assert!(parse_time("not a date").is_none());
    }
}
