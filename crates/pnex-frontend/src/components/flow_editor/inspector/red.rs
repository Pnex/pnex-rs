use super::helpers::*;
use super::*;

/// ─────────────── red (échappement Node-RED) ───────────────

#[component]
pub(super) fn RedForm(
    mut cx: EditorCx,
    initial_type: String,
    initial_config: serde_json::Value,
    can_write: bool,
) -> Element {
    let mut type_name = use_signal(move || initial_type.clone());
    let mut config = use_signal(move || value_text(&initial_config));
    let mut config_invalid = use_signal(|| false);

    rsx! {
        div { class: "space-y-3",
            {text_field(t!("flows-red-type"), type_name, !can_write, move |event| {
                let raw = event.value();
                type_name.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Red { type_name, .. } = &mut node.kind {
                        *type_name = raw;
                    }
                });
            })}
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-red-config")} }
                textarea {
                    class: if config_invalid() {
                        "w-full h-28 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono"
                    } else {
                        "w-full h-28 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono"
                    },
                    value: "{config}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        match serde_json::from_str::<serde_json::Value>(&raw) {
                            Ok(value) => {
                                config_invalid.set(false);
                                config.set(raw.clone());
                                patch_selected(&mut cx, move |node: &mut FlowNode| {
                                    if let FlowNodeKind::Red { config, .. } = &mut node.kind {
                                        *config = value;
                                    }
                                });
                            }
                            Err(_) => {
                                config_invalid.set(true);
                                config.set(raw);
                            }
                        }
                    },
                }
                if config_invalid() {
                    span { class: "text-xs text-red-600 mt-1", {t!("flows-red-config-invalid")} }
                }
            }
        }
    }
}
