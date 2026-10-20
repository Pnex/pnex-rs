//! A run as Vision Lab shows a source: the selected step's screenshot
//! fills the stage, the shared player sits under it (D220) — transport
//! (play, clock, speed, run picker) and the shared timeline, one bar per
//! step as long as the step took, coloured by its result. What the run
//! extracted is listed in the side column.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{
    ExtractKind, RunExtract, RunStatus, RunStep, SiteRecipeRun, StepStatus,
};

use super::{action_label, reason_label, sleep_ms, status_label};
use crate::api;
use crate::components::modal::Modal;
use crate::components::ontology::short_time;
use crate::components::timeline::axis::Scale;
use crate::components::timeline::{Body, Item, Lane, TimeBase, Timeline};
use crate::components::transport::{speed_options, Clock, PlayButton, SpeedPicker};
use crate::components::ui::BTN_SECONDARY_SM;

/// Element id of the lanes column.
const CANVAS: &str = "site-run-frise";
/// Playback speeds (steps per 1.5 s).
const SPEEDS: [u32; 3] = [1, 2, 4];

fn fill(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Ok => "#10b981",
        StepStatus::Failed => "#ef4444",
        StepStatus::Skipped => "#9ca3af",
    }
}

/// Start of each step, ms from the start of the run.
fn starts(steps: &[RunStep]) -> Vec<i64> {
    let mut at = 0i64;
    steps
        .iter()
        .map(|s| {
            let from = at;
            at += s.ms.max(1) as i64;
            from
        })
        .collect()
}

/// The selected step, large: its screenshot and what it did.
#[component]
pub(super) fn Stage(run: Option<SiteRecipeRun>, index: usize) -> Element {
    let step = run.as_ref().and_then(|r| r.steps.get(index).cloned());
    let run_id = run.as_ref().map(|r| r.id.clone()).unwrap_or_default();
    let mut big = use_signal(|| false);
    let path = step
        .as_ref()
        .and_then(|s| s.screenshot.as_ref())
        .map(|f| api::site_recipes::file_path(&run_id, f));
    let image = use_resource(use_reactive!(|path| async move {
        match path {
            Some(p) => crate::util::media_blob_url(&p, Some("image/jpeg")).await,
            None => None,
        }
    }));
    let url = image.read().clone().flatten();
    rsx! {
        div { class: "relative flex h-full min-h-0 items-center justify-center overflow-hidden rounded-lg border border-line bg-rail",
            match (step.clone(), url) {
                (Some(s), Some(src)) => rsx! {
                    img {
                        class: "max-h-full max-w-full cursor-zoom-in object-contain",
                        src: "{src}",
                        alt: "{s.label}",
                        onclick: move |_| big.set(true),
                    }
                    if big() {
                        Modal {
                            title: s.label.clone(),
                            max_width: "sm:max-w-6xl".to_string(),
                            on_close: move |_| big.set(false),
                            img { class: "w-full", src: "{src}", alt: "{s.label}" }
                        }
                    }
                },
                (Some(_), None) => rsx! {
                    p { class: "text-sm text-slate", {t!("web-sites-loading")} }
                },
                (None, _) => rsx! {
                    p { class: "max-w-md px-6 text-center text-sm text-slate", {t!("web-sites-run-empty")} }
                },
            }
            if let Some(s) = step {
                StepBand { step: s }
            }
        }
    }
}

/// Band over the bottom of the stage: step, action, address, reason.
#[component]
fn StepBand(step: RunStep) -> Element {
    let failed = step.status == StepStatus::Failed;
    let tone = if failed { "bg-red-700/90" } else { "bg-ink/75" };
    rsx! {
        div { class: "absolute inset-x-3 bottom-3 rounded-md px-3 py-2 text-sm text-white backdrop-blur-sm {tone}",
            div { class: "flex flex-wrap items-baseline gap-2",
                span { class: "font-semibold", "{step.index + 1}. {action_label(&step.action)}" }
                span { class: "truncate opacity-90", "{step.label}" }
                span { class: "ml-auto font-mono text-xs opacity-80", "{step.ms} ms" }
            }
            if let Some(r) = step.reason.clone() {
                p { class: "font-medium",
                    "{reason_label(&r)}"
                    if let Some(d) = step.detail.clone() {
                        span { class: "ml-1 font-mono text-xs opacity-90", "({d})" }
                    }
                }
            }
            p { class: "truncate font-mono text-xs opacity-80", "{step.url}" }
        }
    }
}

