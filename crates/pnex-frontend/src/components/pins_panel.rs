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
use std::time::Duration;
use crate::util::sleep;
use crate::state::toasts;

/// Un pin affiché : données GET /pins + sélections locales (effectives).
#[component]
fn PinCard(device_pk: i64, pin: api::pins::PinInfo, connected: bool, can_write: bool, on_changed: Callback<()>) -> Element {
    // Valeurs effectives : le select AFFICHE sa sélection (piège des
    // selects contrôlés — leçon visualisation 2026-08-19).
    let mode_init = pin.mode.clone();
    let safe_init = pin.safe_state.clone();
    let mut mode_sel = use_signal(move || mode_init);
    let mut safe_sel = use_signal(move || safe_init);
    let mut interval_sel = use_signal(|| "0".to_string());
    let mut busy = use_signal(|| false);

    let is_digital = pin.mode == "digital_in" || pin.mode == "digital_out";
    let is_output = pin.mode == "digital_out";
    let role_class = if pin.role == "actuator" {
        "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-purple-100 text-purple-800"
    } else {
        "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-blue-100 text-blue-800"
    };
    let last_label = match &pin.last_value {
        Some(serde_json::Value::Bool(b)) => {
            if *b { t!("pins-high") } else { t!("pins-low") }
        }
        Some(v) => v.to_string(),
        None => "—".into(),
    };

    // Toggle digital_out : libellé = action suivante (HIGH ↔ LOW).
    let (toggle_label, toggle_class) = match pin.last_value {
        Some(serde_json::Value::Bool(true)) => (t!("pins-write-low"), "bg-red-600 hover:bg-red-700"),
        _ => (t!("pins-write-high"), "bg-green-600 hover:bg-green-700"),
    };

    // Envoi d'une commande (busy + toast erreur relayée + refresh parent).
    let mut send = move |cmd: api::pins::Command| {
        busy.set(true);
        let pk = device_pk;
        spawn(async move {
            let outcome = api::pins::command(pk, cmd).await;
            busy.set(false);
            match outcome {
                Ok(()) => {
                    // L'état remonte par StateReport → visible au prochain poll.
                    sleep(Duration::from_millis(300)).await;
                    on_changed.call(());
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };

    rsx! {
        div { class: "rounded-lg border border-gray-200 p-4 space-y-3",
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

            // Mode + safe-state (pins digitaux ; A0 reste analog_in).
            if is_digital && can_write {
                div { class: "grid grid-cols-2 gap-2",
                    div { class: "space-y-1",
                        label { class: "text-xs font-medium text-gray-500", {t!("pins-mode")} }
                        select {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                            value: "{mode_sel}",
                            disabled: busy() || !connected,
                            onchange: move |e| mode_sel.set(e.value()),
                            option { value: "digital_in", selected: mode_sel() == "digital_in", {t!("pins-mode-in")} }
                            option { value: "digital_out", selected: mode_sel() == "digital_out", {t!("pins-mode-out")} }
                        }
                    }
                    div { class: "space-y-1",
                        label { class: "text-xs font-medium text-gray-500", {t!("pins-safe-state")} }
                        select {
                            class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                            value: "{safe_sel}",
                            disabled: busy() || !connected,
                            onchange: move |e| safe_sel.set(e.value()),
                            option { value: "low", selected: safe_sel() == "low", {t!("pins-safe-low")} }
                            option { value: "high", selected: safe_sel() == "high", {t!("pins-safe-high")} }
                        }
                    }
                }
            }

            // Appliquer mode + safe-state (set_mode manuel).
            if is_digital && can_write {
                button {
                    class: "w-full px-3 py-1.5 text-xs font-medium text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                    disabled: busy() || !connected,
                    onclick: move |_| {
                        let cmd = api::pins::Command {
                            op: "set_mode",
                            gpio: pin.gpio as u16,
                            mode: Some(if mode_sel() == "digital_out" { "digital_out" } else { "digital_in" }),
                            safe_state: Some(if safe_sel() == "high" { "high" } else { "low" }),
                            value: None,
                            interval_ms: None,
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
                        };
                        rsx! {
                            button {
                                class: "px-3 py-1.5 text-xs font-semibold text-white rounded-lg transition-colors {toggle_class}",
                                disabled: busy() || !connected,
                                onclick: move |_| send(cmd.clone()),
                                {toggle_label}
                            }
                        }
                    }
                }
                // Input (digital_in/analog_in) : cadence de lecture.
                if !is_output && can_write {
                    select {
                        class: "px-2 py-1.5 border border-gray-300 rounded-lg text-xs",
                        value: "{interval_sel}",
                        disabled: busy() || !connected,
                        onchange: move |e| interval_sel.set(e.value()),
                        option { value: "0", selected: interval_sel() == "0", {t!("pins-subscribe-off")} }
                        option { value: "1000", selected: interval_sel() == "1000", {t!("pins-subscribe-1s")} }
                        option { value: "5000", selected: interval_sel() == "5000", {t!("pins-subscribe-5s")} }
                        option { value: "15000", selected: interval_sel() == "15000", {t!("pins-subscribe-15s")} }
                        option { value: "60000", selected: interval_sel() == "60000", {t!("pins-subscribe-60s")} }
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
                            };
                            send(cmd);
                        },
                        {t!("pins-apply")}
                    }
                }
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

    rsx! {
        div { class: "p-6 border-t border-gray-200",
            div { class: "flex items-center justify-between mb-3",
                h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider", {t!("pins-title")} }
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
            if list.is_empty() {
                p { class: "text-sm text-gray-500",
                    {t!("pins-not-provisioned")}
                }
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
