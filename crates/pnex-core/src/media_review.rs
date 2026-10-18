//! Review of a media file (media-review.md D271–D280): the runs of its
//! transcription, the text on the media clock, and what the review screen
//! reads. Text lives in O2 (`tx_<slug>` of the asset's batch stream), the
//! runs are small rows in Postgres.

use serde::{Deserialize, Serialize};

/// Where the text of a run comes from (D276).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    /// ASR and diarization of PNeX on a version of the file.
    Pnex,
    /// Subtitles given by the platform the media comes from: no speakers,
    /// timed per cue, never replaced (a retranscription adds a `pnex` run).
    Platform,
}

impl RunOrigin {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Pnex => "pnex",
            Self::Platform => "platform",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        [Self::Pnex, Self::Platform]
            .into_iter()
            .find(|o| o.wire() == s)
    }
}

/// One transcription run of a media asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaRun {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: uuid::Uuid,
    pub origin: RunOrigin,
    /// Version transcribed (`pnex` runs); `None` for platform subtitles.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = "uuid"))]
    pub version_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub lang: Option<String>,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created_at: String,
}

/// A word of a segment, ms from the start of the media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewWord {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Stream-local speaker label (`S1`…), never an identity.
    #[serde(default)]
    pub speaker: Option<String>,
}

/// A speaker turn, ms from the start of the media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewTurn {
    pub label: String,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A transcribed segment (or a platform cue) on the media clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    #[serde(default)]
    pub words: Vec<ReviewWord>,
    #[serde(default)]
    pub turns: Vec<ReviewTurn>,
}

/// Transcript of a media for the review screen (`GET
/// /api/v1/media/{id}/transcript`): the runs, the one shown, its segments in
/// media order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaTranscript {
    pub runs: Vec<MediaRun>,
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>, format = "uuid"))]
    pub run: Option<uuid::Uuid>,
    pub segments: Vec<ReviewSegment>,
    /// The text store (O2) is not reachable: runs are listed, no text.
    #[serde(default)]
    pub unavailable: bool,
}

/// Run shown by default: the latest `pnex` run of the current version,
/// else the latest run of any origin (`runs` newest first).
pub fn default_run(runs: &[MediaRun], current_version: Option<uuid::Uuid>) -> Option<uuid::Uuid> {
    runs.iter()
        .find(|r| {
            r.origin == RunOrigin::Pnex && r.version_id.is_some() && r.version_id == current_version
        })
        .or_else(|| runs.first())
        .map(|r| r.id)
}

/// Words and turns of a segment stored relative to the segment (as the
/// ASR gives them), moved onto the media clock.
pub fn shift_segment(
    offset_ms: i64,
    duration_ms: i64,
    text: String,
    words_json: &str,
    turns_json: &str,
) -> ReviewSegment {
    #[derive(Deserialize)]
    struct W {
        w: String,
        s: i64,
        e: i64,
        #[serde(default)]
        spk: Option<String>,
    }
    #[derive(Deserialize)]
    struct T {
        label: String,
        start_ms: i64,
        end_ms: i64,
    }
    let words: Vec<W> = serde_json::from_str(words_json).unwrap_or_default();
    let turns: Vec<T> = serde_json::from_str(turns_json).unwrap_or_default();
    ReviewSegment {
        start_ms: offset_ms,
        end_ms: offset_ms + duration_ms.max(0),
        text,
        words: words
            .into_iter()
            .map(|w| ReviewWord {
                text: w.w,
                start_ms: offset_ms + w.s,
                end_ms: offset_ms + w.e,
                speaker: w.spk,
            })
            .collect(),
        turns: turns
            .into_iter()
            .map(|t| ReviewTurn {
                label: t.label,
                start_ms: offset_ms + t.start_ms,
                end_ms: offset_ms + t.end_ms,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(origin: RunOrigin, version: Option<u128>) -> MediaRun {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        MediaRun {
            id: uuid::Uuid::from_fields(n, 0, 0, &[0; 8]),
            origin,
            version_id: version.map(|v| uuid::Uuid::from_fields(v as u32, 0, 0, &[0; 8])),
            lang: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn default_run_prefers_pnex_on_the_current_version() {
        let old = run(RunOrigin::Pnex, Some(1));
        let platform = run(RunOrigin::Platform, None);
        let current = run(RunOrigin::Pnex, Some(2));
        let runs = vec![platform.clone(), current.clone(), old];
        let v2 = Some(uuid::Uuid::from_fields(2 as u32, 0, 0, &[0; 8]));
        assert_eq!(default_run(&runs, v2), Some(current.id));
        // No run on the current version: the newest of any origin.
        assert_eq!(
            default_run(
                &runs,
                Some(uuid::Uuid::from_fields(9 as u32, 0, 0, &[0; 8]))
            ),
            Some(platform.id)
        );
        assert_eq!(default_run(&[], v2), None);
    }

    #[test]
    fn segment_words_and_turns_move_onto_the_media_clock() {
        let s = shift_segment(
            30_000,
            31_000,
            "bonjour à tous".into(),
            r#"[{"w":"bonjour","s":100,"e":600,"spk":"S1"},{"w":"à","s":700,"e":800}]"#,
            r#"[{"label":"S1","start_ms":0,"end_ms":900}]"#,
        );
        assert_eq!((s.start_ms, s.end_ms), (30_000, 61_000));
        assert_eq!(s.words[0].start_ms, 30_100);
        assert_eq!(s.words[0].speaker.as_deref(), Some("S1"));
        assert_eq!(s.words[1].speaker, None);
        assert_eq!(s.turns[0].end_ms, 30_900);
        // Garbage stays empty, never a failure.
        assert!(shift_segment(0, 1, String::new(), "nope", "")
            .words
            .is_empty());
    }
}
