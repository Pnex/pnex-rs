//! Page `/media/:id/review` — review of an audio or video file
//! (media-review.md D271): the media in the centre (nothing for an audio
//! but the shared transport), the transcript on the right, the speakers on
//! the shared timeline below, one cursor across the three. A click on a
//! word, on the timeline or on the player moves all of them.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::{annotation_at, FrameAnnotation};
use pnex_core::media_review::{
    detection_spans, person_at, AttributionInput, AttributionPatch, AttributionStatus, MediaRun,
    MediaSpan, MediaTranscript, ReviewSegment, RunOrigin, SpeechAttribution,
};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::timeline::axis::Scale;
use crate::components::timeline::{Body, Item, ItemKind, Lane, TimeBase, Timeline};
use crate::components::transport::{seek_media, MediaPlayer};
use crate::components::ui::{BTN_SECONDARY_SM, INPUT, PANEL};

pub(crate) const PLAYER: &str = "media-review-player";
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

/// Person speaking in the middle of a block (confirmed attributions only).
fn block_person(b: &Block, attributions: &[SpeechAttribution]) -> Option<(uuid::Uuid, String)> {
    let end = b.words.last().map_or(b.start_ms, |w| w.2);
    person_at((b.start_ms + end) / 2, attributions).map(|a| (a.object_id, a.person.clone()))
}

