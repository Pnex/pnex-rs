//! Flash LED switch of an ESP32-CAM (O15): drives the onboard flash LED
//! (GPIO4) through the regular pin command path (`POST /devices/{id}/commands`,
//! same as the device Pins panel) — the server keeps the one-write-source
//! rule (a flow-reserved pin is refused with 409, the UI greys it first).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::state::toasts;
use crate::util::sleep;

/// GPIO of the AI-Thinker ESP32-CAM onboard flash LED.
const FLASH_GPIO: i32 = 4;

/// Flash toggle for one camera device. Hidden while the pins load; shows a
/// short hint when the device has no flash pin, is offline, or when a flow
/// owns the pin.
#[component]
pub(super) fn FlashToggle(device: i64) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut busy = use_signal(|| false);
    // Last state successfully written from this switch (the pin value may
    // only come back with the next state report).
    let mut written = use_signal(|| None::<bool>);
    // Flows that blocked the switch to output mode (409 on set_mode).
    let mut conflict = use_signal(Vec::<String>::new);

    let pins = use_resource(move || async move {
        let _ = reload();
        api::pins::pins(device).await
    });

    let (connected, pin) = match &*pins.value().read() {
        None => return rsx! {},
        Some(Err(_)) => (false, None),
        Some(Ok(res)) => (
            res.connected,
            res.pins.iter().find(|p| p.gpio == FLASH_GPIO).cloned(),
        ),
    };
    let Some(pin) = pin else {
        return rsx! {
            p { class: "text-xs text-gray-500", {t!("cameras-flash-unavailable")} }
        };
    };

    let mut send = move |cmd: api::pins::Command, next: Option<bool>| {
        busy.set(true);
        spawn(async move {
            let outcome = api::pins::command(device, cmd).await;
            busy.set(false);
            match outcome {
                Ok(api::pins::CommandOutcome::Sent(_)) => {
                    if next.is_some() {
                        written.set(next);
                    }
                    conflict.set(Vec::new());
                    sleep(std::time::Duration::from_millis(300)).await;
                    reload.with_mut(|r| *r += 1);
                }
                Ok(api::pins::CommandOutcome::Conflict { flows }) => {
                    conflict.set(flows.into_iter().map(|(_, name)| name).collect());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    let reserved_names = pin
        .reserved_by
        .iter()
        .map(|r| r.flow_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let is_reserved = !reserved_names.is_empty();
    let is_output = pin.mode == "digital_out";
    let is_on = written().unwrap_or(matches!(
        pin.last_value,
        Some(serde_json::Value::Bool(true))
    ));
    let disabled = busy() || !connected || is_reserved;
    let conflict_names = conflict().join(", ");

    rsx! {
        div { class: "flex flex-wrap items-center gap-3 pt-3",
            span { class: "text-sm font-medium text-gray-700", {t!("cameras-flash")} }
            if is_output {
                button {
                    r#type: "button",
                    role: "switch",
                    "aria-checked": if is_on { "true" } else { "false" },
                    title: if is_on { t!("cameras-flash-turn-off") } else { t!("cameras-flash-turn-on") },
                    class: if is_on { "relative inline-flex h-6 w-11 items-center rounded-full bg-amber-500 transition-colors disabled:opacity-40" } else { "relative inline-flex h-6 w-11 items-center rounded-full bg-gray-300 transition-colors disabled:opacity-40" },
                    disabled,
                    onclick: move |_| {
                        let next = !is_on;
                        let cmd = api::pins::Command {
                            op: "write",
                            gpio: FLASH_GPIO as u16,
                            mode: None,
                            safe_state: None,
                            value: Some(serde_json::Value::Bool(next)),
                            interval_ms: None,
                            confirm_stop_flows: None,
                        };
                        send(cmd, Some(next));
                    },
                    span { class: if is_on { "inline-block h-5 w-5 translate-x-5 rounded-full bg-white shadow transition-transform" } else { "inline-block h-5 w-5 translate-x-0.5 rounded-full bg-white shadow transition-transform" } }
                }
                span { class: "text-xs text-gray-500",
                    if is_on {
                        {t!("cameras-flash-on")}
                    } else {
                        {t!("cameras-flash-off")}
                    }
                }
            } else {
                // Pin still configured as an input (provisioned before the
                // board profile made it an output): one click switches it.
                button {
                    r#type: "button",
                    class: "px-3 py-1.5 text-xs font-medium text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 disabled:opacity-40",
                    disabled,
                    onclick: move |_| {
                        let cmd = api::pins::Command {
                            op: "set_mode",
                            gpio: FLASH_GPIO as u16,
                            mode: Some("digital_out"),
                            safe_state: Some("low"),
                            value: None,
                            interval_ms: None,
                            confirm_stop_flows: None,
                        };
                        send(cmd, None);
                    },
                    {t!("cameras-flash-enable")}
                }
            }
            if !connected {
                span { class: "basis-full text-xs text-gray-500", {t!("cameras-flash-offline")} }
            }
            if is_reserved {
                span { class: "basis-full text-xs text-amber-600",
                    {t!("cameras-flash-reserved", flow : reserved_names.clone())}
                }
            }
            if !conflict_names.is_empty() {
                span { class: "basis-full text-xs text-amber-600",
                    {t!("cameras-flash-mode-conflict", flows : conflict_names.clone())}
                }
            }
        }
    }
}