/// The player under the stage: transport, run picker, timeline.
#[component]
pub(super) fn RunTimeline(
    run: Option<SiteRecipeRun>,
    runs: Vec<SiteRecipeRun>,
    index: Signal<usize>,
    on_pick: EventHandler<String>,
) -> Element {
    let mut index = index;
    let mut playing = use_signal(|| false);
    let mut speed = use_signal(|| 1u32);
    let steps = run.as_ref().map(|r| r.steps.clone()).unwrap_or_default();
    let n = steps.len();
    // Play walks the steps; it stops on the last one.
    use_effect(move || {
        if !playing() {
            return;
        }
        spawn(async move {
            while playing() {
                sleep_ms(1500 / speed().max(1)).await;
                if !playing() {
                    break;
                }
                let next = *index.peek() + 1;
                if next >= n {
                    playing.set(false);
                    break;
                }
                index.set(next);
            }
        });
    });
    let i = index().min(n.saturating_sub(1));
    let at = starts(&steps);
    let end = at.last().copied().unwrap_or(0) + steps.last().map_or(1, |s| s.ms.max(1) as i64);
    let scale = Scale::linear((0, end.max(1000)));
    let cursor = at.get(i).copied().unwrap_or(0);
    let items: Vec<Item> = steps
        .iter()
        .zip(&at)
        .map(|(s, from)| Item {
            id: s.index.to_string(),
            from: *from,
            to: from + s.ms.max(1) as i64,
            label: format!("{} {}", s.index + 1, action_label(&s.action)),
            tip: Some(format!("{} · {} ms", s.label, s.ms)),
            fill: Some(fill(s.status).to_string()),
            selected: s.index as usize == i,
            ..Default::default()
        })
        .collect();
    let extracts: Vec<Item> = steps
        .iter()
        .zip(&at)
        .filter(|(s, _)| {
            matches!(s.action.as_str(), "extract" | "follow_pdf") && s.status == StepStatus::Ok
        })
        .map(|(s, from)| Item {
            id: s.index.to_string(),
            from: *from,
            to: *from,
            label: s.label.clone(),
            fill: Some("#0ea5e9".to_string()),
            ..Default::default()
        })
        .collect();
    let lanes = vec![
        Lane {
            count: Some(n),
            ..Lane::new(
                "steps",
                t!("web-sites-lane-steps"),
                Body::Items {
                    items,
                    max_rows: 3,
                    stack: true,
                    empty: Some(t!("web-sites-run-empty").to_string()),
                },
            )
        },
        Lane {
            tinted: true,
            ..Lane::new(
                "extracts",
                t!("web-sites-extracts"),
                Body::Items {
                    items: extracts,
                    max_rows: 1,
                    stack: false,
                    empty: None,
                },
            )
        },
    ];
    let seek_at = at.clone();
    let seek = move |t: i64| {
        let k = seek_at.iter().rposition(|s| *s <= t).unwrap_or(0);
        playing.set(false);
        index.set(k);
    };
    let current = run.as_ref().map(|r| r.id.clone()).unwrap_or_default();
    let run_label = |r: &SiteRecipeRun| {
        let version = match r.recipe_version {
            Some(v) => format!("v{v}"),
            None => t!("web-sites-draft").to_string(),
        };
        format!(
            "{} · {} · {version} · {}",
            short_time(&r.started_at),
            r.name,
            status_label(r.status)
        )
    };
    let failed = run
        .as_ref()
        .filter(|r| r.status == RunStatus::Failed)
        .map(|r| match r.failed_step {
            Some(f) => t!(
                "web-sites-failed-at",
                n: f + 1,
                reason: reason_label(r.reason.as_deref().unwrap_or_default())
            )
            .to_string(),
            None => reason_label(r.reason.as_deref().unwrap_or_default()),
        });
    rsx! {
        section { class: "shrink-0 overflow-hidden rounded-lg border border-line bg-surface",
            div { class: "flex flex-wrap items-center gap-2 border-b border-line px-3 py-2",
                PlayButton {
                    playing: playing(),
                    on_toggle: move |_| {
                        if !playing() && *index.peek() + 1 >= n {
                            index.set(0);
                        }
                        playing.toggle();
                    },
                }
                Clock {
                    time: if n > 0 { t!("web-sites-step-of", n : i + 1, total : n).to_string() } else { "—".to_string() },
                    sub: steps.get(i).map(|s| format!("{} ms", s.ms)),
                }
                SpeedPicker {
                    value: speed(),
                    options: speed_options(&SPEEDS),
                    on_change: move |v| speed.set(v),
                }
                div { class: "min-w-0 flex-1 basis-56",
                    select {
                        class: "h-8 w-full min-w-0 rounded-md border border-line-strong bg-surface px-2 text-sm text-ink",
                        "aria-label": t!("web-sites-when"),
                        onchange: move |e| on_pick.call(e.value()),
                        if runs.is_empty() {
                            option { value: "", {t!("web-sites-journal-empty")} }
                        }
                        for r in runs.iter() {
                            option {
                                key: "{r.id}",
                                value: "{r.id}",
                                selected: r.id == current,
                                {run_label(r)}
                            }
                        }
                    }
                }
                if let Some(f) = failed {
                    span { class: "text-sm font-medium text-alarm", "{f}" }
                }
            }
            Timeline {
                canvas: CANVAS,
                scale,
                base: TimeBase::Media,
                cursor,
                lanes,
                header: t!("web-sites-lane-time").to_string(),
                density: false,
                on_seek: seek,
                on_item: move |(_, id): (String, String)| {
                    if let Ok(k) = id.parse::<usize>() {
                        playing.set(false);
                        index.set(k);
                    }
                },
            }
        }
    }
}

