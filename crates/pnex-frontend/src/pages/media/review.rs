//! Page `/media/:id/review` — review of an audio or video file
//! (media-review.md D271): the media in the centre (nothing for an audio
//! but the shared transport), the transcript on the right, the speakers on
//! the shared timeline below, one cursor across the three. A click on a
//! word, on the timeline or on the player moves all of them.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_review::{MediaRun, MediaTranscript, ReviewSegment, RunOrigin};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::timeline::axis::Scale;
use crate::components::timeline::{Body, Lane, TimeBase, Timeline};
use crate::components::transport::{seek_media, MediaPlayer};
use crate::components::ui::{INPUT, PANEL};

const PLAYER: &str = "media-review-player";
const CANVAS: &str = "media-review-frise";

/// Colours of the speakers, in order of first appearance.
const SPEAKER_COLORS: [&str; 8] = [
    "#6366f1", "#f59e0b", "#10b981", "#ef4444", "#0ea5e9", "#a855f7", "#84cc16", "#ec4899",
];

/// A run of consecutive words of one speaker (or one cue without words).
#[derive(Debug, Clone, PartialEq)]
struct Block {
    speaker: Option<String>,
    start_ms: i64,
    words: Vec<(String, i64, i64)>,
}

/// Transcript blocks in media order: a new block when the speaker changes;
/// a segment without word timings (platform cue) is one block.
fn blocks(segments: &[ReviewSegment]) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    for seg in segments {
        if seg.words.is_empty() {
            out.push(Block {
                speaker: None,
                start_ms: seg.start_ms,
                words: vec![(seg.text.clone(), seg.start_ms, seg.end_ms)],
            });
            continue;
        }
        for w in &seg.words {
            let word = (w.text.clone(), w.start_ms, w.end_ms);
            // A run never mixes cues and words: same speaker = same block.
            match out.last_mut() {
                Some(b) if b.speaker == w.speaker => b.words.push(word),
                _ => out.push(Block {
                    speaker: w.speaker.clone(),
                    start_ms: w.start_ms,
                    words: vec![word],
                }),
            }
        }
    }
    out
}

/// Speaker labels in order of first appearance.
fn speakers(segments: &[ReviewSegment]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in segments.iter().flat_map(|s| s.turns.iter()) {
        if !out.contains(&t.label) {
            out.push(t.label.clone());
        }
    }
    out
}

fn speaker_color(all: &[String], label: &str) -> &'static str {
    let i = all.iter().position(|l| l == label).unwrap_or(0);
    SPEAKER_COLORS[i % SPEAKER_COLORS.len()]
}

fn run_label(r: &MediaRun) -> String {
    let date = crate::components::badges::date_label(&r.created_at);
    match r.origin {
        RunOrigin::Pnex => t!("media-review-run-pnex", date : date).to_string(),
        RunOrigin::Platform => t!("media-review-run-platform", date : date).to_string(),
    }
}

/// Lanes of the timeline: one per speaker, or the text spans of a run
/// without speakers.
fn lanes(t: &MediaTranscript, all: &[String]) -> Vec<Lane> {
    if all.is_empty() {
        let spans: Vec<(i64, i64)> = t.segments.iter().map(|s| (s.start_ms, s.end_ms)).collect();
        if spans.is_empty() {
            return Vec::new();
        }
        let mut lane = Lane::new(
            "text",
            t!("media-review-text-lane").to_string(),
            Body::Spans {
                spans,
                gaps: false,
                marks: Vec::new(),
            },
        );
        lane.color = Some(SPEAKER_COLORS[0].to_string());
        return vec![lane];
    }
    all.iter()
        .map(|label| {
            let spans: Vec<(i64, i64)> = t
                .segments
                .iter()
                .flat_map(|s| s.turns.iter())
                .filter(|turn| &turn.label == label)
                .map(|turn| (turn.start_ms, turn.end_ms))
                .collect();
            let mut lane = Lane::new(
                label.clone(),
                label.clone(),
                Body::Spans {
                    spans,
                    gaps: false,
                    marks: Vec::new(),
                },
            );
            lane.color = Some(speaker_color(all, label).to_string());
            lane
        })
        .collect()
}

