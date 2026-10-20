//! Versions of a rule (D221): a timeline with one mark per version, then
//! the list, and three verbs everywhere — Open (into the editor), Compare
//! (with the active version), Restore (becomes the active version).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{SiteRecipe, SiteRecipeVersion};

use crate::api;
use crate::api::error_i18n::localize;
use crate::components::ontology::short_time;
use crate::components::transport::{ScrubBar, BAR};
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_ERROR};

/// Line diff of two YAML texts (each line a paragraph of `text_diff`).
fn yaml_diff(old: &str, new: &str) -> pnex_core::text_diff::TextDiff {
    let lines = |s: &str| s.lines().collect::<Vec<_>>().join("\n\n");
    pnex_core::text_diff::diff(&lines(old), &lines(new))
}

#[component]
pub(super) fn VersionsDrawer(
    recipe_id: String,
    active_version: i32,
    can_write: bool,
    on_close: EventHandler<()>,
    on_open: EventHandler<SiteRecipeVersion>,
    on_restored: EventHandler<SiteRecipe>,
) -> Element {
    let id = recipe_id.clone();
    let versions = use_resource(move || {
        let id = id.clone();
        async move { api::site_recipes::versions(&id).await }
    });
    let mut cursor = use_signal(|| None::<i32>);
    let mut comparing = use_signal(|| None::<i32>);
    let mut error = use_signal(|| None::<String>);
    let list: Vec<SiteRecipeVersion> = match &*versions.read() {
        Some(Ok(v)) => v.clone(),
        _ => Vec::new(),
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
                if let Some(e) = error() {
                    p { class: "{FIELD_ERROR} px-4", "{e}" }
                }
                match &*versions.read() {
                    Some(Err(e)) => rsx! {
                        p { class: "{FIELD_ERROR} p-4", "{localize(e)}" }
                    },
                    None => rsx! {},
                    Some(Ok(_)) => rsx! {
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
                                        span { class: "ml-auto text-xs text-slate", "{short_time(&v.created_at)}" }
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
                                                        let id = recipe_id.clone();
                                                        let ver = v.version;
                                                        move |_| {
                                                            let id = id.clone();
                                                            spawn(async move {
                                                                match api::site_recipes::restore(&id, ver).await {
                                                                    Ok(r) => on_restored.call(r),
                                                                    Err(e) => error.set(Some(localize(&e))),
                                                                }
                                                            });
                                                        }
                                                    },
                                                    {t!("web-sites-version-restore")}
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
                    },
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
