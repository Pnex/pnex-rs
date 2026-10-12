//! Case frame (cases-workspace.md K33): the question, the processing basis
//! of personal data (required before a person is attached), the window
//! derived from the events, the detector thresholds, the frozen states, the
//! access log of persons' tracks, the hypotheses board, the pages and, once
//! archived, the purge.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{BoardClaim, CaseWorkspace, ObjectRef};
use pnex_core::ontology::{
    CASE_COPRESENCE_DISTANCE_M, CASE_COPRESENCE_MIN_S, CASE_LEGAL_BASIS, CASE_QUESTION,
    CASE_SIGNAL_GAP_S, CASE_STATUS, CASE_STOP_DIAMETER_M, CASE_STOP_MIN_S,
};
use pnex_core::resources::KIND_DOC;
use pnex_core::track::detect as defaults;
use serde_json::Value;

use super::pages::PagesPanel;
use super::purge::PurgePanel;
use super::snapshots::Snapshots;
use super::{model, Ws};
use crate::api;
use crate::components::ui::{BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};

#[component]
pub(super) fn Frame(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let case = workspace.case.clone();
    let text = |k: &str| {
        case.properties
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (question, basis) = (text(CASE_QUESTION), text(CASE_LEGAL_BASIS));
    let archived = text(CASE_STATUS) == "archived";
    let local = (ws.local_time)();
    let spans: Vec<(i64, i64)> = workspace
        .events
        .iter()
        .filter_map(|e| model::span(&e.event))
        .collect();
    let window = match (
        spans.iter().map(|s| s.0).min(),
        spans.iter().map(|s| s.1).max(),
    ) {
        (Some(a), Some(b)) => format!("{} → {}", model::stamp(a, local), model::stamp(b, local)),
        _ => t!("cw-frame-no-window").to_string(),
    };
    let pages: Vec<ObjectRef> = workspace
        .events
        .iter()
        .flat_map(|e| e.attachments.iter().map(|a| a.object.clone()))
        .chain(workspace.undated.iter().cloned())
        .filter(|o| o.type_key == KIND_DOC)
        .fold(Vec::new(), |mut acc: Vec<ObjectRef>, o| {
            if !acc.iter().any(|x| x.id == o.id) {
                acc.push(o);
            }
            acc
        });
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let save = {
        let case = case.clone();
        move |key: &'static str, value: String| {
            let mut props = case.properties.clone();
            props.insert(key.into(), Value::String(value));
            ws.save_object(case.id.clone(), case.title.clone(), props, case.version);
        }
    };
    let save_basis = save.clone();
    // A threshold left empty goes back to the detector default.
    let save_threshold = {
        let case = case.clone();
        move |key: &'static str, value: String| {
            let mut props = case.properties.clone();
            match value.trim().parse::<f64>() {
                Ok(n) => props.insert(key.into(), serde_json::json!(n)),
                Err(_) => props.remove(key),
            };
            ws.save_object_then(
                case.id.clone(),
                case.title.clone(),
                props,
                case.version,
                true,
            );
        }
    };
    let thresholds = [
        (
            CASE_COPRESENCE_DISTANCE_M,
            t!("cw-th-co-distance"),
            defaults::DEFAULT_CO_DISTANCE_M,
        ),
        (
            CASE_COPRESENCE_MIN_S,
            t!("cw-th-co-min"),
            defaults::DEFAULT_CO_MIN_S as f64,
        ),
        (
            CASE_STOP_DIAMETER_M,
            t!("cw-th-stop-diameter"),
            defaults::DEFAULT_STOP_DIAMETER_M,
        ),
        (
            CASE_STOP_MIN_S,
            t!("cw-th-stop-min"),
            defaults::DEFAULT_STOP_MIN_S as f64,
        ),
        (
            CASE_SIGNAL_GAP_S,
            t!("cw-th-gap"),
            defaults::DEFAULT_SIGNAL_GAP_S as f64,
        ),
    ];
    let close = move |_| {
        let mut f = ws.frame_open;
        f.set(false);
    };
    rsx! {
        div {
            class: "fixed right-4 top-16 z-40 flex max-h-[calc(100%-5rem)] w-[min(28rem,calc(100%-2rem))] flex-col gap-4 overflow-auto rounded-lg border border-line-strong bg-surface p-4 shadow-lg",
            role: "dialog",
            "aria-label": t!("cw-frame"),
            "data-testid": "case-frame-drawer",
            div { class: "flex items-center",
                h2 { class: "text-base font-semibold text-ink", {t!("cw-frame")} }
                button {
                    class: "{BTN_SECONDARY_SM} ml-auto",
                    r#type: "button",
                    onclick: close,
                    {t!("common-close")}
                }
            }
            div {
                label { class: FIELD_LABEL, r#for: "cw-question", {t!("case-question")} }
                textarea {
                    id: "cw-question",
                    class: "{INPUT} min-h-20",
                    disabled: !ws.can_write,
                    placeholder: t!("case-question-placeholder"),
                    initial_value: "{question}",
                    onchange: move |e| save(CASE_QUESTION, e.value()),
                }
            }
            div {
                span { class: FIELD_LABEL, {t!("cw-frame-window")} }
                p { class: "font-mono text-sm text-ink", "{window}" }
                p { class: FIELD_HELP, {t!("cw-frame-window-help")} }
            }
            div {
                label { class: FIELD_LABEL, r#for: "cw-basis", {t!("cw-legal-basis")} }
                input {
                    id: "cw-basis",
                    class: INPUT,
                    disabled: !ws.can_write,
                    maxlength: 255,
                    placeholder: t!("cw-legal-basis-placeholder"),
                    initial_value: "{basis}",
                    onchange: move |e| save_basis(CASE_LEGAL_BASIS, e.value()),
                }
                p { class: FIELD_HELP, {t!("cw-legal-basis-help")} }
            }
            div { "data-testid": "case-thresholds",
                h3 { class: "text-sm font-semibold text-ink", {t!("cw-thresholds")} }
                div { class: "mt-2 grid grid-cols-2 gap-2",
                    for (key, label, default) in thresholds {
                        label {
                            key: "{key}",
                            class: "flex flex-col justify-end gap-1 text-xs text-slate",
                            "{label}"
                            input {
                                class: INPUT,
                                r#type: "number",
                                min: "0",
                                disabled: !ws.can_write,
                                placeholder: "{default}",
                                initial_value: case.properties.get(key).map(|v| v.to_string()).unwrap_or_default(),
                                onchange: {
                                    let save_threshold = save_threshold.clone();
                                    move |e: FormEvent| save_threshold(key, e.value())
                                },
                            }
                        }
                    }
                }
                p { class: FIELD_HELP, {t!("cw-thresholds-help")} }
            }
            Snapshots { case_id: case.id.clone(), title: case.title.clone() }
            ReadLog { case_id: case.id.clone() }
            Board { case_id: case.id.clone(), version: case.version }
            PagesPanel {
                case: case.clone(),
                pages,
                editable: ws.can_write,
                on_changed: move |_| ws.reload(),
            }
            if archived && ws.can_write {
                PurgePanel {
                    case_id: case.id.clone(),
                    types: types.read().clone().unwrap_or_default(),
                    on_purged: move |_| ws.reload(),
                }
            }
        }
    }
}

fn claim_line(c: &BoardClaim) -> String {
    let status = c.status.clone().unwrap_or_default();
    format!(
        "{} · {} · {}",
        c.claim.title,
        status,
        t!("cw-board-sources", count : c.sources)
    )
}

/// Hypotheses with the claims for and against them, then the claims that
/// need attention. No score: the human concludes (cases.md K8).
#[component]
fn Board(case_id: String, version: i32) -> Element {
    let board = use_resource(use_reactive!(|case_id, version| async move {
        let _ = version;
        api::cases::board(&case_id).await.ok()
    }));
    let Some(Some(b)) = board.read().clone() else {
        return rsx! {};
    };
    if b.hypotheses.is_empty() && b.unsourced.is_empty() && b.contradicted.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "space-y-2", "data-testid": "case-board",
            h3 { class: "text-sm font-semibold text-ink", {t!("cw-board")} }
            for h in b.hypotheses.iter() {
                div {
                    key: "{h.hypothesis.id}",
                    class: "rounded-md border border-line p-2 text-sm",
                    p { class: "font-medium text-ink", "{h.hypothesis.title}" }
                    for c in h.supports.iter() {
                        p { key: "s{c.claim.id}", class: "text-xs text-ok", "+ {claim_line(c)}" }
                    }
                    for c in h.contradicts.iter() {
                        p { key: "c{c.claim.id}", class: "text-xs text-alarm",
                            "− {claim_line(c)}"
                        }
                    }
                }
            }
            if !b.unsourced.is_empty() {
                p { class: "text-xs font-medium text-slate", {t!("cw-board-unsourced")} }
                for c in b.unsourced.iter() {
                    p { key: "u{c.claim.id}", class: "text-xs text-ink", {claim_line(c)} }
                }
            }
            if !b.contradicted.is_empty() {
                p { class: "text-xs font-medium text-slate", {t!("cw-board-contradicted")} }
                for c in b.contradicted.iter() {
                    p { key: "x{c.claim.id}", class: "text-xs text-ink", {claim_line(c)} }
                }
            }
        }
    }
}

