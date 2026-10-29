//! Checks panel of a watch case (network-check.md D430): per check, the
//! worst latest result, its rules, the last run and since when it holds.
//! Read from the workspace (`checks`), no journal query when the case opens.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::CaseCheck;

use crate::components::badges::date_label;

fn result_class(result: &str) -> &'static str {
    match result {
        "ok" => "bg-green-100 text-green-800",
        "finding" => "bg-red-100 text-red-800",
        _ => "bg-amber-100 text-amber-800",
    }
}

fn result_label(result: &str) -> String {
    match result {
        "ok" => t!("cases-checks-result-ok"),
        "finding" => t!("cases-checks-result-finding"),
        _ => t!("cases-checks-result-unknown"),
    }
}

#[component]
pub fn Checks(checks: Vec<CaseCheck>) -> Element {
    rsx! {
        div {
            class: "border-t border-line bg-surface px-3 py-2 text-xs",
            "data-testid": "case-checks",
            div { class: "mb-1 font-medium text-ink", {t!("cases-checks-title")} }
            for c in checks {
                CheckRow { key: "{c.check}", check: c }
            }
        }
    }
}

#[component]
fn CheckRow(check: CaseCheck) -> Element {
    let last = check
        .last_run_at
        .as_deref()
        .map(date_label)
        .unwrap_or_default();
    let since = check
        .changed_at
        .as_deref()
        .map(date_label)
        .unwrap_or_default();
    let runs = check.runs_since_change;
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-2 py-1",
            "data-testid": "case-check",
            title: t!("cases-checks-source", source : check.source_ref.clone()),
            span { class: "rounded-full px-2 py-0.5 font-medium {result_class(&check.result)}",
                {result_label(&check.result)}
            }
            span { class: "font-mono text-ink", "{check.check}" }
            span { class: "text-slate", {t!("cases-checks-last-run", at : last)} }
            span { class: "text-slate", {t!("cases-checks-since", at : since, runs : runs)} }
            for r in check.rules.iter().filter(|r| r.result != "ok") {
                span {
                    class: "rounded px-1.5 py-0.5 {result_class(&r.result)}",
                    title: r.reason.clone().unwrap_or_default(),
                    "{r.code}"
                }
            }
        }
    }
}
