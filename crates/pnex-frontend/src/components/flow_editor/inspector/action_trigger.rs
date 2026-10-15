use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::ontology::action::ActionTriggerConfig;

// ─────────────── action-trigger (ontology.md D183) ───────────────

/// Action trigger inspector: one `type · action` pick among the actions
/// declared in the org ontology.
#[component]
pub(super) fn ActionTriggerForm(
    mut cx: EditorCx,
    initial: ActionTriggerConfig,
    can_write: bool,
) -> Element {
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let mut picked = use_signal(move || format!("{}/{}", initial.object_type, initial.action));
    // (value, label) of every declared action, `type/action`.
    let choices: Vec<(String, String)> = types
        .read()
        .clone()
        .unwrap_or_default()
        .iter()
        .flat_map(|t| {
            let kind = crate::components::ontology::type_label(&t.def);
            t.def.actions.iter().map(move |a| {
                (
                    format!("{}/{}", t.def.key, a.key),
                    format!(
                        "{kind} · {}",
                        crate::components::ontology::tr(&a.name, &a.names)
                    ),
                )
            })
        })
        .collect();
    let loaded = types.read().is_some();
    let empty = loaded && choices.is_empty();
    let on_pick = move |e: Event<FormData>| {
        let v = e.value();
        picked.set(v.clone());
        let (object_type, action) = v.split_once('/').unwrap_or_default();
        let next = ActionTriggerConfig {
            object_type: object_type.to_string(),
            action: action.to_string(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::ActionTrigger { config } = &mut node.kind {
                *config = next;
            }
        });
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-action-trigger-help")} }
            label { class: FIELD_LABEL, {t!("flows-action-trigger-action")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                "data-testid": "action-trigger-pick",
                disabled: !can_write,
                onchange: on_pick,
                option { value: "", selected: picked().len() <= 1, "—" }
                for (value, label) in choices {
                    option {
                        key: "{value}",
                        selected: value == picked(),
                        value: "{value}",
                        "{label}"
                    }
                }
            }
            if empty {
                p { class: "text-xs text-amber-700", {t!("flows-action-trigger-none")} }
            }
        }
    }
}
