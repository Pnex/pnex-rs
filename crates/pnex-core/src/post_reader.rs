//! Posts of a followed account read from the JSON a platform's web app
//! receives, as its pack declares them (pipelines.md D951): the engine
//! keeps a grammar of paths and shapes, the pack names every field.
//!
//! A path is dot-separated: a key, an index, `*` (every element), and on a
//! list a filter `[k=a|b]` / `[k!=a]` (missing `k` passes `!=`) or `^k`
//! (the element with the highest `k`). Every value a path reaches is a
//! candidate; a pick takes the first non-empty one of its paths.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::browser_agent::{CaptionCue, SeenPost};

/// Paths tried in order, and how the value found is converted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "PickForm", into = "PickForm")]
pub struct Pick {
    pub from: Vec<String>,
    /// `clock` (`1:02:03` → ms), `seconds` (→ ms), `unix` (seconds →
    /// RFC 3339), `lang` (`fra-FR` → `fr-FR`), or a date format (`%a %b…`).
    pub as_: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum PickForm {
    Paths(Vec<String>),
    Full {
        from: Vec<String>,
        #[serde(rename = "as")]
        as_: Option<String>,
    },
}

impl From<PickForm> for Pick {
    fn from(f: PickForm) -> Self {
        match f {
            PickForm::Paths(from) => Pick { from, as_: None },
            PickForm::Full { from, as_ } => Pick { from, as_ },
        }
    }
}

impl From<Pick> for PickForm {
    fn from(p: Pick) -> Self {
        match p.as_ {
            None => PickForm::Paths(p.from),
            as_ => PickForm::Full { from: p.from, as_ },
        }
    }
}

/// How the posts of a document are found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostReader {
    /// Path of the list of posts; absent: every object of the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
    pub shapes: Vec<Shape>,
    pub id: IdRule,
}

/// One kind of post object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// The object sits under this key; absent: the object itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Path → values it must hold (`a|b`), `*` for any.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub when: BTreeMap<String, String>,
    pub id: Pick,
    #[serde(default)]
    pub text: Pick,
    /// Its author; must be the followed account. Absent: that account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Pick>,
    #[serde(default)]
    pub published: Pick,
    #[serde(default)]
    pub duration: Pick,
    /// Address of its video file, on one of the plan's sites.
    #[serde(default)]
    pub video: Pick,
    /// The video is the post's own page, recorded as it plays.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub video_page: bool,
    /// A short video: its address is the plan's `short`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub short: bool,
    /// Its caption track, listed with the post.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captions: Option<Track>,
}

/// Which ids are a post's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdRule {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub digits: bool,
    /// Characters allowed besides `a-zA-Z0-9`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chars: String,
    #[serde(default)]
    pub min: usize,
    pub max: usize,
}

/// A caption track: its address and language.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub url: Pick,
    #[serde(default)]
    pub lang: Pick,
    /// Added to the address's query (`fmt=json3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// A caption track found on the post's own page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageTrack {
    /// Global of the page that holds the track list.
    pub page_data: String,
    pub track: Track,
}

/// The form of a caption file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Cues {
    #[default]
    Webvtt,
    /// JSON: a list of cues, each with its start, length (ms) and text.
    Json {
        list: String,
        start: String,
        duration: String,
        text: String,
    },
}

/// Every value `path` reaches in `v`.
pub fn resolve<'a>(v: &'a Value, path: &str) -> Vec<&'a Value> {
    let mut at = vec![v];
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        let (name, rest) = match seg.find(['[', '^']) {
            Some(i) => seg.split_at(i),
            None => (seg, ""),
        };
        at = at
            .into_iter()
            .flat_map(|x| -> Vec<&Value> {
                match name {
                    "*" => x.as_array().map(|a| a.iter().collect()).unwrap_or_default(),
                    n => match n.parse::<usize>() {
                        Ok(i) if x.is_array() => x.get(i).into_iter().collect(),
                        _ => x.get(n).into_iter().collect(),
                    },
                }
            })
            .collect();
        if !rest.is_empty() {
            at = at.into_iter().flat_map(|x| select(x, rest)).collect();
        }
    }
    at
}

