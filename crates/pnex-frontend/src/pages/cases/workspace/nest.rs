//! Nested cases (cases-graph-sigma.md GS16): the enclosing cases as a
//! breadcrumb, the sub-cases as links with their member count, and a new
//! sub-case in one click (no modal, D222: renamed in place once open).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{CaseOpenInput, CaseWorkspace};

use super::Ws;
use crate::api;
use crate::app::Route;
use crate::components::ui::BTN_GHOST;
use crate::state::toasts;

#[component]
pub(super) fn Nest(workspace: CaseWorkspace) -> Element {
    let ws = use_context::<Ws>();
    let navigator = use_navigator();
    let mut busy = use_signal(|| false);
    if workspace.parents.is_empty() && workspace.children.is_empty() && !ws.can_write {
        return rsx! {};
    }
    let add = move |_| {
        let input = CaseOpenInput {
            title: t!("cw-subcase-new-title").to_string(),
            parent_id: Some(ws.id()),
            ..Default::default()
        };
        busy.set(true);
        spawn(async move {
            let res = api::cases::open(&input).await;
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
        nav {
            class: "flex flex-wrap items-center gap-x-1 gap-y-1 border-b border-line pb-2 text-xs",
            "aria-label": t!("cw-subcases"),
            "data-testid": "case-nest",
            for p in workspace.parents.iter().rev() {
                Link {
                    key: "{p.id}",
                    class: "text-signal hover:underline",
                    to: Route::CaseWorkspace {
                        id: p.id.clone(),
                    },
                    "{p.title}"
                }
                span { class: "text-slate", "aria-hidden": "true", "›" }
            }
            if !workspace.parents.is_empty() {
                span { class: "font-semibold text-ink", "{workspace.case.title}" }
            }
            for c in workspace.children.iter() {
                Link {
                    key: "{c.case.id}",
                    class: "rounded-full border border-line-strong px-2 py-0.5 text-ink hover:border-signal",
                    to: Route::CaseWorkspace {
                        id: c.case.id.clone(),
                    },
                    "{c.case.title} · {c.members}"
                }
            }
            if ws.can_write {
                button {
                    class: BTN_GHOST,
                    r#type: "button",
                    title: t!("cw-subcase-add-help"),
                    disabled: busy(),
                    "data-testid": "case-subcase-add",
                    onclick: add,
                    {t!("cw-subcase-add")}
                }
            }
        }
    }
}
