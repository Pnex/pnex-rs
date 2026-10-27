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
    /// Spans attributed to persons (D272), every status, in media order.
    #[serde(default)]
    pub attributions: Vec<SpeechAttribution>,
}

/// How a span of a media got its person (D272): every technique is an
/// origin, none is privileged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttributionOrigin {
    /// A person chose the span and the person (D273).
    Manual,
    /// A consented voice profile matched (D275): always proposed first.
    Voice,
    /// The transcript names the speaker ("je reçois X").
    Transcript,
    /// The programme grid announces the guest.
    Grid,
}

impl AttributionOrigin {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Voice => "voice",
            Self::Transcript => "transcript",
            Self::Grid => "grid",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        [Self::Manual, Self::Voice, Self::Transcript, Self::Grid]
            .into_iter()
            .find(|o| o.wire() == s)
    }
}

/// A machine proposes, a person confirms (D272).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AttributionStatus {
    Proposed,
    Confirmed,
    /// Kept so the same proposal is not made again.
    Rejected,
}

impl AttributionStatus {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        [Self::Proposed, Self::Confirmed, Self::Rejected]
            .into_iter()
            .find(|o| o.wire() == s)
    }
}

/// A span of a media attributed to a person of the ontology (D272).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SpeechAttribution {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub id: uuid::Uuid,
    /// The person (ontology object).
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub object_id: uuid::Uuid,
    /// Title of the person.
    pub person: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub origin: AttributionOrigin,
    /// Score of a machine origin (0–1); `None` when manual.
    #[serde(default)]
    pub confidence: Option<f32>,
    pub status: AttributionStatus,
    /// Optimistic concurrency: sent back as `expected_version`.
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// A span on the media clock, ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaSpan {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// `POST /api/v1/media/{id}/attributions`: one person on one or several
/// spans (a whole speaker = one span per turn, D273). Confirmed at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AttributionInput {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub object_id: uuid::Uuid,
    pub spans: Vec<MediaSpan>,
}

/// `PATCH /api/v1/media/{id}/attributions/{aid}`: confirm, reject, move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AttributionPatch {
    pub expected_version: i32,
    #[serde(default)]
    pub status: Option<AttributionStatus>,
    #[serde(default)]
    pub start_ms: Option<i64>,
    #[serde(default)]
    pub end_ms: Option<i64>,
}

/// Most spans of one attribution request.
pub const ATTRIBUTION_SPANS_MAX: usize = 500;
/// Latest position on a media clock: a day.
pub const MEDIA_CLOCK_MAX_MS: i64 = 24 * 3600 * 1000;

/// Spans sorted, overlapping or touching ones merged (`start < end` kept).
pub fn merge_spans(mut spans: Vec<MediaSpan>) -> Vec<MediaSpan> {
    spans.retain(|s| s.start_ms < s.end_ms);
    spans.sort_by_key(|s| s.start_ms);
    let mut out: Vec<MediaSpan> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(last) if s.start_ms <= last.end_ms => last.end_ms = last.end_ms.max(s.end_ms),
            _ => out.push(s),
        }
    }
    out
}

/// Person of each speaker label of a run: the confirmed attribution that
/// covers most of the label's turns (D272 resolution, at the label grain
/// the transcript needs).
pub fn person_of_label<'a>(
    turns: &[ReviewTurn],
    label: &str,
    attributions: &'a [SpeechAttribution],
) -> Option<&'a SpeechAttribution> {
    let overlap = |a: &SpeechAttribution| -> i64 {
        turns
            .iter()
            .filter(|t| t.label == label)
            .map(|t| (t.end_ms.min(a.end_ms) - t.start_ms.max(a.start_ms)).max(0))
            .sum()
    };
    attributions
        .iter()
        .filter(|a| a.status == AttributionStatus::Confirmed)
        .map(|a| (overlap(a), a))
        .filter(|(o, _)| *o > 0)
        .max_by_key(|(o, _)| *o)
        .map(|(_, a)| a)
}

/// Person speaking at `ms`: the confirmed attribution containing the
/// instant (the shortest wins, being the most precise), else none.
pub fn person_at(ms: i64, attributions: &[SpeechAttribution]) -> Option<&SpeechAttribution> {
    attributions
        .iter()
        .filter(|a| a.status == AttributionStatus::Confirmed && a.start_ms <= ms && ms < a.end_ms)
        .min_by_key(|a| a.end_ms - a.start_ms)
}

