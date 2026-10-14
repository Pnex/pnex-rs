//! Video recordings of an IP stream (media-ingest.md D175): segments
//! written by a `video_record` node fed by a `camera_source` reading the
//! stream, newest first, each downloadable as MJPEG-AVI. The page's
//! segments replay back to back in the shared recording player.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::{playback_chain, VideoSegment};
use pnex_core::media_ingest::MediaStream;

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::modal::Modal;
use crate::pages::cameras::player::{span_ms, RecordingPlayer};
use crate::state::toasts;

fn duration_secs(s: &VideoSegment) -> i64 {
    let at = |v: &str| chrono::DateTime::parse_from_rfc3339(v).ok();
    match (at(&s.started_at), at(&s.ended_at)) {
        (Some(a), Some(b)) => (b - a).num_seconds(),
        _ => 0,
    }
}

#[component]
pub fn RecordingsDialog(stream: MediaStream, on_close: Callback<()>) -> Element {
    let page = use_signal(|| 0i64);
    let seek = use_signal(|| None::<i64>);
    let position = use_signal(|| None::<i64>);
    let layers = use_signal(Vec::<String>::new);
    let id = stream.id.clone();
    let results = use_resource(move || {
        let id = id.clone();
        let offset = page() * PAGE_SIZE;
        async move { api::cameras::stream_segments(&id, PAGE_SIZE, offset).await }
    });
    let (list_state, is_empty, count, rows) = match &*results.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(p)) => (
            Some(Ok(())),
            p.results.is_empty(),
            p.count,
            p.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };
    // The page is newest first; playback wants ascending, without overlaps.
    let mut ascending = rows.clone();
    ascending.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    let spans: Vec<(i64, i64)> = ascending
        .iter()
        .map(|s| span_ms(s).unwrap_or((0, 0)))
        .collect();
    let chain: Vec<VideoSegment> = playback_chain(&spans)
        .into_iter()
        .map(|i| ascending[i].clone())
        .collect();
    let blocks: Vec<(i64, i64)> = chain.iter().filter_map(span_ms).collect();
    let window = (
        blocks.first().map_or(0, |b| b.0),
        blocks.last().map_or(1, |b| b.1),
    );
    let player_key = format!("stream-player-{}", page());
    rsx! {
        Modal {
            title: t!("streams-recordings-title", name : stream.name.clone()).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close,
            div { class: "space-y-3",
                p { class: "text-xs text-gray-500", {t!("streams-recordings-help")} }
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("streams-recordings-empty").to_string(),
                    div { class: "space-y-3",
                        if !chain.is_empty() {
                            // Own block: a key is honoured on a block's first node only.
                            {
                                rsx! {
                                    RecordingPlayer {
                                        key: "{player_key}",
                                        device: 0,
                                        segments: chain.clone(),
                                        layers,
                                        seek,
                                        position,
                                        window,
                                        blocks: blocks.clone(),
                                    }
                                }
                            }
                        }
                        div { class: "overflow-x-auto border border-gray-200 rounded-lg",
                            table { class: "min-w-full divide-y divide-gray-200 text-sm",
                                thead { class: "bg-gray-50",
                                    tr {
                                        th { class: "th", {t!("streams-seg-col-start")} }
                                        th { class: "th hidden sm:table-cell",
                                            {t!("streams-seg-col-duration")}
                                        }
                                        th { class: "th hidden sm:table-cell",
                                            {t!("streams-recordings-col-frames")}
                                        }
                                        th { class: "th" }
                                    }
                                }
                                tbody { class: "divide-y divide-gray-100",
                                    for s in rows {
                                        RecordingRow {
                                            key: "{s.id}",
                                            segment: s.clone(),
                                            seek,
                                        }
                                    }
                                }
                            }
                        }
                        ListPager { count, page }
                    }
                }
            }
        }
    }
}

#[component]
fn RecordingRow(segment: VideoSegment, seek: Signal<Option<i64>>) -> Element {
    let start = span_ms(&segment).map(|s| s.0);
    let secs = duration_secs(&segment);
    let mut busy = use_signal(|| false);
    let id = segment.id.clone();
    let name = format!(
        "{}_{}.avi",
        segment.stream,
        segment.started_at.replace(':', "")
    );
    let download = move |_| {
        let id = id.clone();
        let name = name.clone();
        let failed = t!("streams-recordings-download-failed").to_string();
        busy.set(true);
        spawn(async move {
            match api::cameras::segment_bytes(&id).await {
                Ok(bytes) => {
                    if !crate::util::trigger_download(&name, "video/x-msvideo", &bytes) {
                        toasts::error(failed);
                    }
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    rsx! {
        tr {
            td { class: "td whitespace-nowrap",
                {date_label(&segment.started_at)}
                span { class: "ml-1 text-xs text-gray-500", "{segment.stream}" }
            }
            td { class: "td hidden text-gray-600 sm:table-cell", "{secs} s" }
            td { class: "td hidden text-gray-600 sm:table-cell", "{segment.frame_count}" }
            td { class: "td text-right space-x-1 whitespace-nowrap",
                if let Some(at) = start {
                    button {
                        class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50",
                        r#type: "button",
                        onclick: move |_| {
                            let mut seek = seek;
                            seek.set(Some(at));
                        },
                        {t!("streams-recordings-play")}
                    }
                }
                button {
                    class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                    r#type: "button",
                    disabled: busy(),
                    onclick: download,
                    {t!("streams-recordings-download")}
                }
            }
        }
    }
}