/// Who read persons' tracks, when, for what (K38): newest first.
#[component]
fn ReadLog(case_id: String) -> Element {
    let ws = use_context::<Ws>();
    let log = use_resource(use_reactive!(|case_id| async move {
        api::cases::reads(&case_id).await.ok().flatten()
    }));
    let local = (ws.local_time)();
    let stamp = |s: &str| {
        model::ms(s)
            .map(|t| model::stamp(t, local))
            .unwrap_or_default()
    };
    let rows = log.read().clone();
    rsx! {
        div { class: "space-y-1", "data-testid": "case-reads",
            h3 { class: "text-sm font-semibold text-ink", {t!("cw-reads")} }
            match rows {
                None | Some(None) => rsx! {
                    p { class: FIELD_HELP, {t!("cw-reads-unavailable")} }
                },
                Some(Some(r)) if r.is_empty() => rsx! {
                    p { class: FIELD_HELP, {t!("cw-reads-none")} }
                },
                Some(Some(r)) => rsx! {
                    for (i, e) in r.iter().take(20).enumerate() {
                        div { key: "{i}", class: "border-b border-line py-1 text-xs",
                            div { class: "text-ink",
                                "{stamp(&e.at)} · {e.who.clone().unwrap_or_default()} · {purpose_label(&e.purpose)}"
                            }
                            div { class: "text-slate",
                                {read_objects(e)}
                                " · {stamp(&e.from)} → {stamp(&e.to)}"
                            }
                        }
                    }
                },
            }
            p { class: FIELD_HELP, {t!("cw-reads-help")} }
        }
    }
}

fn purpose_label(p: &str) -> String {
    match p {
        "detect" => t!("cw-read-detect"),
        "snapshot" => t!("cw-read-snapshot"),
        _ => t!("cw-read-tracks"),
    }
    .to_string()
}

fn read_objects(e: &pnex_core::ontology::api::CaseRead) -> String {
    let mut names: Vec<String> = e.objects.iter().map(|o| o.title.clone()).collect();
    if e.gone > 0 {
        names.push(t!("cw-read-gone", count : e.gone).to_string());
    }
    names.join(", ")
}
