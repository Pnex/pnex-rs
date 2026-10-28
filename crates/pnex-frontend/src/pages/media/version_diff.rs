//! "Compare" two versions of a document (partner-watch.md D311): the
//! paragraphs added, removed and changed, from the server diff.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::text_diff::{Change, TextDiff};

use crate::api;
use crate::components::modal::Modal;
use crate::components::ui::INPUT;

#[component]
pub(super) fn VersionDiffDialog(
    asset_id: String,
    versions: Vec<i64>,
    on_close: EventHandler<()>,
) -> Element {
    let last = versions.iter().copied().max().unwrap_or(1);
    let before = versions
        .iter()
        .copied()
        .filter(|v| *v < last)
        .max()
        .unwrap_or(last);
    let from = use_signal(move || before);
    let to = use_signal(move || last);
    let id = asset_id.clone();
    let diff = use_resource(move || {
        let id = id.clone();
        let (a, b) = (from(), to());
        async move { api::media::versions_diff(&id, a, b).await }
    });
    let pick = |label: String, value: i64, mut sig: Signal<i64>, all: Vec<i64>| {
        rsx! {
            label { class: "text-sm text-gray-700", "{label}" }
            select {
                class: INPUT,
                onchange: move |e| sig.set(e.value().parse().unwrap_or(value)),
                for v in all {
                    option { key: "{v}", value: "{v}", selected: v == value, "v{v}" }
                }
            }
        }
    };
    let body = match &*diff.read() {
        None => rsx! {
            p { class: "text-sm text-gray-500", {t!("common-loading")} }
        },
        Some(Err(e)) => {
            let msg = crate::api::error_i18n::localize(e);
            rsx! {
                p { class: "text-sm text-red-600", "{msg}" }
            }
        }
        Some(Ok(d)) => rsx! {
            DiffBody { diff: d.clone() }
        },
    };
    rsx! {
        Modal {
            title: t!("media-diff-title").to_string(),
            max_width: "max-w-3xl".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-3",
                div { class: "grid grid-cols-2 gap-3",
                    div { {pick(t!("media-diff-from").to_string(), from(), from, versions.clone())} }
                    div { {pick(t!("media-diff-to").to_string(), to(), to, versions.clone())} }
                }
                {body}
            }
        }
    }
}

#[component]
fn DiffBody(diff: TextDiff) -> Element {
    rsx! {
        p {
            class: "text-sm text-gray-700",
            "data-testid": "media-diff-summary",
            {
                t!(
                    "media-diff-counts", added : diff.added, removed : diff.removed, changed :
                    diff.changed
                )
            }
        }
        if diff.hunks.is_empty() {
            p { class: "text-sm text-gray-500", {t!("media-diff-same")} }
        }
        if diff.truncated {
            p { class: "text-xs text-amber-700", {t!("media-diff-truncated")} }
        }
        div { class: "max-h-[60vh] space-y-2 overflow-y-auto text-sm",
            for (i, h) in diff.hunks.iter().enumerate() {
                div {
                    key: "{i}",
                    class: "rounded border border-gray-200 p-2 space-y-1",
                    p { class: "text-xs text-gray-500",
                        match h.change {
                            Change::Added => t!("media-diff-added"),
                            Change::Removed => t!("media-diff-removed"),
                            Change::Changed => t!("media-diff-changed"),
                        }
                    }
                    for (k, line) in h.before.iter().enumerate() {
                        p {
                            key: "b{k}",
                            class: "rounded bg-red-50 px-2 text-red-800 line-through",
                            "{line}"
                        }
                    }
                    for (k, line) in h.after.iter().enumerate() {
                        p {
                            key: "a{k}",
                            class: "rounded bg-green-50 px-2 text-green-800",
                            "{line}"
                        }
                    }
                }
            }
        }
    }
}
