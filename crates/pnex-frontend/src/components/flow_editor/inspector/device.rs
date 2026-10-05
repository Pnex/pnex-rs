use super::helpers::*;
use super::*;

// ─────────────── device-read / device-write (2026-09-19) ───────────────

/// Toutes les pins d'un device (n'importe quel mode) — le nœud device-read
/// peut relire l'état des sorties aussi bien que les entrées.
fn all_pins_of(
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
    cache.cloned().get(&pk).cloned().unwrap_or_default()
}

/// Mutates the selected node + rewires its output ports: a device-read wire
/// follows **its port label** (pin or device-name port), not its index — a
/// removed pin takes its wire, surviving ports never inherit a removed
/// neighbor's wire.
fn patch_selected_and_prune(cx: &mut EditorCx, f: impl FnOnce(&mut FlowNode) + 'static) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == node_id) {
            let old_pins = match &node.kind {
                FlowNodeKind::DeviceRead { config } => config.pins.clone(),
                _ => Vec::new(),
            };
            f(node);
            let new_pins = match &node.kind {
                FlowNodeKind::DeviceRead { config } => config.pins.clone(),
                _ => Vec::new(),
            };
            state::rewire_ports_by_label(graph, &node_id, &old_pins, &new_pins);
        }
    });
}

/// Mutates the selected function node + rewires its ports around a pick or
/// a rebase: output wires follow **their output name** (a removed output
/// takes its wire, survivors keep theirs even when the index shifts) and
/// input annotations of removed inputs are pruned (the runtime wire goes
/// only when the source feeds nothing else).
pub(super) fn patch_selected_function_and_rewire(
    cx: &mut EditorCx,
    f: impl FnOnce(&mut FlowNode) + 'static,
) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == node_id) {
            let old_outputs = match &node.kind {
                FlowNodeKind::PnexFunction { config } => config.outputs.clone(),
                _ => Vec::new(),
            };
            f(node);
            let (new_inputs, new_outputs) = match &node.kind {
                FlowNodeKind::PnexFunction { config } => {
                    (config.inputs.clone(), config.outputs.clone())
                }
                _ => (Vec::new(), Vec::new()),
            };
            state::rewire_function_outputs(graph, &node_id, &old_outputs, &new_outputs);
            state::prune_function_inputs(graph, &node_id, &new_inputs);
        }
    });
}

/// Mutates the selected notify node + prunes its var anchors around a
/// template (re-)pick: input annotations of vars that left the template are
/// pruned (the runtime wire goes only when the source feeds nothing else),
/// surviving vars keep their wire.
pub(super) fn patch_selected_notify_and_rewire(
    cx: &mut EditorCx,
    f: impl FnOnce(&mut FlowNode) + 'static,
) {
    let Some(node_id) = cx.selected_node.peek().clone() else {
        return;
    };
    cx.update_graph(move |graph| {
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == node_id) {
            f(node);
            let new_vars = match &node.kind {
                FlowNodeKind::PnexNotify { config } => config.template_vars.clone(),
                _ => Vec::new(),
            };
            state::prune_notify_var_anchors(graph, &node_id, &new_vars);
        }
    });
}

/// Checks or unchecks one read source (pin label or metric name) of the
/// selected device-read node; ports stay in natural order.
fn toggle_read_source(cx: &mut EditorCx, label: String, checked: bool) {
    patch_selected_and_prune(cx, move |node| {
        if let FlowNodeKind::DeviceRead { config } = &mut node.kind {
            if checked && !config.pins.contains(&label) {
                config.pins.push(label.clone());
            } else if !checked {
                config.pins.retain(|p| p != &label);
            }
            config.pins = geometry::sorted_pins(&config.pins);
        }
    });
}