#[component]
pub fn MediaReview(id: String) -> Element {
    let navigator = use_navigator();
    let mut run = use_signal(|| None::<String>);
    let mut pos_ms = use_signal(|| 0i64);
    let mut dur_ms = use_signal(|| 0i64);
    let mut blob = use_signal(|| None::<String>);
    let aid = id.clone();
    let asset = use_resource(move || {
        let id = aid.clone();
        async move { api::media::detail(&id).await }
    });
    let tid = id.clone();
    let transcript = use_resource(move || {
        let id = tid.clone();
        let r = run();
        async move { api::media::transcript(&id, r.as_deref()).await }
    });
    let bid = id.clone();
    use_effect(move || {
        let Some(Ok(a)) = &*asset.read() else {
            return;
        };
        if a.current_version_number.is_none() {
            return;
        }
        let path = api::media::content_path(&bid);
        let mime = a.content_type.clone();
        spawn(async move {
            blob.set(crate::util::media_blob_url(&path, mime.as_deref()).await);
        });
    });

    let (name, video, has_file) = match &*asset.read() {
        Some(Ok(a)) => (
            a.name.clone(),
            a.kind == "video",
            a.current_version_number.is_some(),
        ),
        _ => (String::new(), false, false),
    };
    let data = match &*transcript.read() {
        Some(Ok(t)) => Some(t.clone()),
        _ => None,
    };
    let failed = matches!(&*transcript.read(), Some(Err(_)));
    let all = data
        .as_ref()
        .map(|t| speakers(&t.segments))
        .unwrap_or_default();
    let end = data
        .as_ref()
        .and_then(|t| t.segments.iter().map(|s| s.end_ms).max())
        .unwrap_or(0)
        .max(dur_ms())
        .max(1_000);
    let pos = pos_ms();
    let seek = move |ms: i64| {
        pos_ms.set(ms);
        seek_media(PLAYER, ms as f64 / 1000.0);
    };

    rsx! {
        ListLayout {
            title: t!("media-review-title", name : name.clone()).to_string(),
            on_back: move |_| {
                navigator.push(Route::Media {});
            },
            can_write: false,
            if let Some(t) = data.as_ref().filter(|t| t.runs.len() > 1) {
                RunPicker {
                    runs: t.runs.clone(),
                    current: t.run.map(|r| r.to_string()),
                    on_pick: move |r| run.set(Some(r)),
                }
            }
            div { class: "grid gap-4 lg:grid-cols-5",
                div { class: "lg:col-span-3 space-y-3",
                    if !has_file {
                        p { class: "rounded-md bg-gray-50 px-3 py-2 text-sm text-gray-600",
                            {t!("media-review-no-file")}
                        }
                    } else if let Some(url) = blob() {
                        MediaPlayer {
                            id: PLAYER,
                            src: url,
                            video,
                            on_time: move |(p, d): (f64, f64)| {
                                pos_ms.set((p * 1000.0) as i64);
                                dur_ms.set((d * 1000.0) as i64);
                            },
                        }
                    }
                }
                TranscriptPanel {
                    data: data.clone(),
                    failed,
                    speakers: all.clone(),
                    pos,
                    on_seek: seek,
                }
            }
            if let Some(t) = data.as_ref().filter(|t| !t.segments.is_empty()) {
                div { class: "mt-4 {PANEL}",
                    Timeline {
                        canvas: CANVAS,
                        scale: Scale::linear((0, end)),
                        base: TimeBase::Media,
                        cursor: pos,
                        lanes: lanes(t, &all),
                        header: t!("media-review-speakers").to_string(),
                        on_seek: seek,
                    }
                }
            }
        }
    }
}

#[component]
fn RunPicker(
    runs: Vec<MediaRun>,
    current: Option<String>,
    on_pick: EventHandler<String>,
) -> Element {
    let current = current.unwrap_or_default();
    rsx! {
        label { class: "mb-3 flex items-center gap-2 text-sm text-gray-700",
            {t!("media-review-run")}
            select {
                class: "{INPUT} max-w-sm",
                "data-testid": "media-review-run",
                onchange: move |e| on_pick.call(e.value()),
                for r in runs.iter() {
                    option {
                        value: "{r.id}",
                        selected: r.id.to_string() == current,
                        {run_label(r)}
                    }
                }
            }
        }
    }
}

