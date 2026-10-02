use super::helpers::*;
use super::*;

// ─────────────── cartes de régulation mixtes (reg_*) ───────────────

/// Formulaire des cartes tout-ou-rien (chauffage `heat=true` / clim
/// `heat=false`). Liaison **sur un seul device** (carte mixte : le capteur
/// et la sortie sont sur la même board — la régulation tourne embarquée,
/// le serveur caste la config, D13/D17).
#[component]
pub(super) fn RegTtForm(
    mut cx: EditorCx,
    initial: RegTtConfig,
    can_write: bool,
    heat: bool,
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
    // Précharge le pinout du device sélectionné (lecture du graphe DANS la
    // closure — dépendance suivie, pattern DeviceForm).
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
            .and_then(|n| reg_tt_config_mut_ref(n).map(|c| c.device_id.clone()))
            .unwrap_or_default()
            .lines()
            .filter(|slug| !slug.is_empty())
            .filter_map(|slug| list.iter().find(|d| d.device_id == slug).map(|d| d.id))
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

    let mut device_id = use_signal(move || initial.device_id.clone());
    let mut sensor_pin = use_signal(move || initial.sensor_pin.clone());
    let mut actuator_pin = use_signal(move || initial.actuator_pin.clone());
    let mut setpoint = use_signal(move || v_to_string(initial.setpoint));
    let mut deadband = use_signal(move || v_to_string(initial.deadband));
    let mut min_on = use_signal(move || initial.min_on_secs.to_string());
    let mut min_off = use_signal(move || initial.min_off_secs.to_string());
    let mut sample = use_signal(move || initial.sample_ms.to_string());
    let mut timeout = use_signal(move || initial.data_timeout_secs.to_string());
    let safe_state = initial.safe_state;
    // Precomputed actuator options (no `let` inside `for` rsx): a pin
    // claimed by ANOTHER deployed flow stays listed but greyed, unless it is
    // the current selection — an inherited conflict must stay visible and
    // fixable. The deploy gate remains the hard stop server-side.
    let actuator_options: Vec<(api::pins::PinoutPin, bool, Option<String>)> =
        output_pins_of(&devices, &pins_cache, &device_id.cloned())
            .into_iter()
            .map(|pin| {
                let other_owners: Vec<String> = pin
                    .reserved_by
                    .iter()
                    .filter(|r| r.flow_id != flow_id)
                    .map(|r| r.flow_name.clone())
                    .collect();
                let is_current = actuator_pin.cloned() == pin.label;
                let disabled = !other_owners.is_empty() && !is_current;
                let hint = if disabled {
                    Some(t!("flows-pin-reserved", flow: other_owners.join(", ")))
                } else {
                    None
                };
                (pin, disabled, hint)
            })
            .collect();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-reg-help")} }
            // ── Liaison : un seul device, capteur + actionneur sur la même board.
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-reg-device")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let slug = event.value();
                        device_id.set(slug.clone());
                        // Nouveau device : pins résolus invalidés.
                        sensor_pin.set(String::new());
                        actuator_pin.set(String::new());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_tt_config_mut(node) {
                                    config.device_id = slug;
                                    config.sensor_pin.clear();
                                    config.actuator_pin.clear();
                                }
                            },
                        );
                    },
                    option { value: "", selected: device_id.cloned().is_empty(),
                        {t!("flows-device-device-none")}
                    }
                    for device in devices.value().read().clone().unwrap_or_default() {
                        option {
                            key: "{device.id}",
                            value: "{device.device_id}",
                            selected: device_id.cloned() == device.device_id,
                            {device.device_id.clone()}
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-reg-sensor-pin")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write || device_id.cloned().is_empty(),
                    onchange: move |event| {
                        let pin = event.value();
                        sensor_pin.set(pin.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_tt_config_mut(node) {
                                    config.sensor_pin = pin;
                                }
                            },
                        );
                    },
                    option { value: "", selected: sensor_pin.cloned().is_empty(),
                        {t!("flows-device-pin-none")}
                    }
                    for pin in input_pins_of(&devices, &pins_cache, &device_id.cloned()) {
                        option {
                            key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                            value: "{pin.label}",
                            selected: sensor_pin.cloned() == pin.label,
                            {format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))}
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-reg-actuator-pin")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write || device_id.cloned().is_empty(),
                    onchange: move |event| {
                        let pin = event.value();
                        actuator_pin.set(pin.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_tt_config_mut(node) {
                                    config.actuator_pin = pin;
                                }
                            },
                        );
                    },
                    option { value: "", selected: actuator_pin.cloned().is_empty(),
                        {t!("flows-device-pin-none")}
                    }
                    for (pin, reserved_elsewhere, hint) in actuator_options {
                        option {
                            key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                            value: "{pin.label}",
                            disabled: reserved_elsewhere,
                            selected: actuator_pin.cloned() == pin.label,
                            {
                                if let Some(h) = hint {
                                    format!("{} ({}) — {}", pin.label, pin.mode.as_deref().unwrap_or("?"), h)
                                } else {
                                    format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))
                                }
                            }
                        }
                    }
                }
            }
            {
                text_field(
                    t!("flows-reg-setpoint"),
                    setpoint,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        setpoint.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.setpoint = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-deadband"),
                    deadband,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        deadband.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.deadband = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-min-on"),
                    min_on,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        min_on.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.min_on_secs = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-min-off"),
                    min_off,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        min_off.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.min_off_secs = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-sample"),
                    sample,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        sample.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.sample_ms = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-data-timeout"),
                    timeout,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        timeout.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_tt_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.data_timeout_secs = v;
                                }
                            },
                        );
                    },
                )
            }
            SafeStateSelect {
                current: safe_state,
                disabled: !can_write,
                on_change: move |v| {
                    patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            if let Some(config) = reg_tt_config_mut(node) {
                                config.safe_state = v;
                            }
                        },
                    );
                },
            }
            p { class: "text-xs text-gray-400",
                {
                    if heat {
                        t!("flows-reg-tt-heat-hint").to_string()
                    } else {
                        t!("flows-reg-tt-cool-hint").to_string()
                    }
                }
            }
        }
    }
}

