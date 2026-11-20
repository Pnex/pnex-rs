//! Crosstab tab (analysis.md AN4): a count, sum or average of the objects of a
//! type, split in rows and columns by two properties, with filters. No code; the
//! same calculation as a live block of a page, computed with the reader's rights.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::ui::{BTN_PRIMARY, FIELD_ERROR, FIELD_HELP};
use crate::doc_editor::LiveResult;
use crate::pages::docs::live::{compute, Draft, ObjectsFields};

#[component]
pub(super) fn PivotPanel() -> Element {
    let draft = use_signal(Draft::objects);
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let mut result = use_signal(|| None::<LiveResult>);
    let mut busy = use_signal(|| false);
    let all_types = types.read().clone().unwrap_or_default();
    let ready = draft().complete() && !busy();
    let run = move |_| {
        let types = types.read().clone().unwrap_or_default();
        let block = draft().block("pivot", &types);
        busy.set(true);
        spawn(async move {
            result.set(Some(compute(&block, true, &types).await));
            busy.set(false);
        });
    };
    rsx! {
        div { class: "space-y-4 max-w-xl",
            p { class: FIELD_HELP, {t!("pipe-pivot-help")} }
            ObjectsFields { draft, types: all_types }
            button {
                r#type: "button",
                class: BTN_PRIMARY,
                disabled: !ready,
                onclick: run,
                {t!("pipe-query-run")}
            }
        }
        if let Some(r) = result() {
            div { class: "mt-4 space-y-2", "data-testid": "pivot-result",
                if r.status == "error" {
                    p { class: FIELD_ERROR, role: "alert", "{r.note}" }
                } else {
                    div { class: "overflow-x-auto rounded-lg border border-line bg-white",
                        table { class: "min-w-full text-xs tabular-nums",
                            thead {
                                tr {
                                    for (i, c) in r.columns.iter().enumerate() {
                                        th {
                                            key: "{i}",
                                            class: "border-b border-line px-2 py-1.5 text-left font-medium text-ink",
                                            "{c}"
                                        }
                                    }
                                }
                            }
                            tbody {
                                for (i, row) in r.rows.iter().enumerate() {
                                    tr { key: "{i}",
                                        for (j, v) in row.iter().enumerate() {
                                            td {
                                                key: "{j}",
                                                class: if j == 0 { "border-b border-line px-2 py-1 text-ink font-medium" } else { "border-b border-line px-2 py-1 text-ink text-right" },
                                                "{v}"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    p { class: "text-sm text-slate", "{r.note} {r.more}" }
                }
            }
        }
    }
}
