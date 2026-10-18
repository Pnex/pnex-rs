//! X adapter (extension-collector.md §8): reads the posts out of the JSON
//! the X web app already receives (`UserTweets`, `UserMedia`…), relayed by
//! the page hook. No DOM scraping, no call of its own.
//!
//! The walk is structural on purpose: X moves fields between versions
//! (`legacy.screen_name` → `core.screen_name`, `timeline_v2` → `timeline`),
//! so any object that looks like a post (`rest_id` + `legacy.full_text`) is
//! taken wherever it sits.

use std::collections::BTreeMap;

use pnex_core::browser_agent::SeenPost;
use serde_json::Value;

/// GraphQL operations whose responses carry the posts of an account.
pub fn is_timeline_url(url: &str) -> bool {
    url.contains("/graphql/")
        && ["UserTweets", "UserMedia", "UserTweetsAndReplies"]
            .iter()
            .any(|op| url.contains(&format!("/{op}")))
}

/// Posts of `handle` found in one response, by id. Reposts of other
/// accounts are skipped (their author is not the followed account).
pub fn posts_of(body: &str, handle: &str) -> Vec<SeenPost> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut found = BTreeMap::new();
    walk(&json, &mut found);
    found
        .into_values()
        .filter(|p: &SeenPost| p.author.eq_ignore_ascii_case(handle))
        .collect()
}

fn walk(v: &Value, out: &mut BTreeMap<String, SeenPost>) {
    match v {
        Value::Object(map) => {
            if let Some(post) = post_of(v) {
                out.entry(post.platform_id.clone()).or_insert(post);
            }
            map.values().for_each(|child| walk(child, out));
        }
        Value::Array(items) => items.iter().for_each(|child| walk(child, out)),
        _ => {}
    }
}

fn post_of(v: &Value) -> Option<SeenPost> {
    let id = v["rest_id"].as_str()?;
    let legacy = &v["legacy"];
    let text = legacy["full_text"].as_str()?;
    let user = &v["core"]["user_results"]["result"];
    let author = user["core"]["screen_name"]
        .as_str()
        .or_else(|| user["legacy"]["screen_name"].as_str())?;
    let video = best_video(legacy);
    Some(SeenPost {
        platform_id: id.to_string(),
        url: format!("https://x.com/{author}/status/{id}"),
        author: author.to_string(),
        text: text.to_string(),
        published_at: legacy["created_at"].as_str().and_then(rfc3339),
        video_url: video.as_ref().map(|(url, _)| url.clone()),
        duration_ms: video.and_then(|(_, d)| d),
    })
}

/// Highest-bitrate progressive mp4 of the post's first video (or GIF).
fn best_video(legacy: &Value) -> Option<(String, Option<i64>)> {
    let media = legacy["extended_entities"]["media"].as_array()?;
    let video = media
        .iter()
        .find(|m| matches!(m["type"].as_str(), Some("video" | "animated_gif")))?;
    let info = &video["video_info"];
    let url = info["variants"]
        .as_array()?
        .iter()
        .filter(|var| var["content_type"].as_str() == Some("video/mp4"))
        .max_by_key(|var| var["bitrate"].as_i64().unwrap_or(0))?["url"]
        .as_str()?
        .to_string();
    Some((url, info["duration_millis"].as_i64()))
}

/// `Fri Oct 17 18:04:05 +0000 2026` → `2026-10-17T18:04:05+00:00`.
fn rfc3339(raw: &str) -> Option<String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let [_, month, day, time, zone, year] = parts.as_slice() else {
        return None;
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .iter()
    .position(|m| m == month)?
        + 1;
    let day: u32 = day.parse().ok()?;
    let (sign, hm) = zone.split_at(1);
    if zone.len() != 5 || !matches!(sign, "+" | "-") || time.len() != 8 {
        return None;
    }
    Some(format!(
        "{year}-{month:02}-{day:02}T{time}{sign}{}:{}",
        &hm[..2],
        &hm[2..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/x_user_tweets.json");

    #[test]
    fn reads_the_account_posts_with_their_best_video() {
        let posts = posts_of(FIXTURE, "example_account");
        let ids: Vec<&str> = posts.iter().map(|p| p.platform_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "1850000000000000001",
                "1850000000000000002",
                "1850000000000000003"
            ],
            "the repost's original (another author) is skipped"
        );
        let clip = &posts[0];
        assert_eq!(
            clip.video_url.as_deref(),
            Some("https://video.twimg.com/ext_tw_video/1/pu/vid/1280x720/high.mp4")
        );
        assert_eq!(clip.duration_ms, Some(31_500));
        assert_eq!(
            clip.published_at.as_deref(),
            Some("2026-10-17T18:04:05+00:00")
        );
        assert_eq!(
            clip.url,
            "https://x.com/Example_Account/status/1850000000000000001"
        );
        assert!(posts[1].video_url.is_none(), "text only");
        assert!(
            posts[2].video_url.is_none(),
            "the repost wrapper has no media"
        );
    }

    #[test]
    fn timeline_operations_only_and_garbage_tolerated() {
        assert!(is_timeline_url(
            "https://x.com/i/api/graphql/abc/UserTweets?variables=1"
        ));
        assert!(!is_timeline_url(
            "https://x.com/i/api/graphql/abc/HomeTimeline"
        ));
        assert!(posts_of("not json", "a").is_empty());
        assert_eq!(
            rfc3339("Mon Jan 05 01:02:03 -0130 2026").as_deref(),
            Some("2026-01-05T01:02:03-01:30")
        );
        assert!(rfc3339("garbage").is_none());
    }
}