#[component]
fn TranscriptPanel(
    data: Option<MediaTranscript>,
    failed: bool,
    speakers: Vec<String>,
    pos: i64,
    on_seek: EventHandler<i64>,
) -> Element {
    let body = match &data {
        None if failed => rsx! {
            p { class: "text-sm text-red-700", {t!("media-review-failed")} }
        },
        None => rsx! {
            p { class: "text-sm text-gray-500", {t!("media-review-loading")} }
        },
        Some(t) if t.unavailable => rsx! {
            p { class: "text-sm text-gray-600", {t!("media-review-unavailable")} }
        },
        Some(t) if t.segments.is_empty() => rsx! {
            p { class: "text-sm text-gray-600", {t!("media-review-empty")} }
        },
        Some(t) => {
            let list = blocks(&t.segments);
            rsx! {
                for (i, b) in list.into_iter().enumerate() {
                    BlockView {
                        key: "{i}",
                        block: b,
                        speakers: speakers.clone(),
                        pos,
                        on_seek,
                    }
                }
            }
        }
    };
    rsx! {
        section {
            class: "lg:col-span-2 {PANEL} max-h-[520px] overflow-y-auto p-3 space-y-3",
            "data-testid": "media-review-transcript",
            h3 { class: "text-sm font-semibold text-gray-700", {t!("media-review-transcript")} }
            {body}
        }
    }
}

#[component]
fn BlockView(block: Block, speakers: Vec<String>, pos: i64, on_seek: EventHandler<i64>) -> Element {
    let color = block
        .speaker
        .as_deref()
        .map(|l| speaker_color(&speakers, l))
        .unwrap_or("#64748b");
    let start = block.start_ms;
    let clock = TimeBase::Media.clock(start);
    rsx! {
        div { class: "space-y-1",
            div { class: "flex items-center gap-2 text-xs",
                if let Some(label) = block.speaker.clone() {
                    span {
                        class: "rounded px-1.5 py-0.5 font-medium text-white",
                        style: "background: {color}",
                        "{label}"
                    }
                }
                button {
                    class: "font-mono text-gray-500 hover:text-signal-600",
                    r#type: "button",
                    onclick: move |_| on_seek.call(start),
                    "{clock}"
                }
            }
            p { class: "text-sm leading-relaxed text-gray-900",
                for (j, (word, from, to)) in block.words.into_iter().enumerate() {
                    span {
                        key: "{j}",
                        class: if pos >= from && pos < to { "cursor-pointer rounded bg-signal-100 text-signal-900" } else { "cursor-pointer hover:bg-gray-100" },
                        onclick: move |_| on_seek.call(from),
                        "{word} "
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::media_review::{ReviewTurn, ReviewWord};

    fn word(text: &str, s: i64, spk: Option<&str>) -> ReviewWord {
        ReviewWord {
            text: text.into(),
            start_ms: s,
            end_ms: s + 400,
            speaker: spk.map(str::to_string),
        }
    }

    #[test]
    fn blocks_follow_the_speaker_and_cues_stand_alone() {
        let seg = ReviewSegment {
            start_ms: 0,
            end_ms: 3_000,
            text: "a b c d".into(),
            words: vec![
                word("a", 0, Some("S1")),
                word("b", 500, Some("S1")),
                word("c", 1_000, Some("S2")),
                word("d", 1_500, Some("S1")),
            ],
            turns: vec![
                ReviewTurn {
                    label: "S1".into(),
                    start_ms: 0,
                    end_ms: 900,
                },
                ReviewTurn {
                    label: "S2".into(),
                    start_ms: 1_000,
                    end_ms: 1_400,
                },
            ],
        };
        let cue = ReviewSegment {
            start_ms: 4_000,
            end_ms: 6_000,
            text: "sous-titre".into(),
            words: Vec::new(),
            turns: Vec::new(),
        };
        let b = blocks(&[seg.clone(), cue]);
        let who: Vec<Option<&str>> = b.iter().map(|b| b.speaker.as_deref()).collect();
        assert_eq!(who, [Some("S1"), Some("S2"), Some("S1"), None]);
        assert_eq!(b[0].words.len(), 2);
        assert_eq!(b[3].words[0], ("sous-titre".to_string(), 4_000, 6_000));
        assert_eq!(speakers(&[seg]), ["S1", "S2"]);
    }
}
