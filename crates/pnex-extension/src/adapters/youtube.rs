//! YouTube adapter (D282): videos and shorts of a channel, read from the
//! data the page already holds (`ytInitialData`) and from the `browse`
//! responses it receives while scrolling. Structural walk, like X: the
//! renderer names change between YouTube versions, every known one is read.

use std::collections::BTreeMap;

use pnex_core::browser_agent::SeenPost;
use serde_json::Value;

/// Responses that carry more items of a channel list.
pub fn is_list_url(url: &str) -> bool {
    url.contains("/youtubei/v1/browse")
}

/// Videos and shorts found in one JSON document, by id.
pub fn posts_of(body: &str, handle: &str) -> Vec<SeenPost> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut found = BTreeMap::new();
    walk(&json, handle, &mut found);
    found.into_values().collect()
}

fn walk(v: &Value, handle: &str, out: &mut BTreeMap<String, SeenPost>) {
    match v {
        Value::Object(map) => {
            for (key, child) in map {
                if let Some(post) = item_of(key, child, handle) {
                    out.entry(post.platform_id.clone()).or_insert(post);
                }
                walk(child, handle, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|child| walk(child, handle, out)),
        _ => {}
    }
}

/// One video or short card, whatever its renderer.
fn item_of(key: &str, v: &Value, handle: &str) -> Option<SeenPost> {
    let (id, title, short, length) = match key {
        "videoRenderer" => (
            v["videoId"].as_str()?,
            v["title"]["runs"][0]["text"]
                .as_str()
                .or_else(|| v["title"]["simpleText"].as_str()),
            false,
            v["lengthText"]["simpleText"].as_str(),
        ),
        "lockupViewModel" if v["contentType"].as_str() == Some("LOCKUP_CONTENT_TYPE_VIDEO") => (
            v["contentId"].as_str()?,
            v["metadata"]["lockupMetadataViewModel"]["title"]["content"].as_str(),
            false,
            None,
        ),
        "reelItemRenderer" => (
            v["videoId"].as_str()?,
            v["headline"]["simpleText"].as_str(),
            true,
            None,
        ),
        "shortsLockupViewModel" => (
            v["onTap"]["innertubeCommand"]["reelWatchEndpoint"]["videoId"].as_str()?,
            v["overlayMetadata"]["primaryText"]["content"].as_str(),
            true,
            None,
        ),
        _ => return None,
    };
    let valid = id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return None;
    }
    let url = if short {
        format!("https://www.youtube.com/shorts/{id}")
    } else {
        format!("https://www.youtube.com/watch?v={id}")
    };
    Some(SeenPost {
        platform_id: id.to_string(),
        url: url.clone(),
        author: handle.to_string(),
        text: title.unwrap_or_default().to_string(),
        // Lists only give relative dates ("3 days ago"): the watch page has
        // the real one.
        published_at: None,
        // No file URL on YouTube: the watch page, recorded in real time.
        video_url: Some(url),
        duration_ms: length.and_then(duration_ms),
    })
}

/// `1:02:03` / `12:34` → milliseconds.
fn duration_ms(raw: &str) -> Option<i64> {
    raw.split(':')
        .try_fold(0i64, |acc, part| {
            Some(acc * 60 + part.trim().parse::<i64>().ok()?)
        })
        .map(|secs| secs * 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/youtube_channel.json");

    #[test]
    fn reads_videos_and_shorts_of_both_layouts() {
        let posts = posts_of(FIXTURE, "SomeChannel");
        let ids: Vec<&str> = posts.iter().map(|p| p.platform_id.as_str()).collect();
        assert_eq!(
            ids,
            ["AbCdEfGhIj1", "KlMnOpQrSt2", "Short_Old-5", "UvWxYz01234"],
            "playlists are skipped"
        );
        let long = &posts[0];
        assert_eq!(long.text, "Episode 12: the long talk");
        assert_eq!(long.duration_ms, Some(3_723_000));
        assert_eq!(long.url, "https://www.youtube.com/watch?v=AbCdEfGhIj1");
        assert_eq!(long.author, "SomeChannel");
        assert_eq!(
            posts[3].video_url.as_deref(),
            Some("https://www.youtube.com/shorts/UvWxYz01234")
        );
        assert!(is_list_url(
            "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false"
        ));
        assert!(!is_list_url("https://www.youtube.com/youtubei/v1/player"));
    }
}