/// Formulaire de la carte PID — même liaison mixte, gains + cycle relais
/// time-proportional en plus, deadband/anti court-cycle en moins (le PID
/// module un duty % sur `cycle_time_secs`).
#[component]
pub(super) fn RegPidForm(
    mut cx: EditorCx,
    initial: RegPidConfig,
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
        let list = devices.value().read().clone().unwrap_or_default();
        let Some(node_id) = cx.selected_node.cloned() else {
            return;
        };
        let wanted: Vec<i64> = g
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .and_then(|n| reg_pid_config_mut_ref(n).map(|c| c.device_id.clone()))
            .unwrap_or_default()
            .lines()
            .filter(|slug| !slug.is_empty())
            .filter_map(|slug| list.iter().find(|d| d.device_id == slug).map(|d| d.id))
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

    let mut device_id = use_signal(move || initial.device_id.clone());
    let mut sensor_pin = use_signal(move || initial.sensor_pin.clone());
    let mut actuator_pin = use_signal(move || initial.actuator_pin.clone());
    let mut setpoint = use_signal(move || v_to_string(initial.setpoint));
    let mut kp = use_signal(move || v_to_string(initial.kp));
    let mut ki = use_signal(move || v_to_string(initial.ki));
    let mut kd = use_signal(move || v_to_string(initial.kd));
    let mut cycle = use_signal(move || initial.cycle_time_secs.to_string());
    let mut sample = use_signal(move || initial.sample_ms.to_string());
    let mut timeout = use_signal(move || initial.data_timeout_secs.to_string());
    let safe_state = initial.safe_state;
    // Precomputed actuator options (no `let` inside `for` rsx): a pin
    // claimed by ANOTHER deployed flow stays listed but greyed, unless it is
    // the current selection — an inherited conflict must stay visible and
    // fixable. The deploy gate remains the hard stop server-side.
    let actuator_options: Vec<(api::pins::PinoutPin, bool, Option<String>)> =
        output_pins_of(&devices, &pins_cache, &device_id.cloned())
            .into_iter()
            .map(|pin| {
                let other_owners: Vec<String> = pin
                    .reserved_by
                    .iter()
                    .filter(|r| r.flow_id != flow_id)
                    .map(|r| r.flow_name.clone())
                    .collect();
                let is_current = actuator_pin.cloned() == pin.label;
                let disabled = !other_owners.is_empty() && !is_current;
                let hint = if disabled {
                    Some(t!("flows-pin-reserved", flow: other_owners.join(", ")))
                } else {
                    None
                };
                (pin, disabled, hint)
            })
            .collect();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-reg-help")} }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-reg-device")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let slug = event.value();
                        device_id.set(slug.clone());
                        sensor_pin.set(String::new());
                        actuator_pin.set(String::new());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_pid_config_mut(node) {
                                    config.device_id = slug;
                                    config.sensor_pin.clear();
                                    config.actuator_pin.clear();
                                }
                            },
                        );
                    },
                    option { value: "", selected: device_id.cloned().is_empty(),
                        {t!("flows-device-device-none")}
                    }
                    for device in devices.value().read().clone().unwrap_or_default() {
                        option {
                            key: "{device.id}",
                            value: "{device.device_id}",
                            selected: device_id.cloned() == device.device_id,
                            {device.device_id.clone()}
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-reg-sensor-pin")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write || device_id.cloned().is_empty(),
                    onchange: move |event| {
                        let pin = event.value();
                        sensor_pin.set(pin.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_pid_config_mut(node) {
                                    config.sensor_pin = pin;
                                }
                            },
                        );
                    },
                    option { value: "", selected: sensor_pin.cloned().is_empty(),
                        {t!("flows-device-pin-none")}
                    }
                    for pin in input_pins_of(&devices, &pins_cache, &device_id.cloned()) {
                        option {
                            key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                            value: "{pin.label}",
                            selected: sensor_pin.cloned() == pin.label,
                            {format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))}
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("flows-reg-actuator-pin")}
                }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write || device_id.cloned().is_empty(),
                    onchange: move |event| {
                        let pin = event.value();
                        actuator_pin.set(pin.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let Some(config) = reg_pid_config_mut(node) {
                                    config.actuator_pin = pin;
                                }
                            },
                        );
                    },
                    option { value: "", selected: actuator_pin.cloned().is_empty(),
                        {t!("flows-device-pin-none")}
                    }
                    for (pin, reserved_elsewhere, hint) in actuator_options {
                        option {
                            key: "{pin.gpio.unwrap_or(-1)}-{pin.label}",
                            value: "{pin.label}",
                            disabled: reserved_elsewhere,
                            selected: actuator_pin.cloned() == pin.label,
                            {
                                if let Some(h) = hint {
                                    format!("{} ({}) — {}", pin.label, pin.mode.as_deref().unwrap_or("?"), h)
                                } else {
                                    format!("{} ({})", pin.label, pin.mode.as_deref().unwrap_or("?"))
                                }
                            }
                        }
                    }
                }
            }
            {
                text_field(
                    t!("flows-reg-setpoint"),
                    setpoint,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        setpoint.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.setpoint = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-kp"),
                    kp,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        kp.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.kp = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-ki"),
                    ki,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        ki.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.ki = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-kd"),
                    kd,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        kd.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_secs(&raw),
                                ) {
                                    config.kd = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-cycle"),
                    cycle,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        cycle.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.cycle_time_secs = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-sample"),
                    sample,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        sample.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.sample_ms = v;
                                }
                            },
                        );
                    },
                )
            }
            {
                text_field(
                    t!("flows-reg-data-timeout"),
                    timeout,
                    !can_write,
                    move |event| {
                        let raw = event.value();
                        timeout.set(raw.clone());
                        patch_selected(
                            &mut cx,
                            move |node: &mut FlowNode| {
                                if let (Some(config), Some(v)) = (
                                    reg_pid_config_mut(node),
                                    parse_u32(&raw),
                                ) {
                                    config.data_timeout_secs = v;
                                }
                            },
                        );
                    },
                )
            }
            SafeStateSelect {
                current: safe_state,
                disabled: !can_write,
                on_change: move |v| {
                    patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            if let Some(config) = reg_pid_config_mut(node) {
                                config.safe_state = v;
                            }
                        },
                    );
                },
            }
            p { class: "text-xs text-gray-400", {t!("flows-reg-pid-hint")} }
        }
    }
}

