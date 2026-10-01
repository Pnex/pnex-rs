//! Editor toolbar menus (« + Lib », « + PneX API ») and the revision
//! history drawer of the firmware IDE.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::firmware::LibCatalogItem;

use crate::api;
use crate::components::badges::date_label;
use crate::components::icons;

const TOOL_BTN: &str =
    "h-8 px-2.5 rounded-md border border-gray-300 bg-white text-[13px] text-gray-700 hover:bg-gray-50 disabled:opacity-40";
const MENU: &str =
    "absolute right-0 top-9 z-30 w-96 max-h-[420px] overflow-y-auto rounded-xl border border-gray-200 bg-white shadow-lg py-1";
const MENU_ITEM: &str = "w-full text-left px-3 py-2 hover:bg-gray-50 flex flex-col gap-0.5";
const SMALL_BTN: &str =
    "px-2 py-0.5 text-xs text-blue-700 bg-blue-50 border border-blue-200 rounded-md hover:bg-blue-100 disabled:opacity-40";

/// PneX API snippets: (label key, snippet inserted at the caret).
const API_SNIPPETS: &[(&str, &str)] = &[
    ("firmware-api-add-metric", "pnex.addMetric(\"temp\", \"°C\");\n"),
    ("firmware-api-publish", "pnex.publish(\"temp\", value);\n"),
    (
        "firmware-api-on-command",
        "pnex.onCommand(\"calibrate\", [](JsonVariantConst args) {\n    return true;\n});\n",
    ),
    ("firmware-api-add-output", "pnex.addOutput(5, \"relay1\");\n"),
    ("firmware-api-add-input", "pnex.addInput(12, \"door\", true);\n"),
    ("firmware-api-add-analog", "pnex.addAnalogInput(34, \"light\");\n"),
    (
        "firmware-api-every",
        "static unsigned long last_ms = 0;\nif (millis() - last_ms >= 10000) {\n    last_ms = millis();\n}\n",
    ),
];

