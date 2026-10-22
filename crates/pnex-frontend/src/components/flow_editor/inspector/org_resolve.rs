use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::org_resolve::{OrgResolveConfig, CANDIDATES_MAX};

// ─────────────── company name → SIREN (account-mapping.md D332) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Company lookup inspector: the score of a match, the lead it needs over
/// the next candidate, and how many candidates come back.
#[component]
pub(super) fn OrgResolveForm(
    mut cx: EditorCx,
    initial: OrgResolveConfig,
    can_write: bool,
) -> Element {
    let mut config = use_signal(move || initial.clone());
    let current = config();
    let mut set = move |next: OrgResolveConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::OrgResolve { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let num = |e: &Event<FormData>| e.value().trim().parse().unwrap_or(0);
    let on_threshold = move |e: Event<FormData>| {
        set(OrgResolveConfig {
            threshold: num(&e),
            ..config()
        })
    };
    let on_gap = move |e: Event<FormData>| {
        set(OrgResolveConfig {
            gap: num(&e),
            ..config()
        })
    };
    let on_limit = move |e: Event<FormData>| {
        set(OrgResolveConfig {
            limit: num(&e),
            ..config()
        })
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-org-resolve-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-org-resolve-threshold")} }
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "0",
                    max: "100",
                    disabled: !can_write,
                    value: "{current.threshold}",
                    oninput: on_threshold,
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-org-resolve-gap")} }
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "0",
                    max: "100",
                    disabled: !can_write,
                    value: "{current.gap}",
                    oninput: on_gap,
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-org-resolve-limit")} }
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "1",
                    max: "{CANDIDATES_MAX}",
                    disabled: !can_write,
                    value: "{current.limit}",
                    oninput: on_limit,
                }
            }
        }
    }
}
