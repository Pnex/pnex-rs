//! « Sites à valider » (web-presence.md D366): the websites a flow proposed
//! for an organization, with what proved them; a member confirms or
//! rejects each one (a rejected site is never proposed again).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::web_visit::{SiteCandidate, SiteDecisionKind};

use crate::api;
use crate::api::error_i18n::localize;
use crate::app::Route;
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_ERROR};
use crate::state::access::can;

#[component]
pub fn SiteCandidates() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut error = use_signal(|| None::<String>);
    let list = use_resource(move || async move {
        let _ = reload();
        api::web_presence::candidates().await
    });
    let can_write = can("ontology:write");
    let rows: Vec<SiteCandidate> = match &*list.read() {
        Some(Ok(r)) => r.clone(),
        _ => Vec::new(),
    };
    if rows.is_empty() {
        return rsx! {};
    }
    let decide = move |id: i64, d: SiteDecisionKind| {
        spawn(async move {
            match api::web_presence::decide(id, d).await {
                Ok(_) => reload.with_mut(|n| *n += 1),
                Err(e) => error.set(Some(localize(&e))),
            }
        });
    };
    rsx! {
        section {
            class: "mb-4 rounded-lg border border-amber-200 bg-amber-50 p-3 space-y-2",
            "data-testid": "site-candidates",
            h2 { class: "text-sm font-semibold text-ink",
                {t!("site-candidates-title", count : rows.len())}
            }
            p { class: "text-xs text-slate", {t!("site-candidates-help")} }
            if let Some(m) = error() {
                p { class: FIELD_ERROR, "{m}" }
            }
            ul { class: "divide-y divide-amber-200",
                for c in rows {
                    li {
                        key: "{c.link_id}",
                        class: "flex flex-wrap items-center gap-2 py-2 text-sm",
                        Link {
                            class: "font-medium text-signal-700 hover:underline",
                            to: Route::OntologyObject {
                                id: c.organization_id.clone(),
                            },
                            "{c.organization}"
                        }
                        span { class: "text-slate", "→" }
                        // Shown as text: an unknown site is never opened from here.
                        span { class: "font-mono text-xs text-ink", "{c.domain}" }
                        if let Some(s) = c.score {
                            span { class: "rounded border border-line px-1.5 text-xs text-slate",
                                {t!("site-candidates-score", score : s)}
                            }
                        }
                        if c.proof_siren {
                            span { class: "rounded bg-green-100 px-1.5 text-xs text-green-800",
                                {t!("site-candidates-proof-siren")}
                            }
                        }
                        if c.proof_nda {
                            span { class: "rounded bg-green-100 px-1.5 text-xs text-green-800",
                                {t!("site-candidates-proof-nda")}
                            }
                        }
                        if c.source == "email" {
                            span { class: "text-xs text-slate", {t!("site-candidates-from-email")} }
                        }
                        if can_write {
                            span { class: "ml-auto flex gap-2",
                                button {
                                    class: BTN_PRIMARY_SM,
                                    onclick: move |_| decide(c.link_id, SiteDecisionKind::Confirme),
                                    {t!("site-candidates-confirm")}
                                }
                                button {
                                    class: BTN_SECONDARY_SM,
                                    onclick: move |_| decide(c.link_id, SiteDecisionKind::Rejete),
                                    {t!("site-candidates-reject")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
