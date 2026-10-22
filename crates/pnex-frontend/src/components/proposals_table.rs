//! Proposals table (proposals.md D374): what a machine proposed, about
//! which objects, with which values, from where; accept or reject. Used by
//! the « À valider » page and, filtered, on an object's page.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::page_render::text;
use pnex_core::ontology::proposal::{DecisionKind, ProposalStatus, ProposalView};

use crate::api;
use crate::app::Route;
use crate::components::crud::pager::ListPager;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM};
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
            match api::proposals::decide(&id, d).await {
                Ok(_) => reload.with_mut(|n| *n += 1),
                Err(e) => toasts::error(e),
            }
        });
    };
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
                let pairs: Vec<String> = p
                    .values
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.replace('_', " "), text(v)))
                    .collect();
                rsx! {
                    span { class: "text-xs text-slate", "{pairs.join(\" · \")}" }
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
                    let (a, r) = (p.id.clone(), p.id.clone());
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
                                onclick: move |_| decide(r.clone(), DecisionKind::Reject),
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
                let by = p.decided_by.clone().unwrap_or_default();
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
        }
    }
}
