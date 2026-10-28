use super::*;
use crate::components::ui::FIELD_LABEL;

/// Mutation of the **selected** node (the inspector only targets it, and it
/// is remounted by `key` on every selection). No `String` capture in the
/// handlers: `cx` is `Copy`, the id is re-read from the signal.
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

/// Deletes the selected node (id re-read from the signal — no capture).
pub(super) fn remove_selected(cx: &mut EditorCx) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| state::remove_node(graph, &node_id));
}

/// `"12"` → `Some(12.0)`, `""` → `None`, invalid → `None`.
pub(super) fn parse_secs(raw: &str) -> Option<f64> {
    let value = raw.trim().parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// **Input** pins available for a device of a read row: resolves
/// the pk from the device list, then the cached pinout. Empty if
/// the device is unknown or the pinout not loaded yet.
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

/// **Output** pins (actuator, `digital_out`) of a device — the inverse
/// of `input_pins_of`, for the actuator pin of a control card.
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

/// TT config of a reg node (heat or cool) — both variants share the
/// form, only the direction differs.
pub(super) fn reg_tt_config_mut(node: &mut FlowNode) -> Option<&mut RegTtConfig> {
    match &mut node.kind {
        FlowNodeKind::RegTtHeat { config } | FlowNodeKind::RegTtCool { config } => Some(config),
        _ => None,
    }
}

/// PID config of a reg node.
pub(super) fn reg_pid_config_mut(node: &mut FlowNode) -> Option<&mut RegPidConfig> {
    match &mut node.kind {
        FlowNodeKind::RegPid { config } => Some(config),
        _ => None,
    }
}

/// `""` → `None`, otherwise u32 (integer fields of reg cards).
pub(super) fn parse_u32(raw: &str) -> Option<u32> {
    raw.trim().parse::<u32>().ok()
}

// ─────────────── field helpers ───────────────

/// Generic text field (local text + commit via reducer).
pub(super) fn text_field(
    label: impl Into<String>,
    value: Signal<String>,
    disabled: bool,
    oninput: impl FnMut(FormEvent) + 'static,
) -> Element {
    let label = label.into();
    rsx! {
        label { class: "block",
            span { class: FIELD_LABEL, {label} }
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

/// f64 → string without needless decimals (5.0 → "5").
pub(super) fn v_to_string(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

/// Injected payload → edit text (null = empty).
pub(super) fn payload_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}
