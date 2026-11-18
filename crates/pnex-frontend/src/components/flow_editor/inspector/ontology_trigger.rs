use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::ontology::trigger::{OntologyTriggerConfig, ON_LINK, ON_OBJECT, ON_PROPOSAL};

// ─────────────── ontology-trigger (operational-platform.md D998) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Comma-separated keys, trimmed, empties dropped.
fn keys_of(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(String::from)
        .collect()
}

/// Ontology trigger inspector: the type, the kinds of events, and the lists
/// that narrow each kind, typed as comma-separated keys.
#[component]
pub(super) fn OntologyTriggerForm(
    mut cx: EditorCx,
    initial: OntologyTriggerConfig,
    can_write: bool,
) -> Element {
    let mut object_type = use_signal(|| initial.object_type.clone());
    let mut on = use_signal(|| initial.on.clone());
    let mut properties = use_signal(|| initial.properties.join(", "));
    let mut link_types = use_signal(|| initial.link_types.join(", "));
    let mut origin = use_signal(|| initial.origin.clone());
    let mut skip_own = use_signal(|| initial.skip_own);
    let mut save = move || {
        let next = OntologyTriggerConfig {
            object_type: object_type().trim().to_string(),
            on: on(),
            properties: keys_of(&properties()),
            link_types: keys_of(&link_types()),
            origin: origin().trim().to_string(),
            skip_own: skip_own(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::OntologyTrigger { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let mut toggle = move |kind: &'static str| {
        let mut list = on();
        if let Some(i) = list.iter().position(|k| k == kind) {
            list.remove(i);
        } else {
            list.push(kind.to_string());
        }
        on.set(list);
        save();
    };
    let wants = move |kind: &str| {
        let list = on();
        list.iter().any(|k| k == kind) || (list.is_empty() && kind == ON_OBJECT)
    };

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-ontology-trigger-help")} }
            label { class: FIELD_LABEL, r#for: "ontology-trigger-type",
                {t!("flows-ontology-trigger-type")}
            }
            input {
                id: "ontology-trigger-type",
                class: INPUT,
                "data-testid": "ontology-trigger-type",
                disabled: !can_write,
                value: "{object_type}",
                oninput: move |e| object_type.set(e.value()),
                onchange: move |_| save(),
            }
            p { class: FIELD_LABEL, {t!("flows-ontology-trigger-on")} }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    "data-testid": "ontology-trigger-on-object",
                    disabled: !can_write,
                    checked: wants(ON_OBJECT),
                    onchange: move |_| toggle(ON_OBJECT),
                }
                {t!("flows-ontology-trigger-on-object")}
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    "data-testid": "ontology-trigger-on-link",
                    disabled: !can_write,
                    checked: wants(ON_LINK),
                    onchange: move |_| toggle(ON_LINK),
                }
                {t!("flows-ontology-trigger-on-link")}
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    "data-testid": "ontology-trigger-on-proposal",
                    disabled: !can_write,
                    checked: wants(ON_PROPOSAL),
                    onchange: move |_| toggle(ON_PROPOSAL),
                }
                {t!("flows-ontology-trigger-on-proposal")}
            }
            label { class: FIELD_LABEL, r#for: "ontology-trigger-properties",
                {t!("flows-ontology-trigger-properties")}
            }
            input {
                id: "ontology-trigger-properties",
                class: INPUT,
                "data-testid": "ontology-trigger-properties",
                disabled: !can_write,
                value: "{properties}",
                oninput: move |e| properties.set(e.value()),
                onchange: move |_| save(),
            }
            label { class: FIELD_LABEL, r#for: "ontology-trigger-links",
                {t!("flows-ontology-trigger-links")}
            }
            input {
                id: "ontology-trigger-links",
                class: INPUT,
                "data-testid": "ontology-trigger-links",
                disabled: !can_write,
                value: "{link_types}",
                oninput: move |e| link_types.set(e.value()),
                onchange: move |_| save(),
            }
            label { class: FIELD_LABEL, r#for: "ontology-trigger-origin",
                {t!("flows-ontology-trigger-origin")}
            }
            input {
                id: "ontology-trigger-origin",
                class: INPUT,
                "data-testid": "ontology-trigger-origin",
                disabled: !can_write,
                value: "{origin}",
                oninput: move |e| origin.set(e.value()),
                onchange: move |_| save(),
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    "data-testid": "ontology-trigger-skip-own",
                    disabled: !can_write,
                    checked: skip_own(),
                    onchange: move |e| {
                        skip_own.set(e.checked());
                        save();
                    },
                }
                {t!("flows-ontology-trigger-skip-own")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::keys_of;

    #[test]
    fn keys_are_split_and_trimmed() {
        assert_eq!(keys_of(" a, b ,,c "), vec!["a", "b", "c"]);
        assert!(keys_of("  ").is_empty());
    }
}
