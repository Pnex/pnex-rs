use super::*;

/// Mutation du nœud **sélectionné** (l'inspecteur ne cible que lui, et il
/// est remonté par `key` à chaque sélection). Pas de capture de `String`
/// dans les handlers : `cx` est `Copy`, l'id est relu du signal.
pub(super) fn patch_selected(cx: &mut EditorCx, f: impl FnOnce(&mut FlowNode) + 'static) {
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
pub(super) fn remove_selected(cx: &mut EditorCx) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| state::remove_node(graph, &node_id));
}

/// `"12"` → `Some(12.0)`, `""` → `None`, invalide → `None`.
pub(super) fn parse_secs(raw: &str) -> Option<f64> {
    let value = raw.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// Pins **d'entrée** disponibles pour un device d'une ligne de lecture :
/// résout le pk depuis la liste devices, puis le pinout en cache. Vide si
/// le device est inconnu ou le pinout pas encore chargé.
pub(super) fn input_pins_of(
    devices: &Resource<Vec<pnex_core::Device>>,
    cache: &Signal<std::collections::HashMap<i64, Vec<api::pins::PinoutPin>>>,
    device_slug: &str,
) -> Vec<api::pins::PinoutPin> {
    let list = devices.value().read().clone().unwrap_or_default();
    let Some(pk) = list
        .iter()
        .find(|d| d.device_id == device_slug)
        .map(|d| d.id)
    else {
        return Vec::new();
    };
    cache
        .cloned()
        .get(&pk)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|pin| {
            // Mode-less pins (GND/3V3/flash/screen-reserved) are neither
            // readable nor subscribable — and their duplicated labels (3× GND
            // on the 38p board) collide in keyed sibling lists (dioxus panic).
            pin.mode.is_some()
                && pin.mode.as_deref() != Some("digital_out")
                && pin.mode.as_deref() != Some("pwm_out")
        })
        .collect()
}

/// Pins de **sortie** (actionneur, `digital_out`) d'un device — l'inverse
/// d'`input_pins_of`, pour le pin actionneur d'une carte de régulation.
pub(super) fn output_pins_of(
    devices: &Resource<Vec<pnex_core::Device>>,
    cache: &Signal<std::collections::HashMap<i64, Vec<api::pins::PinoutPin>>>,
    device_slug: &str,
) -> Vec<api::pins::PinoutPin> {
    let list = devices.value().read().clone().unwrap_or_default();
    let Some(pk) = list
        .iter()
        .find(|d| d.device_id == device_slug)
        .map(|d| d.id)
    else {
        return Vec::new();
    };
    cache
        .cloned()
        .get(&pk)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|pin| {
            pin.mode.as_deref() == Some("digital_out") || pin.mode.as_deref() == Some("pwm_out")
        })
        .collect()
}

/// Config TT d'un nœud reg (heat ou cool) — les deux variantes partagent le
/// formulaire, seul le sens diffère.
pub(super) fn reg_tt_config_mut(node: &mut FlowNode) -> Option<&mut RegTtConfig> {
    match &mut node.kind {
        FlowNodeKind::RegTtHeat { config } | FlowNodeKind::RegTtCool { config } => Some(config),
        _ => None,
    }
}

/// Config PID d'un nœud reg.
pub(super) fn reg_pid_config_mut(node: &mut FlowNode) -> Option<&mut RegPidConfig> {
    match &mut node.kind {
        FlowNodeKind::RegPid { config } => Some(config),
        _ => None,
    }
}

/// `""` → `None`, sinon u32 (champs entiers des cartes reg).
pub(super) fn parse_u32(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok()
}

// ─────────────── helpers de champs ───────────────

/// Champ texte générique (texte local + commit via réducteur).
pub(super) fn text_field(
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
pub(super) fn v_to_string(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Payload injecté → texte d'édition (null = vide).
pub(super) fn payload_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

/// Config Red → texte d'édition (objet par défaut).
pub(super) fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "{}".into(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}
