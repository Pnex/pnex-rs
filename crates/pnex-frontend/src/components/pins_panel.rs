//! Panneau « Pins » des devices génériques (Brick 0, brick0.md §7).
//!
//! Grille des pins depuis `GET /devices/{id}/pins` (polling 15 s — pattern
//! dashboard), selects mode/safe-state + toggles write + cadences de
//! lecture : chaque action est un POST /commands **manuel** (D17 — bouton,
//! jamais d'automatisme serveur), validée par chip-caps côté backend avant
//! push (400 + raison relayée en toast).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::modal::Modal;
use crate::state::toasts;
use crate::util::sleep;
use std::time::Duration;

/// Un pin affiché : données GET /pins + sélections locales (effectives).
/// Pub(crate) : réutilisée comme panneau de configuration par l'éditeur SVG
/// (board_pinout_editor).
#[component]
pub(crate) fn PinCard(
    device_pk: i64,
    pin: api::pins::PinInfo,
    connected: bool,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    // Valeurs effectives : le select AFFICHE sa sélection (piège des
    // selects contrôlés — leçon visualisation 2026-08-19).
    let mode_init = pin.mode.clone();
    let safe_init = pin.safe_state.clone();
    let mut mode_sel = use_signal(move || mode_init);
    let mut safe_sel = use_signal(move || safe_init);
    // Cadence effective (persistée en base, re-poussée au device après
    // chaque reconnexion) — le select affiche l'état réel, pas « manuel ».
    let interval_init = pin.interval_ms.unwrap_or(0).to_string();
    let mut interval_sel = use_signal(move || interval_init);
    let mut busy = use_signal(|| false);

    // Garde « config device ↔ flows déployés » : la commande set_mode
    // refusée 409 stocke la liste des flows, le modal propose l'arrêt
    // confirmé puis ré-envoie avec `confirm_stop_flows: true`.
    let mut conflict_flows = use_signal(|| Vec::<(i64, String)>::new());
    let mut pending_cmd = use_signal(|| None::<api::pins::Command>);

    let is_digital = pin.mode == "digital_in" || pin.mode == "digital_out";
    let is_pwm = pin.mode == "pwm_out";
    let is_output = pin.mode == "digital_out" || pin.mode == "pwm_out";
    // Analog pins are configurable too (A0 stays analog-only): their select
    // is driven by `available_modes` (chip caps exposed by GET /pins).
    let is_analog = pin.mode == "analog_in";
    let is_configurable = is_digital || is_pwm || is_analog;
    // Flow write reservations (one write source per output): all owning
    // flows are listed — the UI greys the manual write controls and the
    // server enforces the same rule (409).
    let is_reserved = !pin.reserved_by.is_empty();
    let reserved_names = pin
        .reserved_by
        .iter()
        .map(|r| r.flow_name.clone())
        .collect::<Vec<_>>()
        .join(", ");

    // Mode options (base naming — the REST command takes `analog_in`, never
    // the wire string `adc_in`): the chip-cap list served with the pin; a pin
    // the chip caps allow nothing else for keeps its current mode only.
    let mut mode_options: Vec<String> = pin
        .available_modes
        .iter()
        .map(|m| {
            if m == "adc_in" {
                "analog_in".into()
            } else {
                m.clone()
            }
        })
        .collect();
    if mode_options.is_empty() {
        mode_options.push(pin.mode.clone());
    }
    let role_class = if pin.role == "actuator" {
        "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-purple-100 text-purple-800"
    } else {
        "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-blue-100 text-blue-800"
    };
    let last_label = match &pin.last_value {
        Some(serde_json::Value::Bool(b)) => {
            if *b {
                t!("pins-high")
            } else {
                t!("pins-low")
            }
        }
        Some(v) => v.to_string(),
        None => "—".into(),
    };

    // Toggle digital_out : libellé = action suivante (HIGH ↔ LOW).
    let (toggle_label, toggle_class) = match pin.last_value {
        Some(serde_json::Value::Bool(true)) => {
            (t!("pins-write-low"), "bg-red-600 hover:bg-red-700")
        }
        _ => (t!("pins-write-high"), "bg-green-600 hover:bg-green-700"),
    };

    // Envoi d'une commande (busy + toast erreur relayée + refresh parent).
    let mut send = move |cmd: api::pins::Command| {
        if cmd.op == "set_mode" {
            // Mémorisé pour le re-envoi confirmé du modal (confirm flag).
            pending_cmd.set(Some(cmd.clone()));
        }
        busy.set(true);
        let pk = device_pk;
        spawn(async move {
            let outcome = api::pins::command(pk, cmd).await;
            busy.set(false);
            match outcome {
                Ok(api::pins::CommandOutcome::Sent(body)) => {
                    // Un set_mode peut avoir arrêté des flows déployés lisant
                    // ce pin (Phase 6) — l'utilisateur est prévenu explicitement.
                    if let Some(impacts) = body["flow_impacts"].as_array() {
                        let names: Vec<String> = impacts
                            .iter()
                            .filter_map(|f| f["name"].as_str().map(str::to_string))
                            .collect();
                        if !names.is_empty() {
                            toasts::info(
                                t!("pins-flows-stopped", names: names.join(", ")).to_string(),
                            );
                        }
                    }
                    // L'état remonte par StateReport → visible au prochain poll.
                    sleep(Duration::from_millis(300)).await;
                    on_changed.call(());
                }
                Ok(api::pins::CommandOutcome::Conflict { flows }) => {
                    conflict_flows.set(flows);
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    rsx! {
        div { class: if connected { "rounded-lg border border-gray-200 p-4 space-y-3" } else { "rounded-lg border border-gray-200 p-4 space-y-3 opacity-90" },
            // Lecture seule explicite quand le device est offline : les
            // contrôles sont déjà désactivés (`disabled: busy() || !connected`),
            // le bandeau dit pourquoi.
            if !connected {
                div { class: "flex items-start gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-800",
                    span { class: "mt-0.5 shrink-0", {"ℹ"} }
                    p { {t!("pins-offline-readonly")} }
                }
            }
            div { class: "flex items-center justify-between",
                div {
                    span { class: "font-semibold text-gray-900", {pin.label.clone()} }
                    span { class: "ml-2 text-xs text-gray-400", "GPIO{pin.gpio}" }
                }
                span { class: "{role_class}", {role_label(&pin.role)} }
            }

            // Dernière valeur (mémoire de session — « — » si offline).
            div { class: "flex items-center justify-between text-sm",
                span { class: "text-gray-500", {t!("pins-last-value")} }
                span { class: "font-mono font-semibold text-gray-900", {last_label} }
            }

            // Mode + safe-state (any configurable pin — digital, pwm, and
            // analog now; A0 offers only analog_in via available_modes).
            if is_configurable && can_write {
                div { class: "grid grid-cols-2 gap-2",
                    div { class: "space-y-1",
                        label {
                            r#for: "pins-panel-field-1",
                            class: "text-xs font-medium text-gray-500",
                            {t!("pins-mode")}
                        }
                        select {
                            id: "pins-panel-field-1",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                            aria_label: t!("pins-mode"),
                            value: "{mode_sel}",
                            disabled: busy() || !connected,
                            onchange: move |e| mode_sel.set(e.value()),
                            for m in mode_options.clone() {
                                option {
                                    key: "{m}",
                                    value: "{m}",
                                    selected: mode_sel() == m,
                                    {mode_label(&m)}
                                }
                            }
                        }
                    }
                    div { class: "space-y-1",
                        label {
                            r#for: "pins-panel-field-2",
                            class: "text-xs font-medium text-gray-500",
                            {t!("pins-safe-state")}
                        }
                        select {
                            id: "pins-panel-field-2",
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                            aria_label: t!("pins-safe-state"),
                            value: "{safe_sel}",
                            disabled: busy() || !connected,
                            onchange: move |e| safe_sel.set(e.value()),
                            option { value: "low", selected: safe_sel() == "low",
                                {t!("pins-safe-low")}
                            }
                            option { value: "high", selected: safe_sel() == "high",
                                {t!("pins-safe-high")}
                            }
                        }
                    }
                }
            }

            // Appliquer mode + safe-state (set_mode manuel).
            if is_configurable && can_write {
                button {
                    class: "w-full px-3 py-1.5 text-xs font-medium text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                    disabled: busy() || !connected,
                    onclick: move |_| {
                        let cmd = api::pins::Command {
                            op: "set_mode",
                            gpio: pin.gpio as u16,
                            mode: Some({
                                let m = mode_sel().clone();
                                match m.as_str() {
                                    "pwm_out" => "pwm_out",
                                    "digital_out" => "digital_out",
                                    "analog_in" => "analog_in",
                                    _ => "digital_in",
                                }
                            }),
                            safe_state: Some(if safe_sel() == "high" { "high" } else { "low" }),
                            value: None,
                            interval_ms: None,
                            confirm_stop_flows: None,
                        };
                        send(cmd);
                    },
                    {t!("pins-apply-mode")}
                }
            }

            // Actions par rôle.
            div { class: "flex flex-wrap items-center gap-2",
                // digital_out : toggle HIGH/LOW (write manuel, D17).
                if is_output && can_write {
                    {
                        let next_high = !matches!(pin.last_value, Some(serde_json::Value::Bool(true)));
                        let cmd = api::pins::Command {
                            op: "write",
                            gpio: pin.gpio as u16,
                            mode: None,
                            safe_state: None,
                            value: Some(serde_json::Value::Bool(next_high)),
                            interval_ms: None,
                            confirm_stop_flows: None,
                        };
                        rsx! {
                            button {
                                class: if is_reserved { "px-3 py-1.5 text-xs font-semibold text-white rounded-lg transition-colors opacity-40 {toggle_class}" } else { "px-3 py-1.5 text-xs font-semibold text-white rounded-lg transition-colors {toggle_class}" },
                                disabled: busy() || !connected || is_reserved,
                                onclick: move |_| send(cmd.clone()),
                                {toggle_label}
                            }
                        }
                    }
                }
                // pwm_out : duty 0-100 (write manuel).
                if is_pwm && can_write {
                    {
                        let duty_init = match pin.last_value {
                            Some(serde_json::Value::Number(n)) => {
                                n.as_f64().unwrap_or(0.0).round() as u32
                            }
                            _ => 0,
                        };
                        rsx! {
                            DutyControl {
                                device_pk,
                                gpio: pin.gpio,
                                initial: duty_init,
                                connected,
                                disabled: is_reserved,
                            }
                        }
                    }
                }
                // Reserved hint: manual writes are flow-owned (one write
                // source per output) — the server enforces the same rule.
                if is_output && can_write && is_reserved {
                    span { class: "basis-full text-xs text-amber-600",
                        {t!("pins-reserved-by-flow", flow : reserved_names)}
                    }
                }
                // Input (digital_in/analog_in) : cadence de lecture.
                if !is_output && can_write {
                    select {
                        class: "px-2 py-1.5 border border-gray-300 rounded-lg text-xs",
                        aria_label: t!("pins-read-interval"),
                        value: "{interval_sel}",
                        disabled: busy() || !connected,
                        onchange: move |e| interval_sel.set(e.value()),
                        option { value: "0", selected: interval_sel() == "0",
                            {t!("pins-subscribe-off")}
                        }
                        option { value: "1000", selected: interval_sel() == "1000",
                            {t!("pins-subscribe-1s")}
                        }
                        option { value: "5000", selected: interval_sel() == "5000",
                            {t!("pins-subscribe-5s")}
                        }
                        option {
                            value: "15000",
                            selected: interval_sel() == "15000",
                            {t!("pins-subscribe-15s")}
                        }
                        option {
                            value: "60000",
                            selected: interval_sel() == "60000",
                            {t!("pins-subscribe-60s")}
                        }
                    }
                    button {
                        class: "px-3 py-1.5 text-xs font-medium text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                        disabled: busy() || !connected,
                        onclick: move |_| {
                            let interval_ms: u32 = interval_sel().parse().unwrap_or(0);
                            let cmd = api::pins::Command {
                                op: "subscribe",
                                gpio: pin.gpio as u16,
                                mode: None,
                                safe_state: None,
                                value: None,
                                interval_ms: Some(interval_ms),
                                confirm_stop_flows: None,
                            };
                            send(cmd);
                        },
                        {t!("pins-apply")}
                    }
                }
            }
            // Garde « arrêter les flows puis appliquer » : liste des flows
            // déployés touchés + confirmation explicite (jamais d'arrêt
            // silencieux).
            if !conflict_flows.cloned().is_empty() {
                Modal {
                    title: t!("pins-flows-conflict-title").to_string(),
                    max_width: "max-w-md".to_string(),
                    on_close: move |_| {
                        conflict_flows.set(Vec::new());
                        pending_cmd.set(None);
                    },
                    div { class: "space-y-3",
                        p { class: "text-sm text-gray-600", {t!("pins-flows-conflict-message")} }
                        ul { class: "list-disc pl-5 text-sm text-gray-800",
                            for (_, name) in conflict_flows.cloned() {
                                li { key: "{name}", {name} }
                            }
                        }
                        div { class: crate::components::modal::MODAL_FOOTER,
                            button {
                                class: "px-3 py-1.5 text-sm text-gray-600 hover:text-gray-800",
                                onclick: move |_| {
                                    conflict_flows.set(Vec::new());
                                    pending_cmd.set(None);
                                    busy.set(false);
                                },
                                {t!("common-cancel")}
                            }
                        }
                        button {
                            class: "w-full px-3 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 transition-colors",
                            onclick: move |_| {
                                if let Some(mut cmd) = pending_cmd.cloned() {
                                    cmd.confirm_stop_flows = Some(true);
                                    conflict_flows.set(Vec::new());
                                    pending_cmd.set(None);
                                    send(cmd);
                                } else {
                                    conflict_flows.set(Vec::new());
                                    busy.set(false);
                                }
                            },
                            {t!("pins-flows-conflict-confirm")}
                        }
                    }
                }
            }
        }
    }
}

/// PWM duty control (0-100): numeric input + Apply button — manual write
/// (D17). `disabled` greys the whole control when the pin is flow-owned
/// (one write source per output); the server enforces the same rule (409).
#[component]
fn DutyControl(
    device_pk: i64,
    gpio: i32,
    initial: u32,
    connected: bool,
    disabled: bool,
) -> Element {
    let mut duty = use_signal(move || initial.to_string());
    let mut busy = use_signal(|| false);
    rsx! {
        div { class: if disabled { "flex items-center gap-1 opacity-40" } else { "flex items-center gap-1" },
            input {
                r#type: "number",
                class: "w-16 px-2 py-1.5 border border-gray-300 rounded-lg text-xs",
                min: "0",
                max: "100",
                value: "{duty}",
                disabled: busy() || !connected || disabled,
                oninput: move |e| duty.set(e.value()),
            }
            button {
                class: "px-3 py-1.5 text-xs font-medium text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                disabled: busy() || !connected || disabled,
                onclick: move |_| {
                    let v = duty.cloned().parse::<f64>().unwrap_or(-1.0);
                    if !(0.0..=100.0).contains(&v) {
                        toasts::error(t!("pins-duty-range").to_string());
                        return;
                    }
                    let cmd = api::pins::Command {
                        op: "write",
                        gpio: gpio as u16,
                        mode: None,
                        safe_state: None,
                        value: Some(serde_json::json!(v.round() as u32)),
                        interval_ms: None,
                        confirm_stop_flows: None,
                    };
                    busy.set(true);
                    let pk = device_pk;
                    spawn(async move {
                        let res = api::pins::command(pk, cmd).await;
                        busy.set(false);
                        match res {
                            Ok(_) => {
                                sleep(Duration::from_millis(300)).await;
                            }
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                {t!("pins-duty-apply")}
            }
        }
    }
}

/// Libellé du rôle dérivé (sensor/actuator — B0.6, pas de colonne role).
fn role_label(role: &str) -> String {
    if role == "actuator" {
        t!("pins-role-actuator")
    } else {
        t!("pins-role-sensor")
    }
}

/// Localized label for a mode option (base naming, `analog_in` included).
fn mode_label(mode: &str) -> String {
    match mode {
        "digital_in" => t!("pins-mode-in"),
        "digital_out" => t!("pins-mode-out"),
        "pwm_out" => t!("pins-mode-pwm"),
        "analog_in" => t!("pins-mode-adc"),
        other => other.to_string(),
    }
}

/// Panneau complet : polling 15 s (pattern dashboard), carte par pin.
#[component]
pub fn PinsPanel(device_pk: i64, can_write: bool) -> Element {
    const POLL_SECS: u64 = 15;
    let mut reload = use_signal(|| 0u32);

    let pins = use_resource(move || async move {
        let _ = reload();
        api::pins::pins(device_pk).await
    });

    // Polling auto-entretenu tant que le panneau est monté.
    let mut polling = use_signal(|| false);
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    let data = match pins.value().read().as_ref() {
        // pending → rien ; erreur → dégradation silencieuse (le device
        // n'est peut-être pas générique : le panneau n'est rendu que là).
        Some(Err(_)) | None => None,
        Some(Ok(response)) => Some(response.clone()),
    };
    let connected = data.as_ref().is_some_and(|d| d.connected);
    let list = data.map(|d| d.pins).unwrap_or_default();
    let on_changed = Callback::new(move |_: ()| reload.with_mut(|r| *r += 1));
    // Alerte « rien ne publiera » : device connecté avec des pins en entrée
    // mais aucune souscription (interval) — le firmware générique ne publie
    // que les pins souscrits, les flows qui lisent ces pins liront du vide
    // (payload `{}`) sans autre symptôme. Les pins en sortie sont exclus :
    // pas de souscription attendue.
    let input_pins = list.iter().filter(|p| !p.mode.ends_with("_out")).count();
    let subscribed_inputs = list
        .iter()
        .filter(|p| !p.mode.ends_with("_out") && p.interval_ms.is_some_and(|ms| ms > 0))
        .count();
    let silent_device = connected && input_pins > 0 && subscribed_inputs == 0;

    rsx! {
        div { class: "p-6 border-t border-gray-200",
            div { class: "flex items-center justify-between mb-3",
                h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider",
                    {t!("pins-title")}
                }
                div { class: "flex items-center gap-3",
                    if connected {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800",
                            {t!("pins-connected")}
                        }
                    } else {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-600",
                            {t!("pins-offline")}
                        }
                    }
                    span { class: "text-xs text-gray-400", {t!("pins-auto-refresh")} }
                }
            }
            if silent_device {
                div { class: "mb-3 flex items-start gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800",
                    span { class: "mt-0.5 shrink-0", {"⚠"} }
                    p { {t!("pins-none-subscribed")} }
                }
            }
            if list.is_empty() {
                p { class: "text-sm text-gray-500", {t!("pins-not-provisioned")} }
            } else {
                div { class: "grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3",
                    for pin in list {
                        PinCard {
                            key: "{pin.gpio}",
                            device_pk,
                            pin,
                            connected,
                            can_write,
                            on_changed,
                        }
                    }
                }
            }
        }
    }
}