/// Elements of a list kept by `[k=a|b]`, `[k!=a]` and `^k`.
fn select<'a>(list: &'a Value, ops: &str) -> Vec<&'a Value> {
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<&Value> = items.iter().collect();
    let mut rest = ops;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('[') {
            let Some((cond, after)) = r.split_once(']') else {
                return Vec::new();
            };
            rest = after;
            let (key, values, not) = match cond.split_once("!=") {
                Some((k, v)) => (k, v, true),
                None => match cond.split_once('=') {
                    Some((k, v)) => (k, v, false),
                    None => return Vec::new(),
                },
            };
            out.retain(|x| {
                let held = x.get(key).map(scalar);
                let hit = held
                    .as_deref()
                    .is_some_and(|h| values.split('|').any(|v| v == h));
                hit != not
            });
        } else if let Some(r) = rest.strip_prefix('^') {
            let end = r.find(['[', '^']).unwrap_or(r.len());
            let key = &r[..end];
            rest = &r[end..];
            let best = out
                .iter()
                .copied()
                .max_by_key(|x| x.get(key).and_then(Value::as_i64).unwrap_or(i64::MIN));
            out = best.into_iter().collect();
        } else {
            return Vec::new();
        }
    }
    out
}

/// A scalar as text; `None` for null, a list or an object.
fn scalar_opt(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn scalar(v: &Value) -> String {
    scalar_opt(v).unwrap_or_default()
}

impl Pick {
    /// The first value found, converted; `None` when none converts.
    pub fn text(&self, v: &Value) -> Option<String> {
        self.from
            .iter()
            .flat_map(|p| resolve(v, p))
            .filter_map(scalar_opt)
            .find_map(|raw| convert(&raw, self.as_.as_deref()))
    }
}

fn convert(raw: &str, as_: Option<&str>) -> Option<String> {
    Some(match as_ {
        None => raw.to_string(),
        Some("clock") => raw
            .split(':')
            .try_fold(0i64, |acc, p| {
                Some(acc * 60 + p.trim().parse::<i64>().ok()?)
            })
            .map(|s| (s * 1000).to_string())?,
        Some("seconds") => (raw.parse::<f64>().ok()? * 1000.0).round().to_string(),
        Some("unix") => chrono::DateTime::from_timestamp(raw.parse().ok()?, 0)?
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
        Some("lang") => short_lang(raw),
        Some(f) if f.starts_with('%') => {
            chrono::DateTime::parse_from_str(raw, f).ok()?.to_rfc3339()
        }
        Some(_) => return None,
    })
}

/// `fra-FR` → `fr-FR` (BCP 47 wants the two-letter language).
fn short_lang(raw: &str) -> String {
    let (lang, region) = match raw.split_once('-') {
        Some((l, r)) => (l, Some(r)),
        None => (raw, None),
    };
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
    match region {
        Some(r) => format!("{lang}-{r}"),
        None => lang.to_string(),
    }
}

/// Whether `url` is on one of `sites` (`https://*.d/` covers d's subdomains).
pub fn on_site(url: &str, sites: &[String]) -> bool {
    sites.iter().any(|s| match s.strip_prefix("https://*.") {
        Some(rest) => {
            let domain = rest.trim_end_matches('/');
            url.strip_prefix("https://")
                .and_then(|u| u.split('/').next())
                .is_some_and(|host| host.ends_with(&format!(".{domain}")))
        }
        None => url.starts_with(s.as_str()),
    })
}

impl IdRule {
    fn accepts(&self, id: &str) -> bool {
        (self.min..=self.max).contains(&id.len())
            && id.chars().all(|c| match self.digits {
                true => c.is_ascii_digit(),
                false => c.is_ascii_alphanumeric() || self.chars.contains(c),
            })
    }
}

/// A post and, when listed with it, its caption track (language, address).
pub type Found = (SeenPost, Option<(String, String)>);

/// Posts of `handle` in one JSON document, by id.
pub fn posts_of(plan: &crate::browser_agent::FollowPlan, body: &str, handle: &str) -> Vec<Found> {
    let (Some(reader), Ok(json)) = (&plan.posts, serde_json::from_str::<Value>(body)) else {
        return Vec::new();
    };
    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let mut take = |shape: &Shape, v: &Value| {
        if let Some(f) = post_of(plan, reader, shape, v, handle) {
            found.entry(f.0.platform_id.clone()).or_insert(f);
        }
    };
    match &reader.list {
        Some(list) => {
            for item in resolve(&json, list)
                .into_iter()
                .flat_map(|l| l.as_array().into_iter().flatten())
            {
                for shape in reader.shapes.iter().filter(|s| s.key.is_none()) {
                    take(shape, item);
                }
            }
        }
        None => walk(&json, &reader.shapes, &mut take),
    }
    found.into_values().collect()
}

fn walk(v: &Value, shapes: &[Shape], take: &mut impl FnMut(&Shape, &Value)) {
    match v {
        Value::Object(map) => {
            for shape in shapes.iter().filter(|s| s.key.is_none()) {
                take(shape, v);
            }
            for (k, child) in map {
                for shape in shapes.iter().filter(|s| s.key.as_deref() == Some(k)) {
                    take(shape, child);
                }
                walk(child, shapes, take);
            }
        }
        Value::Array(items) => items.iter().for_each(|c| walk(c, shapes, take)),
        _ => {}
    }
}

fn post_of(
    plan: &crate::browser_agent::FollowPlan,
    reader: &PostReader,
    shape: &Shape,
    v: &Value,
    handle: &str,
) -> Option<Found> {
    for (path, want) in &shape.when {
        let held: Vec<String> = resolve(v, path)
            .into_iter()
            .filter_map(scalar_opt)
            .collect();
        let ok = match want.as_str() {
            "*" => !held.is_empty(),
            w => held.iter().any(|h| w.split('|').any(|x| x == h)),
        };
        if !ok {
            return None;
        }
    }
    let id = shape.id.text(v).filter(|id| reader.id.accepts(id))?;
    let author = match &shape.author {
        Some(p) => p.text(v).filter(|a| a.eq_ignore_ascii_case(handle))?,
        None => handle.to_string(),
    };
    let url = plan.post_url(&author, &id, shape.short);
    let video = match shape.video_page {
        true => Some(url.clone()),
        false => shape.video.text(v).filter(|u| on_site(u, &plan.sites)),
    };
    let captions = shape
        .captions
        .as_ref()
        .and_then(|t| track_of(t, v, &plan.sites));
    let post = SeenPost {
        platform_id: id,
        url,
        author,
        text: shape.text.text(v).unwrap_or_default(),
        published_at: shape.published.text(v),
        duration_ms: shape.duration.text(v).and_then(|d| d.parse().ok()),
        video_url: video,
    };
    Some((post, captions))
}

/// A caption track of `v`: its language (`und` when unknown) and address.
pub fn track_of(t: &Track, v: &Value, sites: &[String]) -> Option<(String, String)> {
    let base = t.url.text(v).filter(|u| on_site(u, sites))?;
    let url = match &t.query {
        Some(q) => format!("{base}{}{q}", if base.contains('?') { '&' } else { '?' }),
        None => base,
    };
    Some((t.lang.text(v).unwrap_or_else(|| "und".into()), url))
}

/// Whether a response of the web app is one the plan reads.
pub fn is_response(plan: &crate::browser_agent::FollowPlan, url: &str) -> bool {
    plan.responses.iter().any(|r| url.contains(r.as_str()))
}

/// Cues of a caption file; a cue without text is dropped.
pub fn cues_of(form: &Cues, body: &str) -> Vec<CaptionCue> {
    let cue = |start_ms: i64, end_ms: i64, text: &str| {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!text.is_empty() && end_ms >= start_ms).then(|| CaptionCue {
            start_ms,
            end_ms,
            text: text.chars().take(2000).collect(),
        })
    };
    match form {
        Cues::Webvtt => {
            let mut out = Vec::new();
            let mut lines = body.lines().peekable();
            while let Some(line) = lines.next() {
                let Some((start, end)) = line.split_once("-->") else {
                    continue;
                };
                let (Some(s), Some(e)) = (vtt_time(start), vtt_time(end)) else {
                    continue;
                };
                let mut text = Vec::new();
                while let Some(next) = lines.peek() {
                    if next.trim().is_empty() {
                        break;
                    }
                    text.push(lines.next().unwrap_or_default());
                }
                out.extend(cue(s, e, &text.join(" ")));
            }
            out
        }
        Cues::Json {
            list,
            start,
            duration,
            text,
        } => {
            let Ok(json) = serde_json::from_str::<Value>(body) else {
                return Vec::new();
            };
            resolve(&json, list)
                .into_iter()
                .flat_map(|l| l.as_array().into_iter().flatten())
                .filter_map(|e| {
                    let s = resolve(e, start).first()?.as_i64()?;
                    let d = resolve(e, duration)
                        .first()
                        .and_then(|d| d.as_i64())
                        .unwrap_or(0);
                    let t: String = resolve(e, text)
                        .into_iter()
                        .filter_map(Value::as_str)
                        .collect();
                    cue(s, s + d.max(1), &t)
                })
                .collect()
        }
    }
}

