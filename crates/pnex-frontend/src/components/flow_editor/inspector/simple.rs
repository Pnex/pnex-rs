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
    // Label rows kept in entry order; the graph stores them as a map.
    let mut label_rows = use_signal(move || {
        let rows: Vec<(String, String)> = initial
            .labels
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        rows
    });
    let mut sync_labels = move |rows: Vec<(String, String)>| {
        label_rows.set(rows.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::Metric { config } = &mut node.kind {
                config.labels = rows
                    .into_iter()
                    .filter(|(k, _)| !k.trim().is_empty())
                    .collect();
            }
        });
    };
    let can_add_label = can_write && label_rows.read().len() < pnex_core::SERIES_LABELS_MAX;
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
            {
                text_field(
                    t!("flows-metric-name"),
                    name,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        name.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Metric { config } = &mut node.kind {
                                    config.metric_name = raw;
                                }
                            },
                        );
                    },
                )
            }
            if !preview.cloned().is_empty() {
                p { class: "text-xs text-gray-500",
                    span { class: "font-medium", {t!("flows-metric-preview")} }
                    code { class: "ml-1 px-1 rounded bg-gray-100", {preview.cloned()} }
                }
            }
            p { class: "text-xs text-gray-400", {t!("flows-metric-labels-help", id : flow_id)} }
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-metric-labels")} }
                for (index, (label, source)) in label_rows.read().iter().enumerate() {
                    MetricLabelRow {
                        key: "{index}",
                        index,
                        name: label.clone(),
                        source: source.clone(),
                        can_write,
                        on_change: move |(i, new_name, new_source): (usize, String, String)| {
                            let mut rows = label_rows.read().clone();
                            if let Some(row) = rows.get_mut(i) {
                                *row = (new_name, new_source);
                            }
                            sync_labels(rows);
                        },
                        on_remove: move |i: usize| {
                            let mut rows = label_rows.read().clone();
                            if i < rows.len() {
                                rows.remove(i);
                            }
                            sync_labels(rows);
                        },
                    }
                }
                if can_add_label {
                    button {
                        class: "text-xs text-blue-600 hover:text-blue-700",
                        onclick: move |_| {
                            let mut rows = label_rows.read().clone();
                            rows.push((String::new(), String::new()));
                            sync_labels(rows);
                        },
                        {t!("flows-metric-label-add")}
                    }
                }
                p { class: "text-xs text-gray-400", {t!("flows-metric-label-sources-help")} }
            }
        }
    }
}

/// One label row of the metric node: name + source, removable.
#[component]
fn MetricLabelRow(
    index: usize,
    name: String,
    source: String,
    can_write: bool,
    on_change: EventHandler<(usize, String, String)>,
    on_remove: EventHandler<usize>,
) -> Element {
    let name_edit = name.clone();
    let source_edit = source.clone();
    rsx! {
        div { class: "flex gap-1 items-center",
            input {
                class: "w-28 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: t!("flows-metric-label-name"),
                value: "{name}",
                disabled: !can_write,
                oninput: move |event| {
                    let new_name = event.value();
                    on_change.call((index, new_name, source_edit.clone()));
                },
            }
            input {
                class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: t!("flows-metric-label-source"),
                value: "{source}",
                disabled: !can_write,
                oninput: move |event| {
                    let new_source = event.value();
                    on_change.call((index, name_edit.clone(), new_source));
                },
            }
            if can_write {
                button {
                    class: "text-xs text-red-500 hover:text-red-700",
                    onclick: move |_| on_remove.call(index),
                    "✕"
                }
            }
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
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Debug { config } = &mut node.kind {
                                    config.active = checked;
                                }
                            },
                        );
                    },
                }
                span { class: "text-xs font-medium text-gray-500", {t!("flows-debug-active")} }
            }
            {
                text_field(
                    t!("flows-debug-complete"),
                    complete,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        complete.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Debug { config } = &mut node.kind {
                                    config.complete = Some(raw).filter(|c| !c.is_empty());
                                }
                            },
                        );
                    },
                )
            }
            label { class: "flex items-center gap-2 select-none",
                input {
                    class: "h-4 w-4 accent-blue-600",
                    r#type: "checkbox",
                    checked: console,
                    disabled: !can_write,
                    onchange: move |event| {
                        let checked = event.checked();
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::Debug { config } = &mut node.kind {
                                    config.console = checked;
                                }
                            },
                        );
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
