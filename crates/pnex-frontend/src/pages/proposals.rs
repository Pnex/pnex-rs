//! "To review" (proposals.md D374): every proposal of the org, by status.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::proposal::ProposalStatus;

use crate::components::crud::filters::FilterBar;
use crate::components::crud::layout::ListLayout;
use crate::components::proposals_table::{label, ProposalsTable};
use crate::components::ui::INPUT;

const STATUSES: [ProposalStatus; 4] = [
    ProposalStatus::Proposed,
    ProposalStatus::Accepted,
    ProposalStatus::Rejected,
    ProposalStatus::Superseded,
];

/// `origin`: only the proposals of this origin prefix, such as `agent:<id>` from a run
/// (D785); empty for all.
#[component]
pub fn Proposals(origin: String) -> Element {
    let origin = (!origin.trim().is_empty()).then_some(origin);
    let mut status = use_signal(|| ProposalStatus::Proposed);
    rsx! {
        ListLayout {
            title: t!("proposals-title").to_string(),
            subtitle: Some(t!("proposals-subtitle").to_string()),
            can_write: false,
            filters: rsx! {
                FilterBar {
                    select {
                        class: INPUT,
                        "aria-label": t!("proposals-status"),
                        onchange: move |e| {
                            if let Some(s) = ProposalStatus::parse(&e.value()) {
                                status.set(s);
                            }
                        },
                        for s in STATUSES {
                            option {
                                key: "{s.as_str()}",
                                value: "{s.as_str()}",
                                selected: status() == s,
                                {label("proposals-status", s.as_str())}
                            }
                        }
                    }
                    if let Some(o) = origin.clone() {
                        span { class: "text-sm text-slate", {t!("proposals-origin-filter", origin : o)} }
                    }
                }
            },
            ProposalsTable {
                key: "{status().as_str()}",
                status: status(),
                origin: origin.clone(),
            }
        }
    }
}