/// Metrics a device publishes that are not pins of its board (custom
/// firmware `addMetric`, e.g. `temperature`), with their checked state.
/// Checked entries missing from the catalog (no sample in 24 h) stay listed
/// so they can be unchecked — once the pinout is loaded, so a checked pin is
/// never shown as a metric meanwhile.
fn metric_rows_of(
    series: &[pnex_core::TelemetrySeriesInfo],
    device_slug: &str,
    pins: &[api::pins::PinoutPin],
    selected: &[String],
) -> Vec<(String, bool)> {
    let is_pin = |name: &str| {
        pins.iter()
            .any(|p| p.label.trim().eq_ignore_ascii_case(name.trim()))
    };
    let mut rows: Vec<String> = series
        .iter()
        .filter(|s| s.device_id == device_slug && !is_pin(&s.metric))
        .map(|s| s.metric.clone())
        .chain(
            selected
                .iter()
                .filter(|p| !pins.is_empty() && !is_pin(p))
                .cloned(),
        )
        .collect();
    rows.sort();
    rows.dedup();
    rows.into_iter()
        .map(|m| {
            let checked = selected.contains(&m);
            (m, checked)
        })
        .collect()
}

/// Charge le pinout du device référencé par le nœud courant (cache + effet
/// graphe-dépendant, école DeviceForm historique). Retourne le pk du device.
fn device_pk_of(devices: &Resource<Vec<pnex_core::Device>>, slug: &str) -> Option<i64> {
    let list = devices.value().read().clone().unwrap_or_default();
    list.iter().find(|d| d.device_id == slug).map(|d| d.id)
}

#[component]
pub(super) fn DeviceReadForm(
    mut cx: EditorCx,
    initial: DeviceReadConfig,
    can_write: bool,
) -> Element {
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
    let mut pins_cache =
        use_signal(std::collections::HashMap::<i64, Vec<api::pins::PinoutPin>>::new);
    let mut pins_requested = use_signal(std::collections::HashSet::<i64>::new);
    use_effect(move || {
        let g = cx.graph.cloned();
        let Some(node_id) = cx.selected_node.cloned() else {
            return;
        };
        let wanted: Vec<i64> = g
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .and_then(|n| match &n.kind {
                FlowNodeKind::DeviceRead { config } => Some(config.device_id.clone()),
                _ => None,
            })
            .filter(|slug| !slug.is_empty())
            .and_then(|slug| device_pk_of(&devices, &slug))
            .into_iter()
            .collect();
        for pk in wanted {
            if !pins_requested.cloned().contains(&pk) {
                pins_requested.insert(pk);
                spawn(async move {
                    if let Ok(pins) = api::pins::pinout(pk).await {
                        pins_cache.insert(pk, pins.pins);
                    }
                });
            }
        }
    });

    // Telemetry catalog: the metrics a custom firmware publishes are read by
    // name like a pin (same live cache / OpenObserve series).
    let catalog = use_resource(move || async move {
        api::telemetry::catalog()
            .await
            .map(|c| c.series)
            .unwrap_or_default()
    });

    let mut window = use_signal(move || v_to_string(initial.window_secs));
    let device_slug = initial.device_id.clone();
    let metric_rows = metric_rows_of(
        &catalog.value().read().clone().unwrap_or_default(),
        &device_slug,
        &all_pins_of(&devices, &pins_cache, &device_slug),
        &initial.pins,
    );
    // Pas de `let` dans rsx : (pin, déjà cochée, pin de sortie) précalculés.
    // Une pin de sortie (digital_out/pwm_out) n'est pas une entrée de
    // lecture : checkbox **désactivée** (grisée) — sauf si déjà cochée
    // (config héritée : il faut pouvoir la décocher), la validation reste
    // le garde-fou.
    let pin_rows: Vec<(api::pins::PinoutPin, bool, bool)> =
        all_pins_of(&devices, &pins_cache, &device_slug)
            .into_iter()
            // Mode-less pins (GND/3V3/flash/screen-reserved) never produce
            // telemetry — and their duplicated labels (3× GND on the 38p
            // board) collide in keyed sibling lists (dioxus panic).
            .filter(|pin| pin.mode.is_some())
            .map(|pin| {
                let is_output = pin.mode.as_deref() == Some("digital_out")
                    || pin.mode.as_deref() == Some("pwm_out");
                let checked = initial.pins.iter().any(|p| p == &pin.label);
                (pin, checked, is_output)
            })
            .collect();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-device-read-help")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |event| {
                    let slug = event.value();
                    patch_selected_and_prune(
                        &mut cx,
                        move |node| {
                            if let FlowNodeKind::DeviceRead { config } = &mut node.kind {
                                config.device_id = slug;
                                config.pins.clear();
                            }
                        },
                    );
                },
                option { value: "", selected: device_slug.is_empty(), {t!("flows-device-device-none")} }
                for device in devices.value().read().clone().unwrap_or_default() {
                    option {
                        key: "{device.id}",
                        value: "{device.device_id}",
                        selected: device_slug == device.device_id,
                        {device.device_id.clone()}
                    }
                }
            }
            div { class: "space-y-1",
                for (pin, checked, is_output_pin) in pin_rows.clone() {
                    label {
                        key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                        class: if is_output_pin && !checked { "flex items-center gap-2 text-sm opacity-40" } else { "flex items-center gap-2 text-sm" },
                        input {
                            r#type: "checkbox",
                            disabled: !can_write || (is_output_pin && !checked),
                            checked,
                            onchange: move |event| {
                                toggle_read_source(&mut cx, pin.label.clone(), event.checked());
                            },
                        }
                        span { class: "font-mono text-xs",
                            {
                                if pin.source == "overlay" {
                                    format!(
                                        "{} ({} \u{00b7} {})",
                                        pin.label,
                                        pin.mode.as_deref().unwrap_or("?"),
                                        t!("flows-device-pin-overlay"),
                                    )
                                } else {
                                    format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))
                                }
                            }
                        }
                    }
                }
                for (metric, checked) in metric_rows.clone() {
                    label {
                        key: "metric-{metric}",
                        class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            disabled: !can_write,
                            checked,
                            onchange: move |event| {
                                toggle_read_source(&mut cx, metric.clone(), event.checked());
                            },
                        }
                        span { class: "font-mono text-xs",
                            {format!("{} ({})", metric, t!("flows-device-metric"))}
                        }
                    }
                }
            }
            {
                text_field(
                    t!("flows-device-window"),
                    window,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        window.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let FlowNodeKind::DeviceRead { config } = &mut node.kind {
                                    if let Some(w) = parse_secs(&raw) {
                                        config.window_secs = w;
                                    }
                                }
                            },
                        );
                    },
                )
            }
        }
    }
}

