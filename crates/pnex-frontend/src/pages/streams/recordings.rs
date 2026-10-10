//! Video recordings of an IP stream (media-ingest.md D175): segments
//! written by a `video_record` node fed by a `camera_source` reading the
//! stream, newest first, each downloadable as MJPEG-AVI.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::camera::VideoSegment;
use pnex_core::media_ingest::MediaStream;

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::modal::Modal;
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
                                        RecordingRow { key: "{s.id}", segment: s.clone() }
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
fn RecordingRow(segment: VideoSegment) -> Element {
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
                span { class: "ml-1 text-xs text-gray-400", "{segment.stream}" }
            }
            td { class: "td hidden text-gray-600 sm:table-cell", "{secs} s" }
            td { class: "td hidden text-gray-600 sm:table-cell", "{segment.frame_count}" }
            td { class: "td text-right",
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
