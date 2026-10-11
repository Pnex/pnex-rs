//! "Open a case" on a dated event (cases.md K3): one click creates the case
//! with its window around the event and the flow that raised it, then opens
//! the case workspace.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseFrom, CaseOpenInput};

use crate::api;
use crate::app::Route;
use crate::components::ui::BTN_SECONDARY_SM;
use crate::state::{org, toasts};

/// `kind`: `notification` | `event`; `title`: the event's text, or empty
/// for the generic case title.
#[component]
pub fn OpenCaseButton(kind: String, at_us: i64, flow_id: Option<i64>, title: String) -> Element {
    let mut busy = use_signal(|| false);
    let navigator = use_navigator();
    if !org::current_can_write() {
        return rsx! {};
    }
    let at = chrono::DateTime::from_timestamp_micros(at_us)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default();
    let title = Some(title.trim().chars().take(200).collect::<String>())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| t!("case-from-event-title").to_string());
    let open = move |e: MouseEvent| {
        e.stop_propagation();
        let input = CaseOpenInput {
            title: title.clone(),
            from: Some(CaseFrom {
                kind: kind.clone(),
                at: at.clone(),
                flow_id,
                margin_minutes: None,
            }),
        };
        busy.set(true);
        spawn(async move {
            let res = api::ontology::open_case(&input).await;
            busy.set(false);
            match res {
                Ok(case) => {
                    navigator.push(Route::CaseWorkspace { id: case.id });
                }
                Err(err) => toasts::error(err),
            }
        });
    };
    rsx! {
        button {
            r#type: "button",
            class: BTN_SECONDARY_SM,
            "data-testid": "open-case",
            disabled: busy(),
            onclick: open,
            {t!("case-open-from-event")}
        }
    }
}