#[component]
pub(super) fn DeviceWriteForm(
    mut cx: EditorCx,
    initial: DeviceWriteConfig,
    can_write: bool,
    flow_id: i64,
) -> Element {
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
    let mut pins_cache =
        use_signal(std::collections::HashMap::<i64, Vec<api::pins::PinoutPin>>::new);
    let mut pins_requested = use_signal(std::collections::HashSet::<i64>::new);
    use_effect(move || {
        let g = cx.graph.cloned();
        let Some(node_id) = cx.selected_node.cloned() else {
            return;
        };
        let wanted: Vec<i64> = g
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .and_then(|n| match &n.kind {
                FlowNodeKind::DeviceWrite { config } => Some(config.device_id.clone()),
                _ => None,
            })
            .filter(|slug| !slug.is_empty())
            .and_then(|slug| device_pk_of(&devices, &slug))
            .into_iter()
            .collect();
        for pk in wanted {
            if !pins_requested.cloned().contains(&pk) {
                pins_requested.insert(pk);
                spawn(async move {
                    if let Ok(pins) = api::pins::pinout(pk).await {
                        pins_cache.insert(pk, pins.pins);
                    }
                });
            }
        }
    });
    let device_slug = initial.device_id.clone();
    // Precomputed pin rows (no `let` inside `for` rsx): a pin claimed by
    // ANOTHER deployed flow renders greyed and uncheckable — claims of THIS
    // flow (or an already-checked pin) stay editable so the user can release
    // them. The deploy gate remains the hard stop server-side.
    let pin_rows: Vec<(api::pins::PinoutPin, bool, bool, Option<String>)> =
        output_pins_of(&devices, &pins_cache, &device_slug)
            .into_iter()
            .map(|pin| {
                let checked = initial.pins.iter().any(|p| p == &pin.label);
                let other_owners: Vec<String> = pin
                    .reserved_by
                    .iter()
                    .filter(|r| r.flow_id != flow_id)
                    .map(|r| r.flow_name.clone())
                    .collect();
                let disabled = !other_owners.is_empty() && !checked;
                let hint = if disabled {
                    Some(t!("flows-pin-reserved", flow: other_owners.join(", ")))
                } else {
                    None
                };
                (pin, checked, disabled, hint)
            })
            .collect();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-device-write-help")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |event| {
                    let slug = event.value();
                    patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| { // Unusable annotations (pins gone) must not linger.
                            if let FlowNodeKind::DeviceWrite { config } = &mut node.kind {
                                config.device_id = slug;
                                config.pins.clear();
                                node.inputs.clear();
                            }
                        },
                    );
                },
                option { value: "", selected: device_slug.is_empty(), {t!("flows-device-device-none")} }
                for device in devices.value().read().clone().unwrap_or_default() {
                    option {
                        key: "{device.id}",
                        value: "{device.device_id}",
                        selected: device_slug == device.device_id,
                        {device.device_id.clone()}
                    }
                }
            }
            div { class: "space-y-1.5",
                for (pin, checked, reserved_elsewhere, hint) in pin_rows {
                    label {
                        key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                        class: "flex items-start gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            class: "mt-0.5 shrink-0",
                            disabled: !can_write || reserved_elsewhere,
                            checked,
                            onchange: move |event| {
                                let label = pin.label.clone();
                                let checked = event.checked();
                                patch_selected(
                                    &mut cx,
                                    move |node: &mut FlowNode| {
                                        if let FlowNodeKind::DeviceWrite { config } = &mut node.kind {
                                            if checked && !config.pins.contains(&label) { // Uncheck a pin = its input wiring goes.
                                                config.pins.push(label.clone());
                                            } else if !checked {
                                                config.pins.retain(|p| p != &label);
                                            }
                                            config.pins = geometry::sorted_pins(&config.pins);
                                            node.inputs.retain(|w| config.pins.contains(&w.pin));
                                        }
                                    },
                                );
                            },
                        }
                        // Column layout: pin line on top, reservation hint on
                        // its own line below (a side-by-side flex row wraps
                        // badly in the narrow inspector panel).
                        div { class: "min-w-0 flex-1 leading-tight",
                            span { class: if reserved_elsewhere { "font-mono text-xs opacity-40" } else { "font-mono text-xs" },
                                {
                                    if pin.source == "overlay" {
                                        format!(
                                            "{} ({} \u{00b7} {})",
                                            pin.label,
                                            pin.mode.as_deref().unwrap_or("?"),
                                            t!("flows-device-pin-overlay"),
                                        )
                                    } else {
                                        format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))
                                    }
                                }
                            }
                            if let Some(h) = hint {
                                span { class: "block text-[11px] leading-snug text-amber-600 mt-0.5",
                                    {h}
                                }
                            }
                        }
                    }
                }
            }
            p { class: "text-xs text-gray-400 font-mono", {t!("flows-device-write-payload")} }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(label: &str) -> api::pins::PinoutPin {
        api::pins::PinoutPin {
            gpio: Some(0),
            label: label.into(),
            mode: Some("analog_in".into()),
            source: "instance".into(),
            subscribed_ms: None,
            pos: None,
            kind: None,
            fns: Vec::new(),
            flags: Vec::new(),
            available_modes: Vec::new(),
            warnings: Vec::new(),
            reserved: false,
            last_value: None,
            reserved_by: Vec::new(),
        }
    }

    fn series(device: &str, metric: &str) -> pnex_core::TelemetrySeriesInfo {
        pnex_core::TelemetrySeriesInfo {
            metric: metric.into(),
            device_id: device.into(),
            pred_dev: None,
            last_value: 1.0,
            last_seen: None,
        }
    }

    #[test]
    fn custom_metrics_are_offered_but_pins_are_not_duplicated() {
        let catalog = [
            series("climate-1", "temperature"),
            series("climate-1", "humidity"),
            series("climate-1", "a0"),
            series("other", "pressure"),
        ];
        let rows = metric_rows_of(
            &catalog,
            "climate-1",
            &[pin("A0")],
            &["temperature".to_string()],
        );
        assert_eq!(
            rows,
            vec![
                ("humidity".to_string(), false),
                ("temperature".to_string(), true)
            ]
        );
    }

    #[test]
    fn checked_metric_without_recent_sample_stays_uncheckable() {
        let rows = metric_rows_of(&[], "climate-1", &[pin("A0")], &["humidity".to_string()]);
        assert_eq!(rows, vec![("humidity".to_string(), true)]);
    }

    #[test]
    fn checked_pin_is_not_shown_as_metric_before_the_pinout_loads() {
        assert!(metric_rows_of(&[], "climate-1", &[], &["A0".to_string()]).is_empty());
    }
}