/// Config TT d'un nœud (lecture seule, pour l'effet de précharge).
fn reg_tt_config_mut_ref(node: &FlowNode) -> Option<&RegTtConfig> {
    match &node.kind {
        FlowNodeKind::RegTtHeat { config } | FlowNodeKind::RegTtCool { config } => Some(config),
        _ => None,
    }
}

/// Config PID d'un nœud (lecture seule, pour l'effet de précharge).
fn reg_pid_config_mut_ref(node: &FlowNode) -> Option<&RegPidConfig> {
    match &node.kind {
        FlowNodeKind::RegPid { config } => Some(config),
        _ => None,
    }
}

/// Sélecteur d'état de repos (safe-state) partagé des cartes reg.
#[component]
fn SafeStateSelect(
    current: SafeState,
    disabled: bool,
    on_change: EventHandler<SafeState>,
) -> Element {
    let is_high = matches!(current, SafeState::High);
    rsx! {
        label { class: "block",
            span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-reg-safe-state")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled,
                onchange: move |event| {
                    let safe = if event.value() == "high" {
                        SafeState::High
                    } else {
                        SafeState::Low
                    };
                    on_change.call(safe);
                },
                option { value: "low", selected: !is_high, {t!("flows-reg-safe-low")} }
                option { value: "high", selected: is_high, {t!("flows-reg-safe-high")} }
            }
        }
    }
}
