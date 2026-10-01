use super::helpers::*;
use super::*;

// ─────────────── metric ───────────────

#[component]
pub(super) fn MetricForm(
    mut cx: EditorCx,
    initial: MetricConfig,
    can_write: bool,
    flow_id: i64,
) -> Element {
    let mut name = use_signal(move || initial.metric_name.clone());
    let preview = use_memo(move || {
        let raw = name.cloned();
        if raw.trim().is_empty() {
            String::new()
        } else {
            pnex_core::etl_metric_name(&raw)
        }
    });

    rsx! {
        div { class: "space-y-3",
            {text_field(t!("flows-metric-name"), name, !can_write, move |event| {
                let raw = event.value();
                name.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Metric { config } = &mut node.kind {
                        config.metric_name = raw;
                    }
                });
            })}
            if !preview.cloned().is_empty() {
                p { class: "text-xs text-gray-500",
                    span { class: "font-medium", {t!("flows-metric-preview")} }
                    code { class: "ml-1 px-1 rounded bg-gray-100", {preview.cloned()} }
                }
            }
            p { class: "text-xs text-gray-400", {t!("flows-metric-labels-help", id: flow_id)} }
        }
    }
}

/// ─────────────── debug ───────────────

#[component]
pub(super) fn DebugForm(mut cx: EditorCx, initial: DebugConfig, can_write: bool) -> Element {
    // Champs booléens copiés avant le move d'`initial` dans les signaux.
    let active = initial.active;
    let console = initial.console;
    let mut complete = use_signal(move || initial.complete.clone().unwrap_or_default());

    rsx! {
        div { class: "space-y-3",
            label { class: "flex items-center gap-2 select-none",
                input {
                    class: "h-4 w-4 accent-blue-600",
                    r#type: "checkbox",
                    checked: active,
                    disabled: !can_write,
                    onchange: move |event| {
                        let checked = event.checked();
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::Debug { config } = &mut node.kind {
                                config.active = checked;
                            }
                        });
                    },
                }
                span { class: "text-xs font-medium text-gray-500", {t!("flows-debug-active")} }
            }
            {text_field(t!("flows-debug-complete"), complete, !can_write, move |event| {
                let raw = event.value();
                complete.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Debug { config } = &mut node.kind {
                        config.complete = Some(raw).filter(|c| !c.is_empty());
                    }
                });
            })}
            label { class: "flex items-center gap-2 select-none",
                input {
                    class: "h-4 w-4 accent-blue-600",
                    r#type: "checkbox",
                    checked: console,
                    disabled: !can_write,
                    onchange: move |event| {
                        let checked = event.checked();
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::Debug { config } = &mut node.kind {
                                config.console = checked;
                            }
                        });
                    },
                }
                span { class: "text-xs font-medium text-gray-500", {t!("flows-debug-console")} }
            }
        }
    }
}

/// ─────────────── display (sonde) ───────────────

#[component]
pub(super) fn DisplayForm(can_write: bool) -> Element {
    // Aucune config saisie : l'identité est estampillée par la projection au
    // deploy (pnex_node_id/flow/version) — le formulaire est informatif.
    let _ = can_write;
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-display-hint")} }
        }
    }
}

// ─────────────── json split / json merge ───────────────
