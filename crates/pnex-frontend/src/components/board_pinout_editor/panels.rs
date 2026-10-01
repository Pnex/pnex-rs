//! Selected-pin configuration surface: right slide-over drawer and the
//! configuration panel body (existing PinCard reused as-is).

use super::*;

use super::view::{cap_color, gpio_label};
use crate::components::icons;
use crate::components::pins_panel::PinCard;

/// Panneau de configuration du pin sélectionné : PinCard existante (pin
/// provisionné), panneau réservé (écran activé) ou hint « non provisionné »
/// (pin de profil jamais annoncé). Aucun pin sélectionné → hint.
#[component]
fn PinConfigPanel(
    mut selected: Signal<Option<String>>,
    pinout_pins: Vec<api::pins::PinoutPin>,
    pins_data: Option<api::pins::PinsResponse>,
    device_pk: i64,
    connected: bool,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    let Some(label) = selected() else {
        return rsx! {
            div { class: "rounded-lg border border-gray-200 p-6 text-center text-sm text-gray-400",
                {t!("board-select-pin")}
            }
        };
    };
    let Some(pin) = pinout_pins.iter().find(|p| p.label == label).cloned() else {
        return rsx! {};
    };
    // Pin écran réservé (ou power/gnd/en sélectionné) : panneau informatif.
    let non_gpio = pin.kind.as_deref().is_some_and(|k| k != "gpio");
    if pin.reserved || non_gpio {
        return rsx! {
            div { class: "rounded-lg border border-gray-200 p-4 space-y-2",
                div { class: "flex items-center justify-between",
                    span { class: "font-semibold text-gray-900", {pin.label.clone()} }
                    span { class: "text-xs text-gray-400", {gpio_label(pin.gpio)} }
                }
                if pin.reserved {
                    div { class: "rounded-lg bg-teal-50 border border-teal-200 px-3 py-2 text-sm text-teal-800",
                        // « réservé pour l'écran » — le pin peut être
                        // réservé par un écran externe choisi, pas
                        // seulement soudé (builtin).
                        {t!("board-pin-reserved-screen")}
                    }
                    for w in pin.warnings {
                        p { class: "text-xs text-teal-700", {crate::api::error_i18n::localize_field("", &w)} }
                    }
                } else {
                    p { class: "text-sm text-gray-500", {t!("board-non-configurable")} }
                }
            }
        };
    }
    // Pin provisionné → PinCard (toute la mécanique commandes).
    let card = pins_data
        .as_ref()
        .and_then(|d| d.pins.iter().find(|p| Some(p.gpio) == pin.gpio).cloned());
    if let Some(info) = card {
        return rsx! {
            PinCard {
                key: "{info.gpio}",
                device_pk,
                pin: info,
                connected,
                can_write,
                on_changed,
            }
        };
    }
    // Pin du profil jamais provisionné (device jamais connecté).
    let (color, _key) = cap_color(&pin.fns, pin.kind.as_deref());
    rsx! {
        div { class: "rounded-lg border border-gray-200 p-4 space-y-2",
            div { class: "flex items-center justify-between",
                span { class: "font-semibold text-gray-900", {pin.label.clone()} }
                span { class: "text-xs text-gray-400", {gpio_label(pin.gpio)} }
            }
            span { class: "inline-block w-3 h-3 rounded", style: "background:{color}" }
            for w in pin.warnings {
                p { class: "text-xs text-amber-700", {crate::api::error_i18n::localize_field("", &w)} }
            }
            p { class: "text-sm text-gray-500", {t!("pins-not-provisioned")} }
        }
    }
}

/// Right drawer for the selected pin — slide-over (`w-96` aside,
/// `animate-slide-in` animation, NO backdrop: user decision 2026-09-20,
/// no page graying), closable via × / Escape when the panel has focus
/// (`InspectorPanel` pattern). The body remains owned by `PinConfigPanel`
/// — a new face, not new mechanics.
#[component]
pub(super) fn PinDrawer(
    mut selected: Signal<Option<String>>,
    pinout_pins: Vec<api::pins::PinoutPin>,
    pins_data: Option<api::pins::PinsResponse>,
    device_pk: i64,
    connected: bool,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    let Some(label) = selected() else {
        return rsx! {};
    };
    // Header: color dot of the first function + label + GPIO.
    let pin = pinout_pins.iter().find(|p| p.label == label);
    let (dot, gpio_text) = match pin {
        Some(p) => {
            let (color, _) = cap_color(&p.fns, p.kind.as_deref());
            (
                color,
                p.gpio.map(|g| format!("GPIO{g}")).unwrap_or_default(),
            )
        }
        None => ("#64748b", String::new()),
    };
    rsx! {
        aside {
            class: "fixed inset-y-0 right-0 z-40 flex w-96 max-w-full flex-col border-l border-gray-200 bg-white shadow-xl animate-slide-in",
            tabindex: "0",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    selected.set(None);
                }
            },
            div { class: "flex items-center gap-2.5 border-b border-gray-200 px-4 py-3",
                span { class: "inline-block h-2.5 w-2.5 shrink-0 rounded", style: "background:{dot}" }
                span { class: "min-w-0 truncate font-mono text-sm font-semibold text-gray-900", {label.clone()} }
                if !gpio_text.is_empty() {
                    span { class: "font-mono text-xs text-gray-400", {gpio_text} }
                }
                div { class: "flex-1" }
                button {
                    class: "shrink-0 rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 hover:text-gray-600",
                    title: t!("board-drawer-close"),
                    onclick: move |_| selected.set(None),
                    icons::X { class: "h-4 w-4" }
                }
            }
            div { class: "flex-1 overflow-y-auto p-4",
                PinConfigPanel {
                    selected,
                    pinout_pins,
                    pins_data,
                    device_pk,
                    connected,
                    can_write,
                    on_changed,
                }
            }
        }
    }
}