/// Shortest turn worth a voice print (V5: 3 s of speech per window).
pub const VOICE_TURN_MIN_MS: i64 = 3_000;
/// Longest stretch of one turn decoded for a voice print.
pub const VOICE_TURN_MAX_MS: i64 = 30_000;
/// Turns of one speaker decoded for its voice print.
pub const VOICE_TURNS_PER_SPEAKER: usize = 4;

/// Turns of `label` to decode for its voice print (D275): the longest
/// first, at least [`VOICE_TURN_MIN_MS`], each cut to
/// [`VOICE_TURN_MAX_MS`], at most [`VOICE_TURNS_PER_SPEAKER`].
pub fn voice_turns(segments: &[ReviewSegment], label: &str) -> Vec<MediaSpan> {
    let mut turns: Vec<MediaSpan> = segments
        .iter()
        .flat_map(|s| s.turns.iter())
        .filter(|t| t.label == label && t.end_ms - t.start_ms >= VOICE_TURN_MIN_MS)
        .map(|t| MediaSpan {
            start_ms: t.start_ms,
            end_ms: t.end_ms.min(t.start_ms + VOICE_TURN_MAX_MS),
        })
        .collect();
    turns.sort_by_key(|t| std::cmp::Reverse(t.end_ms - t.start_ms));
    turns.truncate(VOICE_TURNS_PER_SPEAKER);
    turns
}

/// Every turn of `label`, merged, in media order: the spans a voice
/// proposal covers.
pub fn label_spans(segments: &[ReviewSegment], label: &str) -> Vec<MediaSpan> {
    merge_spans(
        segments
            .iter()
            .flat_map(|s| s.turns.iter())
            .filter(|t| t.label == label)
            .map(|t| MediaSpan {
                start_ms: t.start_ms,
                end_ms: t.end_ms,
            })
            .collect(),
    )
}

/// Objects detected on a video of the library (D215), for the review
/// screen: the boxes of the latest detection, `ts_ms` = position in the
/// media (ms), in media order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaDetections {
    /// Name of the detection model; `None` before any detection.
    #[serde(default)]
    pub model: Option<String>,
    pub frames: Vec<crate::camera::FrameAnnotation>,
    /// The detection store (O2) is not reachable.
    #[serde(default)]
    pub unavailable: bool,
}

/// Spans of the media where something is detected: consecutive pictures
/// with boxes (at most `gap_ms` apart) form one span.
pub fn detection_spans(frames: &[crate::camera::FrameAnnotation], gap_ms: i64) -> Vec<MediaSpan> {
    let mut out: Vec<MediaSpan> = Vec::new();
    for f in frames.iter().filter(|f| !f.detections.is_empty()) {
        match out.last_mut() {
            Some(last) if f.ts_ms - last.end_ms <= gap_ms => last.end_ms = f.ts_ms + 1,
            _ => out.push(MediaSpan {
                start_ms: f.ts_ms,
                end_ms: f.ts_ms + 1,
            }),
        }
    }
    out
}

/// Longest window of a stream review (D271 V2).
pub const STREAM_REVIEW_MAX_SECS: i64 = 6 * 3600;

/// A segment of a stream whose audio is still kept, on the window clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AudioPiece {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    pub segment_id: uuid::Uuid,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A window of a live stream for the review screen (D271 V2): its text
/// and speakers, and the pieces of audio still kept (retention), every
/// time in ms from `from`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StreamReview {
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub from: String,
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub to: String,
    pub segments: Vec<ReviewSegment>,
    pub audio: Vec<AudioPiece>,
    /// The text store (O2) is not reachable.
    #[serde(default)]
    pub unavailable: bool,
}

