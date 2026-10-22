//! Proposals table (proposals.md D374): what a machine proposed, about
//! which objects, with which values, from where; accept or reject. Used by
//! the « À valider » page and, filtered, on an object's page.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::page_render::text;
use pnex_core::ontology::proposal::{DecisionKind, ProposalKind, ProposalStatus, ProposalView};

use crate::api;
use crate::app::Route;
use crate::components::crud::form::FormDialog;
use crate::components::crud::pager::ListPager;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access::can;
use crate::state::toasts;

/// Translated label of a dynamic key (`proposals-kind-…`, `proposals-status-…`),
/// else the raw value.
pub fn label(prefix: &str, value: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("{prefix}-{value}"))
        .unwrap_or_else(|_| value.to_string())
}

/// `object_id`: only the proposals about it; `hide_empty`: render nothing
/// when there is none (object pages).
#[component]
pub fn ProposalsTable(
    #[props(default)] object_id: Option<String>,
    status: ProposalStatus,
    #[props(default)] hide_empty: bool,
) -> Element {
    let page = use_signal(|| 0i64);
    let mut reload = use_signal(|| 0u32);
    let object = object_id.clone();
    let rows = use_resource(move || {
        let object = object.clone();
        let (p, _) = (page(), reload());
        async move { api::proposals::page(status.as_str(), object.as_deref(), p).await }
    });
    let can_decide = can("proposals:decide") && status == ProposalStatus::Proposed;
    let (state, list, count) = match &*rows.read() {
        None => (None, Vec::new(), 0),
        Some(Ok(p)) => (Some(Ok(())), p.results.clone(), p.count),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new(), 0),
    };
    if hide_empty && count == 0 {
        return rsx! {};
    }
    let decide = move |id: String, d: DecisionKind| {
        spawn(async move {
            match api::proposals::decide(&id, d, None, None).await {
                Ok(_) => reload.with_mut(|n| *n += 1),
                Err(e) => toasts::error(e),
            }
        });
    };
    // Reject with a reason, or accept with corrected values.
    let mut dialog = use_signal(|| None::<(ProposalView, DecisionKind)>);
    let mut columns = vec![
        Column::new(t!("proposals-col-what").to_string(), |p: &ProposalView| {
            let kind = label("proposals-kind", p.kind.as_str());
            rsx! {
                span { class: "font-medium text-ink", "{kind}" }
                span { class: "ml-1 font-mono text-xs text-slate", "{p.type_key}" }
            }
        }),
        Column::new(
            t!("proposals-col-objects").to_string(),
            |p: &ProposalView| {
                rsx! {
                    span { class: "flex flex-wrap gap-x-2",
                        for o in p.refs.iter() {
                            Link {
                                key: "{o.id}",
                                class: "text-signal-700 hover:underline",
                                to: Route::OntologyObject { id: o.id.clone() },
                                "{o.title}"
                            }
                        }
                        if let Some(t) = p.title.clone() {
                            span { class: "text-ink", "{t}" }
                        }
                    }
                }
            },
        ),
        Column::new(
            t!("proposals-col-values").to_string(),
            |p: &ProposalView| {
                let pairs: Vec<String> = shown_values(p)
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.replace('_', " "), text(v)))
                    .collect();
                // Evidence is untrusted text (a site, a model): shown as text only.
                let evidence: Vec<String> = p
                    .evidence
                    .as_object()
                    .map(|m| {
                        m.iter()
                            .map(|(k, v)| format!("{}: {}", k.replace('_', " "), text(v)))
                            .collect()
                    })
                    .unwrap_or_default();
                rsx! {
                    span { class: "text-xs text-slate", "{pairs.join(\" · \")}" }
                    for (i, e) in evidence.into_iter().enumerate() {
                        p { key: "{i}", class: "mt-1 text-xs italic text-slate", "{e}" }
                    }
                }
            },
        ),
        Column::new(
            t!("proposals-col-origin").to_string(),
            |p: &ProposalView| {
                let confidence = p
                    .confidence
                    .map(|c| format!(" · {c:.0}"))
                    .unwrap_or_default();
                rsx! {
                    span { class: "font-mono text-xs text-slate", "{p.origin}{confidence}" }
                }
            },
        ),
    ];
    if can_decide {
        columns.push(
            Column::new(
                t!("proposals-col-decision").to_string(),
                move |p: &ProposalView| {
                    let a = p.id.clone();
                    let (fix, rej) = (p.clone(), p.clone());
                    // A voice is decided where it can be heard.
                    if p.kind == ProposalKind::VoiceSpan {
                        let id = p
                            .values
                            .get("media_asset_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        return rsx! {
                            Link {
                                class: "text-signal-700 hover:underline",
                                to: Route::MediaReview { id },
                                {t!("proposals-listen")}
                            }
                        };
                    }
                    rsx! {
                        span { class: "flex gap-2",
                            button {
                                class: BTN_PRIMARY_SM,
                                r#type: "button",
                                onclick: move |_| decide(a.clone(), DecisionKind::Accept),
                                {t!("proposals-accept")}
                            }
                            button {
                                class: BTN_SECONDARY_SM,
                                r#type: "button",
                                onclick: move |_| dialog.set(Some((fix.clone(), DecisionKind::Accept))),
                                {t!("proposals-correct")}
                            }
                            button {
                                class: BTN_SECONDARY_SM,
                                r#type: "button",
                                onclick: move |_| dialog.set(Some((rej.clone(), DecisionKind::Reject))),
                                {t!("proposals-reject")}
                            }
                        }
                    }
                },
            )
            .header_sr_only(),
        );
    } else {
        columns.push(Column::new(
            t!("proposals-col-decided").to_string(),
            |p: &ProposalView| {
                // No user and a decision: accepted by an org rule (D377).
                let by = match (&p.decided_by, &p.decided_at) {
                    (Some(name), _) => name.clone(),
                    (None, Some(_)) => {
                        t!("proposals-decided-auto", rule : p.reason.clone().unwrap_or_default())
                            .to_string()
                    }
                    (None, None) => String::new(),
                };
                rsx! {
                    span { class: "text-xs text-slate", "{by}" }
                }
            },
        ));
    }
    rsx! {
        div { "data-testid": "proposals",
            if hide_empty {
                h2 { class: "mb-2 text-sm font-semibold text-ink",
                    {t!("proposals-on-object", count : count)}
                }
            }
            ListStates {
                state,
                is_empty: count == 0,
                empty_message: t!("proposals-empty").to_string(),
                DataTable {
                    columns,
                    rows: list,
                    row_key: RowKey::new(|p: &ProposalView| p.id.clone()),
                }
                ListPager { count, page }
            }
            if let Some((p, d)) = dialog() {
                DecisionDialog {
                    proposal: p,
                    decision: d,
                    on_close: move |_| dialog.set(None),
                    on_done: move |_| {
                        dialog.set(None);
                        reload.with_mut(|n| *n += 1);
                    },
                }
            }
        }
    }
}