/// `00:01:02.500` or `01:02.500` (cue settings after the time ignored).
fn vtt_time(raw: &str) -> Option<i64> {
    let raw = raw.split_whitespace().next()?;
    let (clock, millis) = raw.split_once('.')?;
    let secs = clock
        .split(':')
        .try_fold(0i64, |acc, p| Some(acc * 60 + p.parse::<i64>().ok()?))?;
    Some(secs * 1000 + millis.parse::<i64>().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_filter_pick_the_highest_and_collect() {
        let v = json!({"m": [
            {"type": "photo"},
            {"type": "video", "vs": [{"t": "mp4", "b": 1, "u": "lo"}, {"t": "m3u8", "b": 9, "u": "hls"}, {"t": "mp4", "b": 5, "u": "hi"}]}
        ], "segs": [{"s": "a"}, {"s": "b"}], "tracks": [{"kind": "asr", "u": 1}, {"u": 2}]});
        let first = |p: &str| resolve(&v, p).first().map(|x| scalar(x));
        assert_eq!(
            first("m[type=video|gif].vs[t=mp4]^b.u").as_deref(),
            Some("hi")
        );
        assert_eq!(first("tracks[kind!=asr].u").as_deref(), Some("2"));
        assert_eq!(first("m.0.type").as_deref(), Some("photo"));
        assert_eq!(resolve(&v, "segs.*.s").len(), 2);
        assert!(
            resolve(&v, "m[type").is_empty(),
            "a broken filter finds nothing"
        );
    }

    #[test]
    fn conversions() {
        let c = |raw, as_| convert(raw, Some(as_));
        assert_eq!(c("1:02:03", "clock").as_deref(), Some("3723000"));
        assert_eq!(c("42", "seconds").as_deref(), Some("42000"));
        assert_eq!(c("0", "unix").as_deref(), Some("1970-01-01T00:00:00Z"));
        assert_eq!(
            c("951782400", "unix").as_deref(),
            Some("2000-02-29T00:00:00Z")
        );
        assert_eq!(c("fra-FR", "lang").as_deref(), Some("fr-FR"));
        assert_eq!(
            c("Mon Jan 05 01:02:03 -0130 2026", "%a %b %d %H:%M:%S %z %Y").as_deref(),
            Some("2026-01-05T01:02:03-01:30")
        );
        assert!(c("garbage", "%a %b %d %H:%M:%S %z %Y").is_none());
        assert!(
            c("1", "nope").is_none(),
            "an unknown conversion keeps nothing"
        );
    }

    #[test]
    fn sites_and_webvtt() {
        let sites = vec![
            "https://x.com/".to_string(),
            "https://*.tiktok.com/".to_string(),
        ];
        assert!(on_site("https://x.com/a", &sites));
        assert!(on_site("https://v16.tiktok.com/a.mp4", &sites));
        assert!(!on_site("https://eviltiktok.com/a", &sites));
        assert!(!on_site("http://x.com/a", &sites));
        let vtt = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500 align:start\nHello\nworld\n\n00:03.000 --> 00:04.000\n\n";
        let cues = cues_of(&Cues::Webvtt, vtt);
        assert_eq!(cues.len(), 1, "an empty cue is dropped");
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (1_000, 2_500));
        assert_eq!(cues[0].text, "Hello world");
    }
}
