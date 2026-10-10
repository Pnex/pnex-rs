//! Segments of a stream (media-ingest.md D162, §9): every captured slice
//! with its state, newest first. Gaps are data: failed and skipped
//! segments stay listed. Writers can re-queue failed segments whose audio
//! is still kept (D161).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::{MediaSegment, MediaStream, SegmentState};

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::modal::Modal;
use crate::state::toasts;

const INPUT: &str = "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white";

pub(super) fn state_label(state: SegmentState) -> String {
    match state {
        SegmentState::Captured => t!("streams-seg-captured").to_string(),
        SegmentState::Queued => t!("streams-seg-queued").to_string(),
        SegmentState::Transcribing => t!("streams-seg-transcribing").to_string(),
        SegmentState::Transcribed => t!("streams-seg-transcribed").to_string(),
        SegmentState::Failed => t!("streams-seg-failed").to_string(),
        SegmentState::SkippedSilence => t!("streams-seg-skipped-silence").to_string(),
        SegmentState::SkippedBacklog => t!("streams-seg-skipped-backlog").to_string(),
    }
}

fn state_class(state: SegmentState) -> &'static str {
    match state {
        SegmentState::Transcribed => "bg-emerald-100 text-emerald-800",
        SegmentState::Failed => "bg-red-100 text-red-800",
        SegmentState::SkippedBacklog => "bg-amber-100 text-amber-800",
        SegmentState::Queued | SegmentState::Transcribing => "bg-blue-100 text-blue-800",
        SegmentState::Captured | SegmentState::SkippedSilence => "bg-gray-100 text-gray-700",
    }
}

/// Transcription failure code (`media_segments.error`) → sentence;
/// unknown codes stay verbatim.
fn error_label(code: &str) -> String {
    match code {
        "no-profile" => t!("streams-seg-error-no-profile").to_string(),
        "model-invalid" => t!("streams-seg-error-model-invalid").to_string(),
        "audio-missing" => t!("streams-seg-error-audio-missing").to_string(),
        "asr-failed" => t!("streams-seg-error-asr-failed").to_string(),
        "o2-failed" => t!("streams-seg-error-o2-failed").to_string(),
        other => other.to_string(),
    }
}

fn duration_secs(s: &MediaSegment) -> i64 {
    let at = |v: &str| chrono::DateTime::parse_from_rfc3339(v).ok();
    match (at(&s.started_at), at(&s.ended_at)) {
        (Some(a), Some(b)) => (b - a).num_seconds(),
        _ => 0,
    }
}

#[component]
pub fn SegmentsDialog(stream: MediaStream, can_write: bool, on_close: Callback<()>) -> Element {
    let page = use_signal(|| 0i64);
    let mut state = use_signal(String::new);
    let mut reload = use_signal(|| 0u32);
    let mut busy = use_signal(|| false);
    let id = stream.id.clone();
    let results = use_resource(move || {
        let id = id.clone();
        let offset = page() * PAGE_SIZE;
        let state = state();
        async move {
            let _ = reload();
            api::media_streams::segments(&id, &state, PAGE_SIZE, offset).await
        }
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
    let retry = {
        let id = stream.id.clone();
        move |_| {
            let id = id.clone();
            busy.set(true);
            spawn(async move {
                let result = api::media_streams::retry_segments(&id).await;
                busy.set(false);
                match result {
                    Ok(n) => {
                        toasts::success(t!("streams-seg-requeued", count: n).to_string());
                        reload.with_mut(|r| *r += 1);
                    }
                    Err(err) => toasts::error(err),
                }
            });
        }
    };
    rsx! {
        Modal {
            title: t!("streams-segments-title", name : stream.name.clone()).to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close,
            div { class: "space-y-3",
                div { class: "flex flex-wrap items-center gap-2",
                    select {
                        class: INPUT,
                        aria_label: t!("streams-seg-filter"),
                        onchange: move |e| state.set(e.value()),
                        option { value: "", selected: state().is_empty(), {t!("streams-seg-all")} }
                        for st in SegmentState::ALL {
                            option {
                                value: st.wire(),
                                selected: state() == st.wire(),
                                {state_label(st)}
                            }
                        }
                    }
                    if can_write {
                        button {
                            class: "ml-auto px-3 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-40",
                            r#type: "button",
                            disabled: busy(),
                            onclick: retry,
                            {t!("streams-seg-retry")}
                        }
                    }
                }
                p { class: "text-xs text-gray-500", {t!("streams-seg-help")} }
                ListStates {
                    state: list_state,
                    is_empty,
                    empty_message: t!("streams-seg-empty").to_string(),
                    div { class: "space-y-3",
                        div { class: "overflow-x-auto border border-gray-200 rounded-lg",
                            table { class: "min-w-full divide-y divide-gray-200 text-sm",
                                thead { class: "bg-gray-50",
                                    tr {
                                        th { class: "th", {t!("streams-seg-col-start")} }
                                        th { class: "th hidden sm:table-cell",
                                            {t!("streams-seg-col-duration")}
                                        }
                                        th { class: "th", {t!("streams-seg-col-state")} }
                                        th { class: "th hidden sm:table-cell",
                                            {t!("streams-seg-col-asr")}
                                        }
                                    }
                                }
                                tbody { class: "divide-y divide-gray-100",
                                    for s in rows {
                                        SegmentRow { key: "{s.id}", segment: s.clone() }
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
fn SegmentRow(segment: MediaSegment) -> Element {
    let secs = duration_secs(&segment);
    let asr = segment
        .asr_ms
        .map(|ms| format!("{:.1} s", ms as f64 / 1000.0))
        .unwrap_or_default();
    rsx! {
        tr {
            td { class: "td whitespace-nowrap",
                {date_label(&segment.started_at)}
                span { class: "ml-1 text-xs text-gray-400", "#{segment.seq}" }
            }
            td { class: "td hidden text-gray-600 sm:table-cell", "{secs} s" }
            td { class: "td",
                span { class: "inline-flex px-2 py-0.5 rounded text-xs font-medium {state_class(segment.state)}",
                    {state_label(segment.state)}
                }
                if let Some(err) = segment.error.as_deref() {
                    p { class: "text-xs text-red-700 mt-0.5", {error_label(err)} }
                }
            }
            td { class: "td hidden text-gray-600 sm:table-cell", "{asr}" }
        }
    }
}