/// The piece of audio playing at `ms`, if any.
pub fn piece_at(pieces: &[AudioPiece], ms: i64) -> Option<usize> {
    pieces
        .iter()
        .position(|p| p.start_ms <= ms && ms < p.end_ms)
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
        let v2 = Some(uuid::Uuid::from_fields(2, 0, 0, &[0; 8]));
        assert_eq!(default_run(&runs, v2), Some(current.id));
        // No run on the current version: the newest of any origin.
        assert_eq!(
            default_run(&runs, Some(uuid::Uuid::from_fields(9, 0, 0, &[0; 8]))),
            Some(platform.id)
        );
        assert_eq!(default_run(&[], v2), None);
    }

    fn attribution(start_ms: i64, end_ms: i64, person: &str) -> SpeechAttribution {
        SpeechAttribution {
            id: uuid::Uuid::nil(),
            object_id: uuid::Uuid::nil(),
            person: person.into(),
            start_ms,
            end_ms,
            origin: AttributionOrigin::Manual,
            confidence: None,
            status: AttributionStatus::Confirmed,
            version: 1,
            updated_at: String::new(),
        }
    }

    #[test]
    fn spans_merge_and_persons_resolve() {
        let span = |start_ms, end_ms| MediaSpan { start_ms, end_ms };
        assert_eq!(
            merge_spans(vec![
                span(5, 9),
                span(0, 3),
                span(3, 4),
                span(8, 12),
                span(7, 7)
            ]),
            [span(0, 4), span(5, 12)]
        );
        let all = [
            attribution(0, 10_000, "Alice"),
            attribution(4_000, 6_000, "Bob"),
        ];
        assert_eq!(
            person_at(5_000, &all).unwrap().person,
            "Bob",
            "the shortest wins"
        );
        assert_eq!(person_at(1_000, &all).unwrap().person, "Alice");
        assert!(person_at(10_000, &all).is_none(), "end excluded");
        let turns = [
            ReviewTurn {
                label: "S1".into(),
                start_ms: 0,
                end_ms: 3_000,
            },
            ReviewTurn {
                label: "S2".into(),
                start_ms: 3_000,
                end_ms: 6_000,
            },
        ];
        // S2 overlaps Alice 3 s, Bob 2 s.
        assert_eq!(person_of_label(&turns, "S2", &all).unwrap().person, "Alice");
        let mut rejected = attribution(0, 3_000, "Carol");
        rejected.status = AttributionStatus::Rejected;
        assert!(person_of_label(&turns, "S1", &[rejected]).is_none());
    }

    #[test]
    fn voice_turns_take_the_longest_of_a_speaker() {
        let turn = |label: &str, start_ms, end_ms| ReviewTurn {
            label: label.into(),
            start_ms,
            end_ms,
        };
        let seg = ReviewSegment {
            start_ms: 0,
            end_ms: 200_000,
            text: String::new(),
            words: Vec::new(),
            turns: vec![
                turn("S1", 0, 2_000),
                turn("S1", 10_000, 80_000),
                turn("S2", 80_000, 90_000),
                turn("S1", 90_000, 95_000),
                turn("S1", 95_000, 100_000),
            ],
        };
        let v = voice_turns(std::slice::from_ref(&seg), "S1");
        // 2 s is too short; 70 s is cut to 30 s and comes first.
        assert_eq!(v.len(), 3);
        assert_eq!((v[0].start_ms, v[0].end_ms), (10_000, 40_000));
        // Touching turns merge into the proposal spans.
        let spans = label_spans(&[seg], "S1");
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert_eq!((spans[2].start_ms, spans[2].end_ms), (90_000, 100_000));
    }

    #[test]
    fn detections_gather_into_spans() {
        let frame = |ts_ms: i64, n: usize| crate::camera::FrameAnnotation {
            layer_id: "a1".into(),
            ts_ms,
            width: 640,
            height: 360,
            detections: (0..n)
                .map(|_| crate::vision::Detection {
                    label: "person".into(),
                    class_id: 0,
                    score: 0.9,
                    bbox: [0.0, 0.0, 10.0, 10.0],
                })
                .collect(),
        };
        let frames = [
            frame(0, 1),
            frame(2_000, 2),
            frame(4_000, 0),
            frame(8_000, 1),
        ];
        let spans = detection_spans(&frames, 2_000);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start_ms, spans[0].end_ms), (0, 2_001));
        assert_eq!(spans[1].start_ms, 8_000);
    }

    #[test]
    fn the_piece_under_the_cursor() {
        let p = |start_ms, end_ms| AudioPiece {
            segment_id: uuid::Uuid::nil(),
            start_ms,
            end_ms,
        };
        let pieces = [p(0, 30_000), p(29_000, 60_000)];
        assert_eq!(piece_at(&pieces, 10_000), Some(0));
        assert_eq!(piece_at(&pieces, 30_000), Some(1));
        assert_eq!(piece_at(&pieces, 90_000), None);
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