/// Text of a block, words joined.
fn block_text(b: &Block) -> String {
    b.words
        .iter()
        .map(|w| w.0.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// At most `max` characters, cut on a char boundary.
fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Speaker labels in order of first appearance.
pub(crate) fn speakers(segments: &[ReviewSegment]) -> Vec<String> {
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

/// Persons with a span of the media (rejected proposals left out), in
/// order of first appearance: `(object id, name)`.
fn persons(t: &MediaTranscript) -> Vec<(uuid::Uuid, String)> {
    let mut out: Vec<(uuid::Uuid, String)> = Vec::new();
    for a in t
        .attributions
        .iter()
        .filter(|a| a.status != AttributionStatus::Rejected)
    {
        if !out.iter().any(|(id, _)| *id == a.object_id) {
            out.push((a.object_id, a.person.clone()));
        }
    }
    out
}

/// Lanes of the timeline: one per person (their spans, proposals drawn
/// apart), then one per speaker, or the text spans of a run without
/// speakers. A speaker lane is a selection tool: drag a span, or click its
/// title to take every turn of the voice.
pub(crate) fn lanes(
    t: &MediaTranscript,
    all: &[String],
    picked: Option<uuid::Uuid>,
    can_write: bool,
) -> Vec<Lane> {
    let mut out: Vec<Lane> = persons(t)
        .into_iter()
        .enumerate()
        .map(|(i, (id, name))| {
            let items: Vec<Item> = t
                .attributions
                .iter()
                .filter(|a| a.object_id == id && a.status != AttributionStatus::Rejected)
                .map(|a| Item {
                    id: a.id.to_string(),
                    from: a.start_ms,
                    to: a.end_ms,
                    kind: if a.status == AttributionStatus::Proposed {
                        ItemKind::Proposal
                    } else {
                        ItemKind::Mark
                    },
                    selected: picked == Some(a.id),
                    ..Default::default()
                })
                .collect();
            let mut lane = Lane::new(
                format!("p:{id}"),
                name,
                Body::Items {
                    items,
                    max_rows: 1,
                    stack: false,
                    empty: None,
                },
            );
            lane.color = Some(PERSON_COLORS[i % PERSON_COLORS.len()].to_string());
            lane.item_testid = "media-review-attribution";
            lane
        })
        .collect();
    if all.is_empty() {
        let spans: Vec<(i64, i64)> = t.segments.iter().map(|s| (s.start_ms, s.end_ms)).collect();
        if !spans.is_empty() {
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
            lane.editable = can_write;
            out.push(lane);
        }
        return out;
    }
    out.extend(all.iter().map(|label| {
        let spans: Vec<(i64, i64)> = turns_of(t, label);
        let mut lane = Lane::new(
            format!("s:{label}"),
            label.clone(),
            Body::Spans {
                spans,
                gaps: false,
                marks: Vec::new(),
            },
        );
        lane.color = Some(speaker_color(all, label).to_string());
        lane.tinted = true;
        lane.editable = can_write;
        lane.clickable = can_write;
        lane.title_tip = can_write.then(|| t!("media-review-pick-speaker").to_string());
        lane
    }));
    out
}

/// Turns of a speaker label, in media order.
fn turns_of(t: &MediaTranscript, label: &str) -> Vec<(i64, i64)> {
    t.segments
        .iter()
        .flat_map(|s| s.turns.iter())
        .filter(|turn| turn.label == label)
        .map(|turn| (turn.start_ms, turn.end_ms))
        .collect()
}

/// Colours of the persons (their lanes), apart from the speaker colours.
const PERSON_COLORS: [&str; 6] = [
    "#0f766e", "#b45309", "#7c3aed", "#be123c", "#1d4ed8", "#4d7c0f",
];

#[component]
pub fn MediaReview(id: String) -> Element {
    let navigator = use_navigator();
    let can_write = crate::state::org::current_can_write();
    let mut run = use_signal(|| None::<String>);
    let mut reload = use_signal(|| 0u32);
    let mut pos_ms = use_signal(|| 0i64);
    let mut dur_ms = use_signal(|| 0i64);
    let mut blob = use_signal(|| None::<String>);
    // Spans chosen on the timeline, waiting for a person (D273).
    let mut selection = use_signal(Vec::<MediaSpan>::new);
    let mut picked = use_signal(|| None::<uuid::Uuid>);
    let mut error = use_signal(|| None::<String>);
    let aid = id.clone();
    let asset = use_resource(move || {
        let id = aid.clone();
        async move { api::media::detail(&id).await }
    });
    let did = id.clone();
    let dets = use_resource(move || {
        let id = did.clone();
        async move { api::media::detections(&id).await.ok() }
    });
    let tid = id.clone();
    let transcript = use_resource(move || {
        let id = tid.clone();
        let r = run();
        let _ = reload();
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
    let attributions = data
        .as_ref()
        .map(|t| t.attributions.clone())
        .unwrap_or_default();
    let palette = data.as_ref().map(persons).unwrap_or_default();
    let picked_one = picked().and_then(|p| attributions.iter().find(|a| a.id == p).cloned());

    let attribute = {
        let id = id.clone();
        move |person: uuid::Uuid| {
            let spans = selection();
            if spans.is_empty() {
                return;
            }
            let id = id.clone();
            spawn(async move {
                let input = AttributionInput {
                    object_id: person,
                    spans,
                };
                match api::media::attribute(&id, &input).await {
                    Ok(_) => {
                        selection.set(Vec::new());
                        error.set(None);
                        reload += 1;
                    }
                    Err(e) => error.set(Some(e.message)),
                }
            });
        }
    };
    let on_key = {
        let attribute = attribute.clone();
        let palette = palette.clone();
        move |e: KeyboardEvent| {
            if let Key::Character(c) = e.key() {
                if let Some(n) = c.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                    if let Some((person, _)) = palette.get(n - 1) {
                        attribute.clone()(*person);
                    }
                }
            }
            if e.key() == Key::Escape {
                selection.set(Vec::new());
                picked.set(None);
            }
        }
    };
    let frames: Vec<FrameAnnotation> = dets
        .read()
        .as_ref()
        .and_then(|d| d.as_ref().map(|d| d.frames.clone()))
        .unwrap_or_default();
    let end = end.max(frames.last().map_or(0, |f| f.ts_ms + 2_000));
    let mut lanes_now = data
        .as_ref()
        .map(|t| lanes(t, &all, picked(), can_write))
        .unwrap_or_default();
    if !frames.is_empty() {
        let mut lane = Lane::new(
            "detections",
            t!("media-review-detections").to_string(),
            Body::Spans {
                spans: detection_spans(&frames, DETECTION_GAP_MS)
                    .into_iter()
                    .map(|s| (s.start_ms, s.end_ms))
                    .collect(),
                gaps: false,
                marks: Vec::new(),
            },
        );
        lane.color = Some("#f97316".into());
        lane.tinted = true;
        lanes_now.push(lane);
    }
    let shown = annotation_at(&frames, pos, DETECTION_GAP_MS / 2).cloned();
    let has_lanes = !lanes_now.is_empty();
    let speaker_turns = data.clone();

    rsx! {
        ListLayout {
            title: t!("media-review-title", name : name.clone()).to_string(),
            on_back: move |_| {
                navigator.push(Route::Media {});
            },
            can_write,
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
                            overlay: shown.map(|f| rsx! {
                                BoxOverlay { frame: f }
                            }),
                        }
                    }
                }
                TranscriptPanel {
                    asset_id: id.clone(),
                    can_write,
                    data: data.clone(),
                    failed,
                    speakers: all.clone(),
                    attributions: attributions.clone(),
                    pos,
                    on_seek: seek,
                }
            }
            if has_lanes {
                div {
                    class: "mt-4 {PANEL} outline-none",
                    tabindex: "0",
                    onkeydown: on_key,
                    Timeline {
                        canvas: CANVAS,
                        scale: Scale::linear((0, end)),
                        base: TimeBase::Media,
                        cursor: pos,
                        lanes: lanes_now,
                        header: t!("media-review-speakers").to_string(),
                        on_seek: seek,
                        on_create: move |(_, from, to): (String, i64, i64)| {
                            if to > from {
                                picked.set(None);
                                selection
                                    .set(
                                        vec![
                                            MediaSpan {
                                                start_ms: from,
                                                end_ms: to,
                                            },
                                        ],
                                    );
                            }
                        },
                        on_title: move |lane: String| {
                            let Some(label) = lane.strip_prefix("s:") else {
                                return;
                            };
                            let Some(t) = speaker_turns.as_ref() else {
                                return;
                            };
                            picked.set(None);
                            selection
                                .set(
                                    turns_of(t, label)
                                        .into_iter()
                                        .map(|(start_ms, end_ms)| MediaSpan { start_ms, end_ms })
                                        .collect(),
                                );
                        },
                        on_item: move |(_, item): (String, String)| {
                            selection.set(Vec::new());
                            picked.set(item.parse().ok());
                        },
                    }
                    if can_write && !selection().is_empty() {
                        SelectionBar {
                            spans: selection(),
                            palette: palette.clone(),
                            on_pick: attribute.clone(),
                            on_cancel: move |_| selection.set(Vec::new()),
                        }
                    }
                    if let Some(a) = picked_one {
                        PickedBar {
                            asset_id: id.clone(),
                            attribution: a,
                            can_write,
                            on_done: move |e: Option<String>| {
                                picked.set(None);
                                error.set(e);
                                reload += 1;
                            },
                        }
                    }
                    if let Some(e) = error() {
                        p { class: "border-t border-line px-3 py-2 text-sm text-red-700",
                            {crate::api::error_i18n::localize_code(&e, None, &e)}
                        }
                    }
                }
            }
        }
    }
}