#[component]
pub(super) fn Extracts(run: Option<SiteRecipeRun>) -> Element {
    let Some(run) = run else {
        return rsx! {
            p { class: "text-sm text-slate", {t!("web-sites-run-empty")} }
        };
    };
    rsx! {
        if run.extracts.is_empty() {
            p { class: "text-sm text-slate", {t!("web-sites-no-extract")} }
        }
        div { class: "space-y-2",
            for e in run.extracts.clone() {
                ExtractCard {
                    key: "{e.name}",
                    run_id: run.id.clone(),
                    diff: run.changes.get(&e.name).cloned(),
                    extract: e,
                }
            }
        }
    }
}

#[component]
fn ExtractCard(
    run_id: String,
    extract: RunExtract,
    diff: Option<pnex_core::text_diff::TextDiff>,
) -> Element {
    let mut open = use_signal(|| false);
    let kind = match extract.kind {
        ExtractKind::Text => t!("web-sites-extract-text").to_string(),
        ExtractKind::Pdf => t!("web-sites-extract-pdf").to_string(),
    };
    let pdf_file = extract.file.clone();
    let download = move |_| {
        let Some(file) = pdf_file.clone() else {
            return;
        };
        let path = api::site_recipes::file_path(&run_id, &file);
        spawn(async move {
            if let Ok(bytes) = api::client::request_bytes(reqwest::Method::GET, &path).await {
                crate::util::save_blob(&file, &bytes);
            }
        });
    };
    rsx! {
        div { class: "rounded border border-line p-2 space-y-1",
            div { class: "flex flex-wrap items-center gap-2 text-sm",
                span { class: "font-medium text-ink", "{extract.name}" }
                span { class: "text-xs text-slate",
                    "{kind} · "
                    {t!("web-sites-chars", n : extract.chars)}
                }
                match &diff {
                    Some(d) => rsx! {
                        span { class: "rounded border border-amber-200 bg-amber-50 px-1.5 text-xs text-amber-800",
                            {t!("web-sites-changed", summary : d.summary())}
                        }
                    },
                    None => rsx! {
                        span { class: "text-xs text-slate", {t!("web-sites-unchanged")} }
                    },
                }
                if extract.file.is_some() {
                    button {
                        class: "{BTN_SECONDARY_SM} ml-auto",
                        onclick: download,
                        {t!("web-sites-download-pdf")}
                    }
                }
            }
            button { class: "text-xs text-signal", onclick: move |_| open.toggle(),
                if open() {
                    {t!("web-sites-hide-text")}
                } else {
                    {t!("web-sites-show-text")}
                }
            }
            if open() {
                if let Some(d) = diff.clone() {
                    div { class: "space-y-1",
                        for (i, h) in d.hunks.iter().enumerate() {
                            div {
                                key: "{i}",
                                class: "rounded bg-gray-50 p-1 text-xs",
                                for line in h.before.iter() {
                                    p { class: "text-alarm line-through whitespace-pre-wrap",
                                        "{line}"
                                    }
                                }
                                for line in h.after.iter() {
                                    p { class: "text-green-800 whitespace-pre-wrap",
                                        "{line}"
                                    }
                                }
                            }
                        }
                    }
                }
                pre { class: "max-h-96 overflow-auto whitespace-pre-wrap rounded bg-gray-50 p-2 text-xs text-ink",
                    "{extract.text}"
                }
            }
        }
    }
}
