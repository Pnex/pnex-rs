//! Versions of a YAML document (D221): a timeline with one mark per version, then
//! the list, and three verbs everywhere: Open (into the editor), Compare (with the
//! active version), Restore (becomes the active version).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::error::ApiError;
use crate::api::error_i18n::localize;
use crate::components::ontology::short_time;
use crate::components::transport::{ScrubBar, BAR};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_ERROR};

/// One version, whatever it is a version of.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionRow {
    pub version: i32,
    pub yaml: String,
    pub created_at: String,
    pub author: Option<String>,
}

/// Line diff of two YAML texts (each line a paragraph of `text_diff`).
fn yaml_diff(old: &str, new: &str) -> pnex_core::text_diff::TextDiff {
    let lines = |s: &str| s.lines().collect::<Vec<_>>().join("\n\n");
    pnex_core::text_diff::diff(&lines(old), &lines(new))
}

#[component]
pub fn VersionsDrawer(
    /// `None` while loading.
    rows: Option<Result<Vec<VersionRow>, ApiError>>,
    active_version: i32,
    can_write: bool,
    /// Why the last restore failed.
    #[props(default)]
    error: Option<String>,
    /// Label of the restore verb, when the caller's word differs (skills: "Activate").
    #[props(default)]
    restore_label: Option<String>,
    on_close: EventHandler<()>,
    on_open: EventHandler<VersionRow>,
    on_restore: EventHandler<i32>,
) -> Element {
    let restore = restore_label.unwrap_or_else(|| t!("web-sites-version-restore").to_string());
    let mut cursor = use_signal(|| None::<i32>);
    let mut comparing = use_signal(|| None::<i32>);
    let list: Vec<VersionRow> = match &rows {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
    };
    let failed = match &rows {
        Some(Err(e)) => Some(localize(e)),
        _ => None,
    };
    let active_yaml = list
        .iter()
        .find(|v| v.version == active_version)
        .map(|v| v.yaml.clone())
        .unwrap_or_default();
    // Oldest left, newest right.
    let n = list.len();
    let pos = |k: usize| {
        if n <= 1 {
            100.0
        } else {
            k as f64 / (n - 1) as f64 * 100.0
        }
    };
    let ordered: Vec<i32> = list.iter().rev().map(|v| v.version).collect();
    let marks: Vec<f64> = (0..n).map(pos).collect();
    let shown = cursor().unwrap_or(active_version);
    let cursor_at = ordered.iter().position(|v| *v == shown).map(pos);
    let ticks: Vec<(f64, String)> = ordered
        .iter()
        .enumerate()
        .map(|(k, v)| (pos(k), format!("v{v}")))
        .collect();
    let on_seek = {
        let ordered = ordered.clone();
        move |pct: f64| {
            let k = if n <= 1 {
                0
            } else {
                (pct / 100.0 * (n - 1) as f64).round() as usize
            };
            cursor.set(ordered.get(k.min(n - 1)).copied());
        }
    };
    rsx! {
        div { class: "fixed inset-0 z-40",
            div {
                class: "absolute inset-0",
                onclick: move |_| on_close.call(()),
            }
            aside { class: "pnex-drawer-in-right absolute inset-y-0 right-0 flex w-[28rem] max-w-full flex-col border-l border-line bg-surface shadow-xl",
                div { class: "flex items-center justify-between border-b border-line px-4 py-3",
                    h3 { class: "text-sm font-semibold text-ink", {t!("web-sites-versions")} }
                    button {
                        class: "text-slate hover:text-ink",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                if n > 1 {
                    div { class: "px-4 pt-3",
                        div { class: BAR,
                            ScrubBar {
                                cursor: cursor_at,
                                marks,
                                ticks,
                                on_seek,
                            }
                        }
                    }
                }
                if let Some(e) = error {
                    p { class: "{FIELD_ERROR} px-4", "{e}" }
                }
                if let Some(e) = failed {
                    p { class: "{FIELD_ERROR} p-4", "{e}" }
                }
                ul { class: "flex-1 divide-y divide-line overflow-y-auto",
                    for v in list.clone() {
                        li {
                            key: "{v.version}",
                            class: if shown == v.version { "space-y-2 bg-signal-50 px-4 py-3" } else { "space-y-2 px-4 py-3" },
                            div { class: "flex items-center gap-2 text-sm",
                                span { class: "font-semibold text-ink", "v{v.version}" }
                                if v.version == active_version {
                                    span { class: "rounded-full border border-green-200 bg-green-50 px-2 text-xs text-green-800",
                                        {t!("web-sites-version-active")}
                                    }
                                }
                                span { class: "ml-auto text-xs text-slate",
                                    "{short_time(&v.created_at)}"
                                }
                            }
                            if let Some(a) = v.author.clone() {
                                p { class: "text-xs text-slate", "{a}" }
                            }
                            div { class: "flex flex-wrap gap-2",
                                button {
                                    class: BTN_SECONDARY_SM,
                                    onclick: {
                                        let v = v.clone();
                                        move |_| on_open.call(v.clone())
                                    },
                                    {t!("web-sites-version-open")}
                                }
                                if v.version != active_version {
                                    button {
                                        class: BTN_SECONDARY_SM,
                                        onclick: {
                                            let ver = v.version;
                                            move |_| comparing.set(if comparing() == Some(ver) { None } else { Some(ver) })
                                        },
                                        {t!("web-sites-version-compare")}
                                    }
                                    if can_write {
                                        button {
                                            class: BTN_SECONDARY_SM,
                                            onclick: {
                                                let ver = v.version;
                                                move |_| on_restore.call(ver)
                                            },
                                            {restore.clone()}
                                        }
                                    }
                                }
                            }
                            if comparing() == Some(v.version) {
                                DiffView { diff: yaml_diff(&v.yaml, &active_yaml) }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn DiffView(diff: pnex_core::text_diff::TextDiff) -> Element {
    rsx! {
        div { class: "space-y-1 rounded bg-gray-50 p-2 font-mono text-xs",
            p { class: "text-slate", {t!("web-sites-version-diff", summary : diff.summary())} }
            for (i, h) in diff.hunks.iter().enumerate() {
                div { key: "{i}",
                    for line in h.before.iter() {
                        p { class: "whitespace-pre-wrap text-alarm", "- {line}" }
                    }
                    for line in h.after.iter() {
                        p { class: "whitespace-pre-wrap text-green-800", "+ {line}" }
                    }
                }
            }
        }
    }
}
