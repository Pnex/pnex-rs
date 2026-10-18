//! "Analyses" card of an asset (media-import-pipeline.md D213): one line
//! per step proposed for its kind (index, summary...), state, error,
//! Launch / Relaunch, and "Relaunch all". A step needing an LLM without a
//! provider is shown greyed with a link to the org detail (P2). Writers
//! only act; the server re-checks the role.

use super::*;
use crate::app::Route;
use crate::components::confirm::ConfirmDialog;
use pnex_core::err_codes;
use pnex_core::media_analysis::{
    AnalysisStep, MediaAnalysis, StartAnalyses, StepOffer, LARGE_CALLS,
};

/// Polling period while a step is in flight.
const POLL_SECS: u64 = 3;

/// Status line and optional detail line of an index state. Render scope
/// only (`t!`).
fn index_texts(state: &api::media::IndexState) -> (String, Option<String>) {
    match state.status.as_str() {
        "pending" => (t!("media-index-status-pending").to_string(), None),
        "extracting" => (t!("media-index-status-extracting").to_string(), None),
        "needs_ocr" => (t!("media-index-status-needs-ocr").to_string(), None),
        "indexed" => {
            let chunks = t!("media-index-chunks", n : state.chunk_count).to_string();
            let detail = match state.page_count {
                Some(pages) => format!("{} · {chunks}", t!("media-index-pages", n : pages)),
                None => chunks,
            };
            (t!("media-index-status-indexed").to_string(), Some(detail))
        }
        "error" => {
            let detail = state
                .error_code
                .as_deref()
                .map(|code| crate::api::error_i18n::localize_code(code, None, code));
            (t!("media-index-status-error").to_string(), detail)
        }
        other => (other.to_string(), None),
    }
}

fn step_label(step: AnalysisStep) -> String {
    match step {
        AnalysisStep::Index => t!("media-analysis-step-index"),
        AnalysisStep::Summarize => t!("media-analysis-step-summarize"),
        AnalysisStep::Transcribe => t!("media-analysis-step-transcribe"),
    }
    .to_string()
}

/// Status line and optional detail line of an analysis row.
fn analysis_texts(a: Option<&MediaAnalysis>) -> (String, Option<String>) {
    let Some(a) = a else {
        return (t!("media-analysis-state-none").to_string(), None);
    };
    match a.state.wire() {
        "queued" => (t!("media-analysis-state-queued").to_string(), None),
        // A transcription reports no percentage (the file length is not known).
        "running" if a.progress == 0 => (t!("media-analysis-state-busy").to_string(), None),
        "running" => (
            t!("media-analysis-state-running", pct : a.progress).to_string(),
            None,
        ),
        "done" => (t!("media-analysis-state-done").to_string(), None),
        "failed" => (
            t!("media-analysis-state-failed").to_string(),
            a.error_code
                .as_deref()
                .map(|code| crate::api::error_i18n::localize_code(code, None, code)),
        ),
        other => (other.to_string(), None),
    }
}

/// Whether a step of `state` is queued or running.
fn in_flight(state: &api::media::AnalysesState) -> bool {
    let index = state
        .index
        .as_ref()
        .is_some_and(|i| matches!(i.status.as_str(), "pending" | "extracting"));
    index
        || state
            .analyses
            .iter()
            .any(|a| matches!(a.state.wire(), "queued" | "running"))
}