/// « + Lib » menu: catalog libraries of the chip. Picking one adds it to
/// the project's `lib_deps` and inserts its `#include` at the caret.
#[component]
pub(super) fn LibMenu(
    chip: String,
    selected: Signal<Vec<String>>,
    readonly: bool,
    on_include: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let catalog = use_resource(|| async { api::firmware::lib_catalog().await });
    let items: Vec<LibCatalogItem> = match &*catalog.value().read() {
        Some(Ok(all)) => all
            .iter()
            .filter(|e| e.chip_families.is_empty() || e.chip_families.contains(&chip))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    rsx! {
        div { class: "relative",
            button {
                class: TOOL_BTN,
                disabled: readonly,
                onclick: move |_| open.toggle(),
                icons::Plus { class: "h-3.5 w-3.5 inline mr-1" }
                {t!("firmware-menu-lib")}
            }
            if open() {
                div { class: "fixed inset-0 z-20", onclick: move |_| open.set(false) }
                div { class: MENU,
                    p { class: "px-3 py-1.5 text-xs text-gray-500", {t!("firmware-libs-hint")} }
                    for item in items {
                        LibItem { item, selected, open, on_include }
                    }
                }
            }
        }
    }
}

#[component]
fn LibItem(
    item: LibCatalogItem,
    selected: Signal<Vec<String>>,
    open: Signal<bool>,
    on_include: EventHandler<String>,
) -> Element {
    let mut selected = selected;
    let mut open = open;
    let already = selected.read().contains(&item.id);
    let version = item
        .pio_spec
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .to_string();
    let desc_key = format!("fw-lib-{}", item.id);
    let id = item.id.clone();
    let header = item.header.clone();
    rsx! {
        button {
            class: MENU_ITEM,
            onclick: move |_| {
                if !selected.read().contains(&id) {
                    selected.with_mut(|v| v.push(id.clone()));
                }
                on_include.call(header.clone());
                open.set(false);
            },
            span { class: "text-sm text-gray-900 flex items-center gap-1.5",
                {item.name.clone()}
                span { class: "text-xs text-gray-400", {version} }
                if already {
                    icons::Check { class: "h-3.5 w-3.5 text-green-600" }
                }
            }
            span { class: "text-xs text-gray-500", {t!(&desc_key)} }
        }
    }
}

/// Selected libraries as removable chips (next to the toolbar menus).
#[component]
pub(super) fn LibChips(selected: Signal<Vec<String>>, readonly: bool) -> Element {
    let mut selected = selected;
    let ids = selected();
    rsx! {
        for id in ids {
            span { key: "{id}", class: "inline-flex items-center gap-1 h-7 px-2 rounded-full bg-slate-100 text-slate-700 text-xs font-mono",
                {id.clone()}
                if !readonly {
                    button {
                        class: "text-slate-400 hover:text-slate-700",
                        title: t!("firmware-lib-remove"),
                        onclick: move |_| {
                            let target = id.clone();
                            selected.with_mut(|v| v.retain(|x| *x != target));
                        },
                        icons::X { class: "h-3 w-3" }
                    }
                }
            }
        }
    }
}

/// « + PneX API » menu — picking an entry inserts its snippet at the caret.
#[component]
pub(super) fn ApiMenu(readonly: bool, on_insert: EventHandler<String>) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "relative",
            button {
                class: TOOL_BTN,
                disabled: readonly,
                onclick: move |_| open.toggle(),
                icons::Plus { class: "h-3.5 w-3.5 inline mr-1" }
                {t!("firmware-menu-api")}
            }
            if open() {
                div { class: "fixed inset-0 z-20", onclick: move |_| open.set(false) }
                div { class: MENU,
                    for (key, snippet) in API_SNIPPETS.iter().copied() {
                        button {
                            class: MENU_ITEM,
                            onclick: move |_| {
                                on_insert.call(snippet.to_string());
                                open.set(false);
                            },
                            span { class: "text-sm text-gray-900", {t!(key)} }
                            code { class: "text-[11px] font-mono text-gray-600 truncate",
                                {snippet.lines().next().unwrap_or_default()}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Revision history drawer — any past revision can be loaded into the
/// editor as a local draft (saving then creates a new revision).
#[component]
pub(super) fn RevisionsDrawer(
    project_id: i64,
    current_revision: i64,
    on_close: Callback<()>,
    on_load: Callback<(String, Vec<String>)>,
) -> Element {
    let revisions = use_resource(move || async move { api::firmware::revisions(project_id).await });
    let close = move |_| on_close.call(());
    rsx! {
        div { class: "fixed inset-0 z-40",
            div { class: "absolute inset-0", onclick: close }
            aside { class: "absolute inset-y-0 right-0 w-96 max-w-full bg-white shadow-xl border-l border-gray-200 flex flex-col",
                div { class: "flex items-center justify-between px-4 py-3 border-b border-gray-200",
                    h3 { class: "text-sm font-semibold text-gray-900", {t!("firmware-revisions-title")} }
                    button { class: "text-gray-400 hover:text-gray-600", onclick: close, icons::X { class: "h-4 w-4" } }
                }
                match &*revisions.value().read() {
                    Some(Ok(rows)) => rsx! {
                        ul { class: "flex-1 overflow-y-auto divide-y divide-gray-100",
                            for rev in rows.clone() {
                                li { key: "{rev.id}", class: "px-4 py-3 space-y-1.5",
                                    div { class: "flex items-center gap-2",
                                        span { class: "text-sm font-semibold text-gray-900", {format!("r{}", rev.revision_number)} }
                                        if rev.revision_number == current_revision {
                                            span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                                {t!("firmware-revision-current")}
                                            }
                                        }
                                        span { class: "text-xs text-gray-400 ml-auto", {date_label(&rev.created_at)} }
                                    }
                                    if let Some(note) = &rev.note {
                                        p { class: "text-xs text-gray-600", {note.clone()} }
                                    }
                                    if !rev.lib_deps.is_empty() {
                                        p { class: "text-xs text-gray-500 font-mono", {rev.lib_deps.join(", ")} }
                                    }
                                    button {
                                        class: SMALL_BTN,
                                        onclick: move |_| on_load.call((rev.main_cpp.clone(), rev.lib_deps.clone())),
                                        {t!("firmware-revision-load")}
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "m-4 bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                            {crate::api::error_i18n::localize(err)}
                        }
                    },
                    None => rsx! {
                        div { class: "flex-1 flex items-center justify-center",
                            span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" }
                        }
                    },
                }
            }
        }
    }
}