/// Reject with a reason, or accept with corrected text values (numbers and
/// booleans stay as proposed).
#[component]
fn DecisionDialog(
    proposal: ProposalView,
    decision: DecisionKind,
    on_close: EventHandler<()>,
    on_done: EventHandler<()>,
) -> Element {
    let mut reason = use_signal(String::new);
    let initial = editable(&proposal);
    let mut values = use_signal(move || initial.clone());
    let mut busy = use_signal(|| false);
    let id = proposal.id.clone();
    let submit = move |_| {
        let id = id.clone();
        let reject = decision == DecisionKind::Reject;
        let r = Some(reason()).filter(|r| !r.trim().is_empty());
        let v = (!reject).then(|| values());
        busy.set(true);
        spawn(async move {
            match api::proposals::decide(&id, decision, r, v).await {
                Ok(_) => on_done.call(()),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let reject = decision == DecisionKind::Reject;
    let fields: Vec<(String, String)> = editable(&proposal)
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    rsx! {
        FormDialog {
            title: if reject { t!("proposals-reject").to_string() } else { t!("proposals-correct").to_string() },
            submit_label: if reject { t!("proposals-reject").to_string() } else { t!("proposals-accept").to_string() },
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: true,
            div { class: "space-y-3",
                if reject {
                    label { class: FIELD_LABEL, {t!("proposals-reason")} }
                    textarea {
                        class: INPUT,
                        rows: "3",
                        maxlength: "{pnex_core::ontology::proposal::REASON_MAX}",
                        value: "{reason}",
                        oninput: move |e| reason.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("proposals-reason-help")} }
                } else {
                    for (k, v) in fields {
                        label { key: "{k}", class: "block",
                            span { class: FIELD_LABEL, "{k.replace('_', \" \")}" }
                            input {
                                class: INPUT,
                                value: "{v}",
                                oninput: {
                                    let k = k.clone();
                                    move |e: FormEvent| values.with_mut(|m| set_value(m, &k, e.value()))
                                },
                            }
                        }
                    }
                    p { class: FIELD_HELP, {t!("proposals-correct-help")} }
                }
            }
        }
    }
}

/// What the values column shows: a case detection by its detector and span
/// (its key and members are internal; the members are in Objects).
fn shown_values(p: &ProposalView) -> serde_json::Map<String, serde_json::Value> {
    if p.kind == ProposalKind::VoiceSpan {
        let clock = |k: &str| {
            let s = p
                .values
                .get(k)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
                / 1_000;
            format!("{}:{:02}:{:02}", s / 3_600, s / 60 % 60, s % 60)
        };
        let mut out = serde_json::Map::new();
        out.insert("from".into(), clock("start_ms").into());
        out.insert("to".into(), clock("end_ms").into());
        return out;
    }
    if p.kind != ProposalKind::CaseEvent {
        return p.values.clone();
    }
    let at = |k: &str| {
        p.values
            .get(k)
            .and_then(serde_json::Value::as_i64)
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
    };
    let mut out = serde_json::Map::new();
    for (k, v) in [
        ("detector", p.values.get("detector").cloned()),
        ("from", at("from_ms").map(Into::into)),
        ("to", at("to_ms").map(Into::into)),
    ] {
        if let Some(v) = v {
            out.insert(k.into(), v);
        }
    }
    out
}

/// What "Correct and accept" edits: the title of a case detection's event,
/// else the proposed values.
fn editable(p: &ProposalView) -> serde_json::Map<String, serde_json::Value> {
    if p.kind != ProposalKind::CaseEvent {
        return p.values.clone();
    }
    let mut out = serde_json::Map::new();
    out.insert("title".into(), p.title.clone().unwrap_or_default().into());
    out
}

fn set_value(values: &mut serde_json::Map<String, serde_json::Value>, key: &str, value: String) {
    values.insert(key.to_string(), serde_json::Value::String(value));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_case_detection_shows_its_span_and_edits_its_title() {
        let mut p: ProposalView = serde_json::from_value(json!({
            "id": "p", "kind": "case_event", "status": "proposed", "type_key": "event", "refs": [],
            "title": "stop: T-07", "values": {"key": "k", "detector": "stop@1", "members": ["m"],
            "from_ms": 0, "to_ms": 60_000}, "evidence": null, "origin": "detector:stop@1",
            "created_at": "2026-10-23T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(
            serde_json::Value::Object(shown_values(&p)),
            json!({"detector": "stop@1", "from": "1970-01-01 00:00 UTC", "to": "1970-01-01 00:01 UTC"})
        );
        assert_eq!(
            serde_json::Value::Object(editable(&p)),
            json!({"title": "stop: T-07"})
        );
        p.kind = ProposalKind::LinkCreate;
        assert_eq!(shown_values(&p), p.values);
        p.kind = ProposalKind::VoiceSpan;
        p.values = json!({"start_ms": 3_725_000, "end_ms": 3_790_500})
            .as_object()
            .cloned()
            .unwrap();
        assert_eq!(
            serde_json::Value::Object(shown_values(&p)),
            json!({"from": "1:02:05", "to": "1:03:10"})
        );
    }
}