/// The spans chosen on the timeline and who they are: a person of the
/// palette (keys 1–9), another person of the ontology, or a new one.
#[component]
fn SelectionBar(
    spans: Vec<MediaSpan>,
    palette: Vec<(uuid::Uuid, String)>,
    on_pick: EventHandler<uuid::Uuid>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut new_name = use_signal(String::new);
    let all = use_resource(move || async move {
        let q = pnex_core::ontology::api::OntologyQuery {
            type_key: Some("person".into()),
            limit: Some(500),
            ..Default::default()
        };
        api::ontology::query(&q)
            .await
            .map(|r| r.rows)
            .unwrap_or_default()
    });
    let rows = all.read().clone().unwrap_or_default();
    let first = spans.first().map_or(0, |s| s.start_ms);
    let last = spans.last().map_or(0, |s| s.end_ms);
    let span_text = format!(
        "{} → {}",
        TimeBase::Media.clock(first),
        TimeBase::Media.clock(last)
    );
    let create = move |_| {
        let title = new_name().trim().to_string();
        if title.is_empty() {
            return;
        }
        spawn(async move {
            let input = pnex_core::ontology::api::ObjectInput {
                type_key: "person".into(),
                title,
                ..Default::default()
            };
            if let Ok(o) = api::ontology::create_object(&input).await {
                if let Ok(id) = o.id.parse() {
                    new_name.set(String::new());
                    on_pick.call(id);
                }
            }
        });
    };
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-2 border-t border-line px-3 py-2 text-sm",
            "data-testid": "media-review-selection",
            span { class: "font-medium text-gray-800",
                {t!("media-review-selection", span : span_text, n : spans.len())}
            }
            for (i, (person, name)) in palette.into_iter().enumerate().take(9) {
                button {
                    key: "{person}",
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| on_pick.call(person),
                    span { class: "font-mono text-xs text-gray-500", "{i + 1}" }
                    "{name}"
                }
            }
            select {
                class: "{INPUT} max-w-xs",
                "data-testid": "media-review-person",
                onchange: move |e| {
                    if let Ok(id) = e.value().parse() {
                        on_pick.call(id);
                    }
                },
                option { value: "", {t!("media-review-other-person")} }
                for r in rows {
                    option { key: "{r.object.id}", value: "{r.object.id}", "{r.object.title}" }
                }
            }
            input {
                class: "{INPUT} max-w-xs",
                placeholder: t!("media-review-new-person").to_string(),
                value: "{new_name}",
                oninput: move |e| new_name.set(e.value()),
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: new_name().trim().is_empty(),
                onclick: create,
                {t!("media-review-create-person")}
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                onclick: move |_| on_cancel.call(()),
                {t!("media-review-cancel")}
            }
        }
    }
}

