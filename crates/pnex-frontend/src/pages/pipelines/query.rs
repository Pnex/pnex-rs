//! SQL query tab (analysis.md D825–D828): one read-only SELECT on the tables and on
//! `objects.<type>`, sent to the worker, followed until it answers.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::analysis::{AnalysisQuery, AnalysisState};

use super::code_label;
use crate::api;
use crate::api::error_i18n::localize;
use crate::components::ui::{BTN_PRIMARY, FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access::can;

/// How often the tab asks for the answer.
const POLL_MS: u64 = 700;

#[component]
pub(super) fn QueryPanel() -> Element {
    let mut sql = use_signal(String::new);
    let mut current = use_signal(|| None::<AnalysisQuery>);
    let mut error = use_signal(|| None::<String>);
    let running = current()
        .as_ref()
        .is_some_and(|q| matches!(q.state, AnalysisState::Queued | AnalysisState::Running));
    let mut run = move || {
        let text = sql().trim().to_string();
        if text.is_empty() || running {
            return;
        }
        error.set(None);
        spawn(async move {
            let mut q = match api::analysis::submit(&text).await {
                Ok(q) => q,
                Err(e) => {
                    error.set(Some(localize(&e)));
                    return;
                }
            };
            loop {
                current.set(Some(q.clone()));
                if matches!(q.state, AnalysisState::Done | AnalysisState::Failed) {
                    break;
                }
                crate::util::sleep(std::time::Duration::from_millis(POLL_MS)).await;
                q = match api::analysis::get(&q.id).await {
                    Ok(q) => q,
                    Err(e) => {
                        error.set(Some(localize(&e)));
                        break;
                    }
                };
            }
        });
    };
    if !can("analysis:run") {
        return rsx! {
            p { class: "text-slate text-center py-12", {t!("access-not-allowed")} }
        };
    }
    let state_text = current().map(|q| match q.state {
        AnalysisState::Queued => t!("pipe-query-queued").to_string(),
        AnalysisState::Running => t!("pipe-query-running").to_string(),
        _ => String::new(),
    });
    rsx! {
        div { class: "space-y-4",
            div {
                label { class: FIELD_LABEL, r#for: "query-sql", {t!("pipe-query-label")} }
                textarea {
                    id: "query-sql",
                    class: "{INPUT} font-mono",
                    rows: "8",
                    spellcheck: "false",
                    value: "{sql}",
                    oninput: move |e| sql.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && e.modifiers().ctrl() {
                            e.prevent_default();
                            run();
                        }
                    },
                }
                p { class: FIELD_HELP,
                    {t!("pipe-query-help")}
                    " "
                    {t!("pipe-query-shortcut")}
                }
            }
            div { class: "flex items-center gap-3",
                button {
                    r#type: "button",
                    class: BTN_PRIMARY,
                    disabled: running || sql().trim().is_empty(),
                    onclick: move |_| run(),
                    {t!("pipe-query-run")}
                }
                if let Some(text) = state_text.filter(|t| !t.is_empty()) {
                    span { class: "text-sm text-slate", role: "status", "{text}" }
                }
            }
            if let Some(e) = error() {
                p { class: FIELD_ERROR, role: "alert", "{e}" }
            }
            if let Some(q) = current() {
                QueryResult { query: q }
            }
        }
    }
}

#[component]
fn QueryResult(query: AnalysisQuery) -> Element {
    match query.state {
        AnalysisState::Failed => {
            let code = query.code.clone().unwrap_or_default();
            let reason = query
                .args
                .get("reason")
                .and_then(|r| r.as_str())
                .map(str::to_string);
            rsx! {
                div { class: "space-y-1", role: "alert",
                    p { class: FIELD_ERROR, {code_label(&code, Some(&query.args))} }
                    // The engine's own words: a runtime diagnostic, shown as it is.
                    if let Some(r) = reason {
                        pre { class: "whitespace-pre-wrap break-words text-xs text-slate",
                            "{r}"
                        }
                    }
                }
            }
        }
        AnalysisState::Done => {
            let count = query.rows.len();
            rsx! {
                div { class: "space-y-2",
                    p { class: "text-sm text-slate tabular-nums",
                        {t!("pipe-query-rows", count : count)}
                        if query.truncated {
                            " "
                            {t!("pipe-query-truncated")}
                        }
                    }
                    div {
                        class: "overflow-x-auto rounded-lg border border-line bg-white",
                        "data-testid": "query-result",
                        table { class: "min-w-full text-xs",
                            thead {
                                tr {
                                    for (i, c) in query.columns.iter().enumerate() {
                                        th {
                                            key: "{i}",
                                            class: "border-b border-line px-2 py-1.5 text-left font-medium text-ink",
                                            "{c}"
                                        }
                                    }
                                }
                            }
                            tbody {
                                for (i, row) in query.rows.iter().enumerate() {
                                    tr { key: "{i}",
                                        for (j, v) in row.iter().enumerate() {
                                            td {
                                                key: "{j}",
                                                class: "max-w-xs truncate border-b border-line px-2 py-1 text-ink",
                                                title: v.clone().unwrap_or_default(),
                                                {v.clone().unwrap_or_default()}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => rsx! {},
    }
}
