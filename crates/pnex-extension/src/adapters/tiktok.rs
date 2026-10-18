//! TikTok adapter (D282, lot L3): posts of an account read from the
//! `item_list` responses the TikTok web app receives while the profile is
//! scrolled, with their play address and the captions TikTok publishes
//! (WebVTT, often automatic). TikTok pauses in a hidden tab: its missions
//! need the agent's window in front (D269, measured in the test campaign).

use std::collections::BTreeMap;

use pnex_core::browser_agent::{CaptionCue, SeenPost};
use serde_json::Value;

/// Responses that carry the posts of a profile.
pub fn is_list_url(url: &str) -> bool {
    url.contains("/api/post/item_list")
}

/// A post and, when TikTok has one, its caption track (language, URL).
pub type PostWithCaptions = (SeenPost, Option<(String, String)>);

/// Posts of `handle` in one response, by id; photo posts carry no video.
pub fn posts_of(body: &str, handle: &str) -> Vec<PostWithCaptions> {
    let Ok(json) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut found = BTreeMap::new();
    for item in json["itemList"].as_array().into_iter().flatten() {
        let Some(id) = item["id"]
            .as_str()
            .filter(|id| id.chars().all(|c| c.is_ascii_digit()))
        else {
            continue;
        };
        let author = item["author"]["uniqueId"].as_str().unwrap_or_default();
        if !author.eq_ignore_ascii_case(handle) {
            continue;
        }
        let video = &item["video"];
        let play = video["playAddr"]
            .as_str()
            .or_else(|| video["downloadAddr"].as_str())
            .filter(|u| u.starts_with("https://"));
        let captions = video["subtitleInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["Format"].as_str() == Some("webvtt"))
            .and_then(|s| {
                let url = s["Url"].as_str().filter(|u| u.starts_with("https://"))?;
                let lang = s["LanguageCodeName"].as_str().unwrap_or("und");
                Some((short_lang(lang), url.to_string()))
            });
        let post = SeenPost {
            platform_id: id.to_string(),
            url: format!("https://www.tiktok.com/@{author}/video/{id}"),
            author: author.to_string(),
            text: item["desc"].as_str().unwrap_or_default().to_string(),
            published_at: item["createTime"].as_i64().map(unix_to_rfc3339),
            video_url: play.map(str::to_string),
            duration_ms: video["duration"].as_i64().map(|s| s * 1000),
        };
        found.entry(id.to_string()).or_insert((post, captions));
    }
    found.into_values().collect()
}

/// `fra-FR` → `fr-FR` (BCP 47 wants the two-letter language).
fn short_lang(raw: &str) -> String {
    let mut parts = raw.splitn(2, '-');
    let lang = parts.next().unwrap_or("und");
    let lang = match lang {
        "fra" => "fr",
        "eng" => "en",
        "spa" => "es",
        "deu" | "ger" => "de",
        "ita" => "it",
        "por" => "pt",
        "ara" => "ar",
        other => other,
    };
    match parts.next() {
        Some(region) => format!("{lang}-{region}"),
        None => lang.to_string(),
    }
}

/// Cues of a WebVTT track (`HH:MM:SS.mmm --> HH:MM:SS.mmm` blocks).
pub fn vtt_cues(vtt: &str) -> Vec<CaptionCue> {
    let mut cues = Vec::new();
    let mut lines = vtt.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((start, end)) = line.split_once("-->") else {
            continue;
        };
        let (Some(start_ms), Some(end_ms)) = (vtt_time(start), vtt_time(end)) else {
            continue;
        };
        let mut text = Vec::new();
        while let Some(next) = lines.peek() {
            if next.trim().is_empty() {
                break;
            }
            text.push(lines.next().unwrap_or_default().trim().to_string());
        }
        let text = text.join(" ");
        if !text.is_empty() && end_ms >= start_ms {
            cues.push(CaptionCue {
                start_ms,
                end_ms,
                text: text.chars().take(2000).collect(),
            });
        }
    }
    cues
}

/// `00:01:02.500` or `01:02.500` (cue settings after the time ignored).
fn vtt_time(raw: &str) -> Option<i64> {
    let raw = raw.split_whitespace().next()?;
    let (clock, millis) = raw.split_once('.')?;
    let secs = clock
        .split(':')
        .try_fold(0i64, |acc, part| Some(acc * 60 + part.parse::<i64>().ok()?))?;
    Some(secs * 1000 + millis.parse::<i64>().ok()?)
}

/// Unix seconds → RFC 3339 UTC, without a date crate (civil-from-days).
fn unix_to_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/tiktok_item_list.json");

    #[test]
    fn reads_the_account_posts_with_play_address_and_captions() {
        let posts = posts_of(FIXTURE, "Some.Account");
        assert_eq!(posts.len(), 2, "the other author is skipped");
        let (clip, captions) = &posts[0];
        assert_eq!(clip.platform_id, "7420000000000000001");
        assert_eq!(
            clip.url,
            "https://www.tiktok.com/@some.account/video/7420000000000000001"
        );
        assert_eq!(clip.duration_ms, Some(42_000));
        assert_eq!(clip.published_at.as_deref(), Some("2025-10-18T10:04:05Z"));
        assert!(clip.video_url.as_deref().unwrap().contains("a.mp4"));
        assert_eq!(captions.as_ref().map(|(l, _)| l.as_str()), Some("fr-FR"));
        assert!(posts[1].0.video_url.is_none(), "photo post");
        assert!(is_list_url(
            "https://www.tiktok.com/api/post/item_list/?aid=1988"
        ));
    }

    #[test]
    fn webvtt_cues_and_dates() {
        let vtt = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500 align:start\nHello\nworld\n\n00:03.000 --> 00:04.000\n\n";
        let cues = vtt_cues(vtt);
        assert_eq!(cues.len(), 1, "an empty cue is dropped");
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (1_000, 2_500));
        assert_eq!(cues[0].text, "Hello world");
        assert_eq!(unix_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }
}