#[component]
pub(super) fn AnalysesCard(asset_id: String, can_write: bool) -> Element {
    let mut state = use_signal(|| None::<Result<api::media::AnalysesState, String>>);
    let mut busy = use_signal(|| false);
    // Steps waiting for the large-file confirmation, with the estimate.
    let mut pending_large = use_signal(|| None::<(Vec<AnalysisStep>, u32)>);
    let id_for_poll = asset_id.clone();
    // Loads once, then refetches while a step is in flight. No reactive
    // read in the loop (peek): a refetch must not restart it.
    use_future(move || {
        let id = id_for_poll.clone();
        async move {
            let mut first = true;
            loop {
                let flying = matches!(&*state.peek(), Some(Ok(s)) if in_flight(s));
                if first || flying {
                    let next = api::media::analyses(&id)
                        .await
                        .map_err(|e| crate::api::error_i18n::localize(&e));
                    state.set(Some(next));
                    first = false;
                }
                sleep(std::time::Duration::from_secs(POLL_SECS)).await;
            }
        }
    });

    let id_for_start = asset_id.clone();
    let start = move |steps: Vec<AnalysisStep>, confirm_large: bool| {
        let id = id_for_start.clone();
        spawn(async move {
            busy.set(true);
            let input = StartAnalyses {
                steps,
                locale: crate::i18n::current_tag(),
                confirm_large,
            };
            match api::media::start_analyses(&id, &input).await {
                Ok(next) => {
                    toasts::success(t!("media-analysis-started"));
                    state.set(Some(Ok(next)));
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };

    let snapshot = state.read().clone();
    let (offers, analyses, index, load_error) = match snapshot {
        None => (Vec::new(), Vec::new(), None, None),
        Some(Ok(s)) => (s.offers, s.analyses, s.index, None),
        Some(Err(e)) => (Vec::new(), Vec::new(), None, Some(e)),
    };
    let runnable: Vec<AnalysisStep> = offers
        .iter()
        .filter(|o| o.blocked.is_none())
        .map(|o| o.step)
        .collect();
    // Estimate of the summary when it crosses the confirmation threshold.
    let large_calls = offers
        .iter()
        .find(|o| o.step == AnalysisStep::Summarize)
        .and_then(|o| o.calls)
        .filter(|c| *c > LARGE_CALLS);
    let request = {
        let start = start.clone();
        move |steps: Vec<AnalysisStep>| match large_calls {
            Some(calls) if steps.contains(&AnalysisStep::Summarize) => {
                pending_large.set(Some((steps, calls)))
            }
            _ => start(steps, false),
        }
    };

    rsx! {
        div { class: "rounded-xl border border-gray-200 bg-white p-4",
            div { class: "flex items-center justify-between gap-2",
                h3 { class: "text-sm font-semibold text-gray-700", {t!("media-analysis-title")} }
                if can_write && runnable.len() > 1 {
                    button {
                        class: "text-xs text-signal-600 hover:underline disabled:opacity-50",
                        r#type: "button",
                        disabled: busy(),
                        onclick: {
                            let mut request = request.clone();
                            let all = runnable.clone();
                            move |_| request(all.clone())
                        },
                        {t!("media-analysis-run-all")}
                    }
                }
            }
            if let Some(err) = load_error {
                div { class: "mt-2 text-sm text-red-600", "{err}" }
            }
            ul { class: "mt-2 divide-y divide-gray-100",
                for offer in offers {
                    StepLine {
                        key: "{offer.step.as_str()}",
                        offer: offer.clone(),
                        analysis: analyses.iter().find(|a| a.step == offer.step).cloned(),
                        index: index.clone(),
                        can_write,
                        disabled: busy(),
                        on_start: {
                            let mut request = request.clone();
                            move |step: AnalysisStep| request(vec![step])
                        },
                    }
                }
            }
            if let Some((steps, calls)) = pending_large() {
                ConfirmDialog {
                    title: t!("media-analysis-large-title"),
                    message: t!("media-analysis-large-message", calls : calls),
                    confirm_label: t!("media-analysis-large-confirm"),
                    on_confirm: {
                        let start = start.clone();
                        move |_| {
                            pending_large.set(None);
                            start(steps.clone(), true);
                        }
                    },
                    on_cancel: move |_| pending_large.set(None),
                }
            }
        }
    }
}

#[component]
fn StepLine(
    offer: StepOffer,
    analysis: Option<MediaAnalysis>,
    index: Option<api::media::IndexState>,
    can_write: bool,
    disabled: bool,
    on_start: EventHandler<AnalysisStep>,
) -> Element {
    let step = offer.step;
    let (status, detail) = match step {
        AnalysisStep::Index => match &index {
            Some(i) => index_texts(i),
            None => (t!("media-index-status-none").to_string(), None),
        },
        _ => analysis_texts(analysis.as_ref()),
    };
    let flying = match step {
        AnalysisStep::Index => index
            .as_ref()
            .is_some_and(|i| matches!(i.status.as_str(), "pending" | "extracting")),
        _ => analysis
            .as_ref()
            .is_some_and(|a| matches!(a.state.wire(), "queued" | "running")),
    };
    let ran = match step {
        AnalysisStep::Index => index.is_some(),
        _ => analysis.is_some(),
    };
    let page_id = analysis.as_ref().and_then(|a| a.page_id);
    rsx! {
        li { class: "py-2 flex items-start justify-between gap-3",
            div { class: "min-w-0",
                p { class: "text-sm text-gray-900", {step_label(step)} }
                if offer.blocked.as_deref() == Some(err_codes::MEDIA_ANALYSIS_NO_ASR) {
                    p { class: "text-xs text-gray-500",
                        {t!("media-analysis-needs-asr")}
                        " "
                        Link {
                            class: "text-signal-600 hover:underline",
                            to: Route::Streams {},
                            {t!("media-analysis-open-asr")}
                        }
                    }
                } else if offer.blocked.is_some() {
                    p { class: "text-xs text-gray-500",
                        {t!("media-analysis-needs-llm")}
                        " "
                        Link {
                            class: "text-signal-600 hover:underline",
                            to: Route::OrgsCurrent {},
                            {t!("media-analysis-connect-llm")}
                        }
                    }
                } else {
                    p { class: "text-xs text-gray-600", "{status}" }
                    if let Some(calls) = offer.calls {
                        p { class: "text-xs text-gray-500",
                            {t!("media-analysis-estimate", calls : calls)}
                        }
                    }
                    if let Some(d) = detail {
                        p { class: "text-xs text-gray-500", "{d}" }
                    }
                    if let Some(pid) = page_id {
                        Link {
                            class: "text-xs text-signal-600 hover:underline",
                            to: Route::Docs { id: pid.to_string() },
                            {t!("media-analysis-open-page")}
                        }
                    }
                }
            }
            if can_write && offer.blocked.is_none() && !flying {
                button {
                    class: "shrink-0 text-xs text-signal-600 hover:underline disabled:opacity-50",
                    r#type: "button",
                    disabled,
                    onclick: move |_| on_start.call(step),
                    if ran {
                        {t!("media-analysis-rerun")}
                    } else {
                        {t!("media-analysis-run")}
                    }
                }
            }
        }
    }
}
