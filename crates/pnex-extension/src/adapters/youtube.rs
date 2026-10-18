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

/// Caption track of a watch page (`ytInitialPlayerResponse`): a track
/// written by people when there is one, else the automatic one. Returns
/// its language and the URL of its `json3` form.
pub fn caption_track(player: &str) -> Option<(String, String)> {
    let json: Value = serde_json::from_str(player).ok()?;
    let tracks = json["captions"]["playerCaptionsTracklistRenderer"]["captionTracks"].as_array()?;
    let track = tracks
        .iter()
        .find(|t| t["kind"].as_str() != Some("asr"))
        .or_else(|| tracks.first())?;
    let base = track["baseUrl"].as_str()?;
    if !base.starts_with("https://www.youtube.com/") {
        return None;
    }
    let lang = track["languageCode"].as_str().unwrap_or("und").to_string();
    let sep = if base.contains('?') { '&' } else { '?' };
    Some((lang, format!("{base}{sep}fmt=json3")))
}

/// Cues of a `json3` caption track.
pub fn cues_of(json3: &str) -> Vec<pnex_core::browser_agent::CaptionCue> {
    let Ok(json) = serde_json::from_str::<Value>(json3) else {
        return Vec::new();
    };
    json["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter_map(|e| {
                    let start = e["tStartMs"].as_i64()?;
                    let text: String = e["segs"]
                        .as_array()?
                        .iter()
                        .filter_map(|s| s["utf8"].as_str())
                        .collect::<String>()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    if text.is_empty() {
                        return None;
                    }
                    let end = start + e["dDurationMs"].as_i64().unwrap_or(0).max(1);
                    Some(pnex_core::browser_agent::CaptionCue {
                        start_ms: start,
                        end_ms: end,
                        text: text.chars().take(2000).collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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

    #[test]
    fn captions_prefer_human_tracks_and_read_json3() {
        let player = r#"{"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[
            {"baseUrl":"https://www.youtube.com/api/timedtext?v=x&lang=fr&kind=asr","languageCode":"fr","kind":"asr"},
            {"baseUrl":"https://www.youtube.com/api/timedtext?v=x&lang=en","languageCode":"en"}]}}}"#;
        let (lang, url) = caption_track(player).unwrap();
        assert_eq!(lang, "en");
        assert_eq!(
            url,
            "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=json3"
        );
        assert!(caption_track(r#"{"captions":{}}"#).is_none());
        let evil = r#"{"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[{"baseUrl":"https://evil.example/x","languageCode":"en"}]}}}"#;
        assert!(
            caption_track(evil).is_none(),
            "only YouTube's own caption URLs"
        );

        let json3 = r#"{"events":[
            {"tStartMs":0,"dDurationMs":1500,"segs":[{"utf8":"hello"},{"utf8":"  world"}]},
            {"tStartMs":1500,"dDurationMs":10,"segs":[{"utf8":"\n"}]},
            {"tStartMs":2000}]}"#;
        let cues = cues_of(json3);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "hello world");
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (0, 1500));
    }
}