/// The attribution clicked on a person's lane: confirm or reject a
/// proposal, remove a span.
#[component]
fn PickedBar(
    asset_id: String,
    attribution: SpeechAttribution,
    can_write: bool,
    on_done: EventHandler<Option<String>>,
) -> Element {
    let a = attribution.clone();
    let span = format!(
        "{} → {}",
        TimeBase::Media.clock(a.start_ms),
        TimeBase::Media.clock(a.end_ms)
    );
    let set_status = {
        let asset_id = asset_id.clone();
        let a = a.clone();
        move |status: AttributionStatus| {
            let (asset_id, aid, version) = (asset_id.clone(), a.id.to_string(), a.version);
            spawn(async move {
                let patch = AttributionPatch {
                    expected_version: version,
                    status: Some(status),
                    start_ms: None,
                    end_ms: None,
                };
                let r = api::media::update_attribution(&asset_id, &aid, &patch).await;
                on_done.call(r.err().map(|e| e.message));
            });
        }
    };
    let (confirm, reject) = (set_status.clone(), set_status);
    let remove = {
        let (asset_id, aid) = (asset_id.clone(), a.id.to_string());
        move |_| {
            let (asset_id, aid) = (asset_id.clone(), aid.clone());
            spawn(async move {
                let r = api::media::delete_attribution(&asset_id, &aid).await;
                on_done.call(r.err().map(|e| e.message));
            });
        }
    };
    let origin = origin_label(a.origin);
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-2 border-t border-line px-3 py-2 text-sm",
            "data-testid": "media-review-picked",
            span { class: "font-medium text-gray-800", "{a.person}" }
            span { class: "font-mono text-xs text-gray-500", "{span}" }
            span { class: "text-xs text-gray-500", "{origin}" }
            if can_write && a.status == AttributionStatus::Proposed {
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| confirm.clone()(AttributionStatus::Confirmed),
                    {t!("media-review-confirm")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: move |_| reject.clone()(AttributionStatus::Rejected),
                    {t!("media-review-reject")}
                }
            }
            if can_write {
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    onclick: remove,
                    {t!("media-review-remove")}
                }
            }
            if can_write && a.status == AttributionStatus::Confirmed {
                EnrolButton {
                    asset_id: asset_id.clone(),
                    object_id: a.object_id.to_string(),
                }
            }
        }
    }
}

/// Pictures of a video are analysed every 2 s: closer ones are one span.
const DETECTION_GAP_MS: i64 = 2_000;

/// Boxes of one analysed picture over the video (same box, `meet` like the
/// video's own `object-fit: contain`).
#[component]
fn BoxOverlay(frame: FrameAnnotation) -> Element {
    let (w, h) = (frame.width.max(1), frame.height.max(1));
    rsx! {
        svg {
            class: "pointer-events-none absolute inset-0 h-full w-full",
            view_box: "0 0 {w} {h}",
            preserve_aspect_ratio: "xMidYMid meet",
            "data-testid": "media-review-boxes",
            for (i, d) in frame.detections.iter().enumerate() {
                g { key: "{i}",
                    rect {
                        x: "{d.bbox[0]}",
                        y: "{d.bbox[1]}",
                        width: "{d.bbox[2]}",
                        height: "{d.bbox[3]}",
                        fill: "none",
                        stroke: "#f97316",
                        stroke_width: "3",
                    }
                    text {
                        x: "{d.bbox[0]}",
                        y: "{(d.bbox[1] - 4.0).max(12.0)}",
                        fill: "#f97316",
                        font_size: "16",
                        "{d.label} {(d.score * 100.0).round()} %"
                    }
                }
            }
        }
    }
}

/// Enrols the confirmed passages of a person into their voice profile
/// (D277). No profile yet: consent is recorded on the person's page first.
#[component]
fn EnrolButton(asset_id: String, object_id: String) -> Element {
    let mut outcome = use_signal(|| None::<Result<String, String>>);
    let enrol = {
        let (asset_id, object_id) = (asset_id.clone(), object_id.clone());
        move |_| {
            let (asset_id, object_id) = (asset_id.clone(), object_id.clone());
            spawn(async move {
                let r = match api::voice_id::by_object(&object_id).await {
                    Ok(Some(p)) => api::voice_id::add_from_media(&p.id, &asset_id)
                        .await
                        .map(|p| t!("media-review-enrolled", n : p.samples.len()).to_string())
                        .map_err(|e| {
                            crate::api::error_i18n::localize_code(&e.message, None, &e.message)
                        }),
                    Ok(None) => Err(t!("media-review-enrol-consent").to_string()),
                    Err(e) => Err(e.message),
                };
                outcome.set(Some(r));
            });
        }
    };
    rsx! {
        button {
            class: BTN_SECONDARY_SM,
            r#type: "button",
            "data-testid": "media-review-enrol",
            onclick: enrol,
            {t!("media-review-enrol")}
        }
        match outcome() {
            Some(Ok(text)) | Some(Err(text)) => rsx! {
                span { class: "text-xs text-gray-600", "{text}" }
                Link {
                    class: "text-xs text-signal-600 hover:underline",
                    to: Route::OntologyObject {
                        id: object_id.clone(),
                    },
                    {t!("media-review-enrol-open")}
                }
            },
            None => rsx! {},
        }
    }
}

