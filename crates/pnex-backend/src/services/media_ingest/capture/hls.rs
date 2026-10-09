//! Minimal HLS playlist reader (RFC 8216) for audio capture (D160). The
//! fetcher alone interprets playlists; every URI read here is resolved
//! against the playlist URL and re-checked by the guarded client before it
//! is fetched. Encrypted media (`EXT-X-KEY` other than `NONE`) is refused.

use chrono::{DateTime, Utc};

/// Largest playlist accepted (D160).
pub const MAX_PLAYLIST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    /// `CODECS` attribute, lowercase (`mp4a.40.2,avc1…`).
    pub codecs: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaSegment {
    pub uri: String,
    pub sequence: u64,
    pub duration_secs: f64,
    pub program_date_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaPlaylist {
    pub target_duration_secs: u64,
    pub segments: Vec<MediaSegment>,
    /// `EXT-X-MAP` URI (fMP4 init section).
    pub map_uri: Option<String>,
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Playlist {
    Master(Vec<Variant>),
    Media(MediaPlaylist),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsError {
    NotAPlaylist,
    Encrypted,
    Empty,
}

/// Value of `KEY=` in an attribute list (quoted or not).
fn attr<'a>(list: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = list;
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = &rest[eq + 1..];
        let (value, next) = if let Some(stripped) = after.strip_prefix('"') {
            let end = stripped.find('"')?;
            (&stripped[..end], &stripped[end + 1..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(value);
        }
        rest = next.trim_start_matches(',');
    }
    None
}

pub fn parse(text: &str) -> Result<Playlist, HlsError> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if lines.next() != Some("#EXTM3U") {
        return Err(HlsError::NotAPlaylist);
    }
    let mut variants = Vec::new();
    let mut media = MediaPlaylist::default();
    let mut sequence = 0u64;
    let mut pending_variant: Option<(u64, String)> = None;
    let mut pending_duration: Option<f64> = None;
    let mut pending_pdt: Option<DateTime<Utc>> = None;
    for line in lines {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let bandwidth = attr(rest, "BANDWIDTH")
                .and_then(|b| b.parse().ok())
                .unwrap_or(0);
            let codecs = attr(rest, "CODECS")
                .unwrap_or_default()
                .to_ascii_lowercase();
            pending_variant = Some((bandwidth, codecs));
        } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            media.target_duration_secs = rest.parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            sequence = rest.parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let dur = rest.split(',').next().unwrap_or_default();
            pending_duration = Some(dur.parse().unwrap_or(0.0));
        } else if let Some(rest) = line.strip_prefix("#EXT-X-PROGRAM-DATE-TIME:") {
            pending_pdt = DateTime::parse_from_rfc3339(rest)
                .ok()
                .map(|t| t.with_timezone(&Utc));
        } else if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
            if attr(rest, "METHOD").is_some_and(|m| !m.eq_ignore_ascii_case("NONE")) {
                return Err(HlsError::Encrypted);
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            media.map_uri = attr(rest, "URI").map(str::to_string);
        } else if line == "#EXT-X-ENDLIST" {
            media.ended = true;
        } else if line.starts_with('#') {
            // Other tags: ignored.
        } else if let Some((bandwidth, codecs)) = pending_variant.take() {
            variants.push(Variant {
                uri: line.to_string(),
                bandwidth,
                codecs,
            });
        } else if let Some(duration_secs) = pending_duration.take() {
            media.segments.push(MediaSegment {
                uri: line.to_string(),
                sequence,
                duration_secs,
                program_date_time: pending_pdt.take(),
            });
            sequence += 1;
        }
    }
    if !variants.is_empty() {
        return Ok(Playlist::Master(variants));
    }
    if media.segments.is_empty() && !media.ended {
        return Err(HlsError::Empty);
    }
    Ok(Playlist::Media(media))
}

/// Variant to capture: the lowest bandwidth one that carries audio (an
/// audio-only rendition when there is one). Speech needs little.
pub fn pick_variant(variants: &[Variant]) -> Option<&Variant> {
    let audio_only = |v: &&Variant| {
        !v.codecs.is_empty() && !v.codecs.contains("avc") && !v.codecs.contains("hvc")
    };
    variants
        .iter()
        .filter(audio_only)
        .min_by_key(|v| v.bandwidth)
        .or_else(|| variants.iter().min_by_key(|v| v.bandwidth))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_playlist_picks_lowest_audio_variant() {
        let text = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,CODECS=\"avc1.4d401f,mp4a.40.2\"\nvideo.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=192000,CODECS=\"mp4a.40.2\"\nhigh.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.5\"\nlow.m3u8\n";
        let Playlist::Master(v) = parse(text).unwrap() else {
            panic!("master expected")
        };
        assert_eq!(v.len(), 3);
        assert_eq!(pick_variant(&v).unwrap().uri, "low.m3u8");
    }

    #[test]
    fn media_playlist_tracks_sequence_and_pdt() {
        let text = "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:120\n\
            #EXT-X-PROGRAM-DATE-TIME:2026-10-10T08:00:00.000Z\n#EXTINF:4.0,\nseg120.aac\n\
            #EXTINF:4.0,\nseg121.aac\n";
        let Playlist::Media(m) = parse(text).unwrap() else {
            panic!("media expected")
        };
        assert_eq!(m.target_duration_secs, 4);
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.segments[0].sequence, 120);
        assert_eq!(m.segments[1].sequence, 121);
        assert!(m.segments[0].program_date_time.is_some());
        assert!(m.segments[1].program_date_time.is_none());
        assert!(!m.ended);
    }

    #[test]
    fn encrypted_and_garbage_are_refused() {
        let enc = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://k/\"\n#EXTINF:4,\na.ts\n";
        assert_eq!(parse(enc), Err(HlsError::Encrypted));
        let clear = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\na.ts\n";
        assert!(parse(clear).is_ok());
        assert_eq!(parse("<html>"), Err(HlsError::NotAPlaylist));
        assert_eq!(
            parse("#EXTM3U\n#EXT-X-TARGETDURATION:4\n"),
            Err(HlsError::Empty)
        );
    }

    #[test]
    fn map_uri_is_read() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n#EXTINF:2,\n1.m4s\n";
        let Playlist::Media(m) = parse(text).unwrap() else {
            panic!()
        };
        assert_eq!(m.map_uri.as_deref(), Some("init.mp4"));
    }
}
