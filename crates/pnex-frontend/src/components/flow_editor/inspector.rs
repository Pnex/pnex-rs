//! Inspecteur de configuration du nœud sélectionné — un composant par kind.
//!
//! Remonté par `key` à chaque changement de sélection : les champs repartent
//! de la config du nœud, état local propre (pattern `MetadataEditor` des
//! devices : texte local + drapeau d'invalidité pour les JSON).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    DebugConfig, FlowNode, FlowNodeKind, InjectConfig, PnexSqlConfig,
};

use super::{state, EditorCx};
use crate::components::confirm::ConfirmDialog;

#[component]
pub(crate) fn Inspector(mut cx: EditorCx, can_write: bool) -> Element {
    let Some(node_id) = cx.selected_node.cloned() else {
        return empty_panel();
    };
    let graph = cx.graph.cloned();
    let Some(node) = graph.nodes.iter().find(|n| n.id == node_id).cloned() else {
        return empty_panel();
    };

    // Violations du nœud (messages FR de pnex-core, affichés tels quels).
    let node_violations = cx.violations_of(&node_id);
    let mut confirm_delete = use_signal(|| false);

    rsx! {
        div { class: "w-72 shrink-0 bg-white rounded-lg shadow-sm p-4 space-y-3 overflow-y-auto",
            div { class: "flex items-center justify-between",
                h3 { class: "text-xs font-semibold text-gray-400 uppercase tracking-wider",
                    {format!("#{}", node.id)}
                }
                if can_write {
                    button {
                        class: "text-xs text-red-600 hover:text-red-700",
                        onclick: move |_| confirm_delete.set(true),
                        {t!("flows-node-delete")}
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-node-name")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm disabled:bg-gray-50 disabled:text-gray-400",
                    r#type: "text",
                    value: "{node.name.clone().unwrap_or_default()}",
                    disabled: !can_write,
                    oninput: move |event| patch_selected(&mut cx, move |node: &mut FlowNode| {
                        let value = event.value();
                        node.name = if value.trim().is_empty() { None } else { Some(value) };
                    }),
                }
            }
            {kind_form(&node, cx, can_write)}

            if !node_violations.is_empty() {
                div { class: "rounded-lg bg-red-50 border border-red-200 p-2 space-y-1",
                    for violation in node_violations {
                        p { class: "text-xs text-red-700", {violation.message.clone()} }
                    }
                }
            }

            if confirm_delete() {
                ConfirmDialog {
                    title: t!("flows-node-delete"),
                    message: format!("#{}", node.id),
                    confirm_label: t!("flows-node-delete"),
                    on_confirm: move |_| {
                        confirm_delete.set(false);
                        remove_selected(&mut cx);
                        cx.selected_node.set(None);
                    },
                    on_cancel: move |_| confirm_delete.set(false),
                }
            }
        }
    }
}

/// Panneau vide quand aucun nœud n'est sélectionné.
fn empty_panel() -> Element {
    rsx! {
        div { class: "w-72 shrink-0 bg-white rounded-lg shadow-sm p-4",
            p { class: "text-xs text-gray-400", {t!("flows-inspector-empty")} }
        }
    }
}

/// Formulaire propre au kind — un composant par variante (hooks isolés).
fn kind_form(node: &FlowNode, cx: EditorCx, can_write: bool) -> Element {
    match &node.kind {
        FlowNodeKind::Inject { config } => rsx! {
            InjectForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::PnexSql { config } => rsx! {
            SqlForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Debug { config } => rsx! {
            DebugForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Red { type_name, config } => rsx! {
            RedForm {
                cx,
                initial_type: type_name.clone(),
                initial_config: config.clone(),
                can_write,
            }
        },
    }
}

/// Mutation du nœud **sélectionné** (l'inspecteur ne cible que lui, et il
/// est remonté par `key` à chaque sélection). Pas de capture de `String`
/// dans les handlers : `cx` est `Copy`, l'id est relu du signal.
fn patch_selected(cx: &mut EditorCx, f: impl FnOnce(&mut FlowNode) + 'static) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == node_id) {
            f(node);
        }
    });
}

/// Supprime le nœud sélectionné (id relu du signal — aucune capture).
fn remove_selected(cx: &mut EditorCx) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| state::remove_node(graph, &node_id));
}