fn origin_label(o: pnex_core::media_review::AttributionOrigin) -> String {
    use pnex_core::media_review::AttributionOrigin as O;
    match o {
        O::Manual => t!("media-review-origin-manual"),
        O::Voice => t!("media-review-origin-voice"),
        O::Transcript => t!("media-review-origin-transcript"),
        O::Grid => t!("media-review-origin-grid"),
    }
    .to_string()
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
pub(crate) fn TranscriptPanel(
    asset_id: String,
    can_write: bool,
    data: Option<MediaTranscript>,
    failed: bool,
    speakers: Vec<String>,
    attributions: Vec<SpeechAttribution>,
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
            let texts: Vec<String> = list.iter().map(block_text).collect();
            rsx! {
                for (i, b) in list.into_iter().enumerate() {
                    BlockView {
                        key: "{i}",
                        person: block_person(&b, &attributions),
                        before: i.checked_sub(1).map(|k| texts[k].clone()).unwrap_or_default(),
                        after: texts.get(i + 1).cloned().unwrap_or_default(),
                        asset_id: asset_id.clone(),
                        can_write,
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
fn BlockView(
    block: Block,
    person: Option<(uuid::Uuid, String)>,
    /// Texts of the neighbour blocks: the context of a statement (K43).
    before: String,
    after: String,
    asset_id: String,
    can_write: bool,
    speakers: Vec<String>,
    pos: i64,
    on_seek: EventHandler<i64>,
) -> Element {
    let mut created = use_signal(|| None::<String>);
    let quote = block_text(&block);
    let make_statement = {
        let (person, start) = (person.clone(), block.start_ms);
        move |_| {
            let (quote, before, after, asset_id, person) = (
                quote.clone(),
                before.clone(),
                after.clone(),
                asset_id.clone(),
                person.clone(),
            );
            spawn(async move {
                let mut props = serde_json::Map::new();
                props.insert("quote".into(), clip(&quote, 2_000).into());
                props.insert("locator".into(), format!("media:{asset_id}@{start}").into());
                props.insert("context_before".into(), clip(&before, 1_000).into());
                props.insert("context_after".into(), clip(&after, 1_000).into());
                let input = pnex_core::ontology::api::ObjectInput {
                    type_key: "statement".into(),
                    title: clip(&quote, 120),
                    properties: props,
                    ..Default::default()
                };
                let Ok(obj) = api::ontology::create_object(&input).await else {
                    return;
                };
                if let Some((pid, _)) = person {
                    let mut attributes = serde_json::Map::new();
                    attributes.insert("origin".into(), "manual".into());
                    let link = pnex_core::ontology::api::LinkInput {
                        link_type: "said_by".into(),
                        source_id: obj.id.clone(),
                        target_id: pid.to_string(),
                        attributes,
                        valid_from: None,
                        valid_to: None,
                    };
                    let _ = api::ontology::create_link(&link).await;
                }
                created.set(Some(obj.id));
            });
        }
    };
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
                if let Some((_, name)) = person.clone() {
                    span {
                        class: "font-medium text-gray-900",
                        "data-testid": "media-review-person-name",
                        "{name}"
                    }
                }
                button {
                    class: "font-mono text-gray-500 hover:text-signal-600",
                    r#type: "button",
                    onclick: move |_| on_seek.call(start),
                    "{clock}"
                }
                if can_write && created().is_none() {
                    button {
                        class: "ml-auto text-gray-500 hover:text-signal-600",
                        r#type: "button",
                        "data-testid": "media-review-statement",
                        onclick: make_statement,
                        {t!("media-review-statement")}
                    }
                }
                if let Some(oid) = created() {
                    Link {
                        class: "ml-auto text-signal-600 hover:underline",
                        to: Route::OntologyObject { id: oid },
                        {t!("media-review-statement-open")}
                    }
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
