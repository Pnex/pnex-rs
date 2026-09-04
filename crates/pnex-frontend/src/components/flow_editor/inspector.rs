//! Inspecteur de configuration du nœud sélectionné — un composant par kind.
//!
//! Remonté par `key` à chaque changement de sélection : les champs repartent
//! de la config du nœud, état local propre (pattern `MetadataEditor` des
//! devices : texte local + drapeau d'invalidité pour les JSON).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    CalcConfig, DebugConfig, DeviceConfig, DeviceRead, FlowNode, FlowNodeKind, InjectConfig,
    MetricConfig, PnexSqlConfig,
};

use super::{state, EditorCx};
use crate::api;
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
        FlowNodeKind::Device { config } => rsx! {
            DeviceForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Calc { config } => rsx! {
            CalcForm { cx, initial: config.clone(), can_write }
        },
        FlowNodeKind::Metric { config } => rsx! {
            MetricForm { cx, initial: config.clone(), can_write }
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

/// Pins **d'entrée** disponibles pour un device d'une ligne de lecture :
/// résout le pk depuis la liste devices, puis le pinout en cache. Vide si
/// le device est inconnu ou le pinout pas encore chargé.
fn input_pins_of(
    devices: &Resource<Vec<pnex_core::Device>>,
    cache: &Signal<std::collections::HashMap<i64, Vec<api::pins::PinoutPin>>>,
    device_slug: &str,
) -> Vec<api::pins::PinoutPin> {
    let list = devices.value().read().clone().unwrap_or_default();
    let Some(pk) = list.iter().find(|d| d.device_id == device_slug).map(|d| d.id) else {
        return Vec::new();
    };
    cache
        .cloned()
        .get(&pk)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|pin| pin.mode != "digital_out")
        .collect()
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

/// ─────────────── device (lectures pins, Phase 6) ───────────────

#[component]
fn DeviceForm(mut cx: EditorCx, initial: DeviceConfig, can_write: bool) -> Element {
    // Devices de l'org (slug → pk pour charger le pinout), chargés une fois.
    let devices = use_resource(move || async move {
        api::devices::list(&api::devices::DeviceFilters {
            active: Some(true),
            limit: Some(200),
            ..Default::default()
        })
        .await
        .map(|page| page.results)
        .unwrap_or_default()
    });
    let mut pins_cache = use_signal(std::collections::HashMap::<i64, Vec<api::pins::PinoutPin>>::new);
    let mut pins_requested = use_signal(std::collections::HashSet::<i64>::new);
    // Précharge le pinout des devices référencés par le nœud. La lecture du
    // graphe se fait DANS la closure (dépendance suivie) : l'effet se
    // re-déclenche au choix d'un device dans une ligne — un snapshot figé au
    // montage ne verrait jamais les devices ajoutés ensuite (retour e2e
    // 2026-09-04 : select pin désespérément vide). `pins_requested` évite les
    // requêtes en double (l'effet re-tourne à chaque mutation du graphe).
    use_effect(move || {
        let g = cx.graph.cloned();
        let list = devices.value().read().clone().unwrap_or_default();
        let Some(node_id) = cx.selected_node.cloned() else {
            return;
        };
        let wanted: Vec<i64> = g
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .and_then(|n| match &n.kind {
                FlowNodeKind::Device { config } => Some(config.reads.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .iter()
            .filter(|r| !r.device_id.is_empty())
            .filter_map(|r| list.iter().find(|d| d.device_id == r.device_id).map(|d| d.id))
            .collect();
        for pk in wanted {
            if !pins_requested.cloned().contains(&pk) {
                pins_requested.insert(pk);
                spawn(async move {
                    if let Ok(pins) = api::pins::pinout(pk).await {
                        pins_cache.insert(pk, pins);
                    }
                });
            }
        }
    });

    let mut window = use_signal(move || v_to_string(initial.window_secs));

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-device-multi-help")} }
            div { class: "space-y-2",
                for (i, read) in initial.reads.iter().enumerate() {
                    div { key: "{i}-{read.device_id}-{read.pin}", class: "rounded-lg border border-gray-200 p-2 space-y-2",
                        div { class: "flex items-center gap-1",
                            select {
                                class: "flex-1 px-2 py-1 border border-gray-300 rounded-lg text-sm",
                                disabled: !can_write,
                                // Sélection par attribut `selected` des <option> :
                                // `value` sur <select> est ignoré par les navigateurs.
                                onchange: move |event| {
                                    let slug = event.value();
                                    patch_selected(&mut cx, move |node: &mut FlowNode| {
                                        if let FlowNodeKind::Device { config } = &mut node.kind {
                                            if let Some(r) = config.reads.get_mut(i) {
                                                r.device_id = slug;
                                                r.pin.clear();
                                            }
                                        }
                                    });
                                },
                                option { value: "", selected: read.device_id.is_empty(), {t!("flows-device-device-none")} }
                                for device in devices.value().read().clone().unwrap_or_default() {
                                    option {
                                        key: "{device.id}",
                                        value: "{device.device_id}",
                                        selected: read.device_id == device.device_id,
                                        {device.device_id.clone()}
                                    }
                                }
                            }
                            button {
                                class: "text-xs text-red-600 hover:text-red-700 px-1",
                                disabled: !can_write,
                                onclick: move |_| {
                                    patch_selected(&mut cx, move |node: &mut FlowNode| {
                                        if let FlowNodeKind::Device { config } = &mut node.kind {
                                            if i < config.reads.len() {
                                                config.reads.remove(i);
                                            }
                                        }
                                    });
                                },
                                "✕"
                            }
                        }
                        select {
                            class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                            disabled: !can_write || read.device_id.is_empty(),
                            onchange: move |event| {
                                let pin = event.value();
                                patch_selected(&mut cx, move |node: &mut FlowNode| {
                                    if let FlowNodeKind::Device { config } = &mut node.kind {
                                        if let Some(r) = config.reads.get_mut(i) {
                                            r.pin = pin;
                                        }
                                    }
                                });
                            },
                            option { value: "", selected: read.pin.is_empty(), {t!("flows-device-pin-none")} }
                            for pin in input_pins_of(&devices, &pins_cache, &read.device_id) {
                                option {
                                    key: "{pin.gpio}",
                                    value: "{pin.label}",
                                    selected: read.pin == pin.label,
                                    {if pin.source == "overlay" {
                                        format!("{} ({} · {})", pin.label, pin.mode, t!("flows-device-pin-overlay"))
                                    } else {
                                        format!("{} ({})", pin.label, pin.mode)
                                    }}
                                }
                            }
                        }
                        if !read.device_id.is_empty() && !read.pin.trim().is_empty() {
                            span { class: "block text-xs text-gray-400 font-mono",
                                {pnex_core::device_payload_key(&read.device_id, &read.pin)}
                            }
                        }
                    }
                }
            }
            button {
                class: "w-full px-2 py-1.5 text-sm text-blue-600 border border-dashed border-blue-300 rounded-lg hover:bg-blue-50 transition-colors disabled:opacity-40",
                disabled: !can_write,
                onclick: move |_| {
                    patch_selected(&mut cx, move |node: &mut FlowNode| {
                        if let FlowNodeKind::Device { config } = &mut node.kind {
                            config.reads.push(DeviceRead::default());
                        }
                    });
                },
                {t!("flows-device-add-read")}
            }
            {text_field(t!("flows-device-window"), window, !can_write, move |event| {
                let raw = event.value();
                window.set(raw.clone());
                patch_selected(&mut cx, move |node: &mut FlowNode| {
                    if let FlowNodeKind::Device { config } = &mut node.kind {
                        if let Some(w) = parse_secs(&raw) {
                            config.window_secs = w;
                        }
                    }
                });
            })}
        }
    }
}

/// ─────────────── calc ───────────────

#[component]
fn CalcForm(mut cx: EditorCx, initial: CalcConfig, can_write: bool) -> Element {
    let mut expression = use_signal(move || initial.expression.clone());
    let errors = use_memo(move || pnex_core::validate_calc(&expression.cloned()));

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-calc-expression")} }
                textarea {
                    class: if !errors.cloned().is_empty() {
                        "w-full h-20 px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm font-mono"
                    } else {
                        "w-full h-20 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono"
                    },
                    value: "{expression}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        expression.set(raw.clone());
                        patch_selected(&mut cx, move |node: &mut FlowNode| {
                            if let FlowNodeKind::Calc { config } = &mut node.kind {
                                config.expression = raw;
                            }
                        });
                    },
                }
            }
            if !errors.cloned().is_empty() {
                div { class: "rounded-lg bg-red-50 border border-red-200 p-2 space-y-1",
                    for e in errors.cloned() {
                        p { class: "text-xs text-red-700", {e.to_string()} }
                    }
                }
            }
            if !expression.cloned().trim().is_empty() && errors.cloned().is_empty() {
                p { class: "text-xs text-gray-500",
                    {format!("{} : {}", t!("flows-calc-vars"), pnex_core::calc_variables(&expression.cloned()).join(", "))}
                }
            }
            p { class: "text-xs text-gray-400", {t!("flows-calc-functions-help")} }
        }
    }
}

/// ─────────────── metric ───────────────

#[component]
fn MetricForm(mut cx: EditorCx, initial: MetricConfig, can_write: bool) -> Element {
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
            p { class: "text-xs text-gray-400", {t!("flows-metric-labels-help")} }
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