/// `"12"` → `Some(12.0)`, `""` → `None`, invalide → `None`.
fn parse_secs(raw: &str) -> Option<f64> {
    let value = raw.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// ─────────────── inject ───────────────

#[component]
fn InjectForm(mut cx: EditorCx, initial: InjectConfig, can_write: bool) -> Element {
    let mut repeat = use_signal(move || initial.repeat_secs.map(v_to_string).unwrap_or_default());
    let mut cron = use_signal(move || initial.cron.clone());
    let mut once = use_signal(move || initial.once_delay_secs.map(v_to_string).unwrap_or_default());
    let mut topic = use_signal(move || initial.topic.clone().unwrap_or_default());
    let mut payload = use_signal(move || payload_text(&initial.payload));
    let mut payload_invalid = use_signal(|| false);

    rsx! {
        div { class: "space-y-3",
            {text_field(t!("flows-inject-repeat"), repeat, !can_write, move |event| {
                let raw = event.value();
                repeat.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Inject { config } = &mut node.kind {
                        config.repeat_secs = parse_secs(&raw);
                    }
                });
            })}
            {text_field(t!("flows-inject-cron"), cron, !can_write, move |event| {
                let raw = event.value();
                cron.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Inject { config } = &mut node.kind {
                        config.cron = raw;
                    }
                });
            })}
            {text_field(t!("flows-inject-once-delay"), once, !can_write, move |event| {
                let raw = event.value();
                once.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Inject { config } = &mut node.kind {
                        config.once_delay_secs = parse_secs(&raw);
                    }
                });
            })}
            {text_field(t!("flows-inject-topic"), topic, !can_write, move |event| {
                let raw = event.value();
                topic.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Inject { config } = &mut node.kind {
                        config.topic = Some(raw).filter(|t| !t.is_empty());
                    }
                });
            })}
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-inject-payload")} }
                textarea {
                    class: if payload_invalid() {
                        "w-full h-20 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono"
                    } else {
                        "w-full h-20 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono"
                    },
                    value: "{payload}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        match serde_json::from_str::<serde_json::Value>(&raw) {
                            Ok(value) => {
                                payload_invalid.set(false);
                                payload.set(raw.clone());
                                patch_selected(&mut cx, move |node: &mut FlowNode| {
                                    if let FlowNodeKind::Inject { config } = &mut node.kind {
                                        config.payload = value;
                                    }
                                });
                            }
                            Err(_) => {
                                // Saisie intermédiaire invalide : texte conservé,
                                // graphe intact (pattern MetadataEditor).
                                payload_invalid.set(true);
                                payload.set(raw);
                            }
                        }
                    },
                }
                if payload_invalid() {
                    span { class: "text-xs text-red-600 mt-1", {t!("flows-inject-payload-invalid")} }
                }
            }
        }
    }
}

/// ─────────────── pnex_sql ───────────────

#[component]
fn SqlForm(mut cx: EditorCx, initial: PnexSqlConfig, can_write: bool) -> Element {
    let mut query = use_signal(move || initial.query.clone());
    let mut params = use_signal(move || initial.params.join(", "));

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-sql-query")} }
                textarea {
                    class: "w-full h-28 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                    value: "{query}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        query.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::PnexSql { config } = &mut node.kind {
                                config.query = raw;
                            }
                        });
                    },
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-sql-params")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    value: "{params}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        params.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::PnexSql { config } = &mut node.kind {
                                config.params = raw
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|p| !p.is_empty())
                                    .map(str::to_string)
                                    .collect();
                            }
                        });
                    },
                }
            }
        }
    }
}

/// ─────────────── debug ───────────────

#[component]
fn DebugForm(mut cx: EditorCx, initial: DebugConfig, can_write: bool) -> Element {
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

/// ─────────────── red (échappement Node-RED) ───────────────

#[component]
fn RedForm(
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

// ─────────────── helpers de champs ───────────────

/// Champ texte générique (texte local + commit via réducteur).
fn text_field(
    label: impl Into<String>,
    value: Signal<String>,
    disabled: bool,
    oninput: impl FnMut(FormEvent) + 'static,
) -> Element {
    let label = label.into();
    rsx! {
        label { class: "block",
            span { class: "text-xs font-medium text-gray-500 mb-1 block", {label} }
            input {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                r#type: "text",
                value: "{value}",
                disabled,
                oninput,
            }
        }
    }
}

/// f64 → chaîne sans décimales inutiles (5.0 → « 5 »).
fn v_to_string(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Payload injecté → texte d'édition (null = vide).
fn payload_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// Config Red → texte d'édition (objet par défaut).
fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "{}".into(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}
