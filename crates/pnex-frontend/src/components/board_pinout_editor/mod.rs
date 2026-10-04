//! Éditeur de pinout SVG (profils board v2) — remplace la grille de cartes
//! `PinsPanel` pour les devices génériques.
//!
//! La carte est dessinée depuis `GET /pinout` (section `board` + pins
//! positionnées, enrichissement chip-caps calculé serveur) ; la sélection
//! d'un pin affiche la `PinCard` existante comme panneau de configuration
//! (toute la mécanique set_mode/write/subscribe/409 est réutilisée telle
//! quelle — un nouveau visage, pas une nouvelle mécanique). Without a v2
//! profile (`board: null`), falls back to `PinsPanel`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::state::toasts;
use crate::util::sleep;
use std::time::Duration;

mod panels;
mod preview;
mod screens;
mod svg;
mod view;

pub use preview::BoardPreviewModal;

pub(crate) use screens::*;

use panels::PinDrawer;
use svg::{BoardSvg, Legend};
use view::{board_ratio, layout_per_side, legend_of, pin_views, svg_height, svg_width};

/// En-tête : nom de la variante + chip + badge connexion + picker écran.
#[component]
fn BoardHeader(
    pinout: Option<api::pins::Pinout>,
    device_pk: i64,
    mut screen_busy: Signal<bool>,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    let board = pinout.as_ref().and_then(|p| p.board.clone());
    let Some(board) = board else {
        return rsx! {};
    };
    let screens = board
        .peripherals
        .get("screens")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let options = screen_options(&board);
    let title = board
        .pretty_name
        .clone()
        .or(board.name.clone())
        .unwrap_or_default();
    let connected = pinout.as_ref().is_some_and(|p| p.connected);
    rsx! {
        div { class: "flex flex-wrap items-center justify-between gap-2 mb-3",
            div { class: "flex items-center gap-2",
                h3 { class: "text-sm font-semibold text-gray-500 uppercase tracking-wider",
                    {title}
                }
                if let Some(chip) = &board.chip_label {
                    span { class: "text-xs font-mono text-gray-400", {chip.clone()} }
                }
                if connected {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-100 text-green-800",
                        {t!("pins-connected")}
                    }
                } else {
                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-gray-100 text-gray-600",
                        {t!("pins-offline")}
                    }
                }
            }
            if !screens.is_empty() {
                div { class: "flex flex-wrap items-center gap-1",
                    span { class: "text-xs text-gray-400 mr-1", {t!("board-screen-toggle")} }
                    for opt in options {
                        button {
                            key: "{opt.label}",
                            class: if opt.active { "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium bg-teal-600 text-white hover:bg-teal-700 disabled:opacity-50" } else { "inline-flex items-center gap-1 px-3 py-1.5 rounded-lg text-xs font-medium border border-teal-300 text-teal-700 hover:bg-teal-50 disabled:opacity-50" },
                            disabled: screen_busy() || !can_write || opt.locked,
                            title: if opt.locked { t!("board-screen-locked").to_string() } else { String::new() },
                            onclick: move |_| {
                                if opt.active {
                                    return;
                                }
                                let kind = opt.kind.clone();
                                let label = opt.label.clone();
                                let is_none = opt.kind.is_none();
                                let on_changed = on_changed;
                                screen_busy.set(true);
                                spawn(async move {
                                    match set_screen(device_pk, kind).await {
                                        Ok(()) => {
                                            let state_label = if is_none {
                                                t!("board-screen-off").to_string()
                                            } else {
                                                label
                                            };
                                            toasts::info(
                                                t!("board-screen-rebuild-toast", state : state_label).to_string(),
                                            );
                                            on_changed.call(());
                                        }
                                        Err(msg) => toasts::error(msg),
                                    }
                                    screen_busy.set(false);
                                });
                            },
                            {opt.label.clone()}
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub fn BoardPinoutEditor(device_pk: i64, can_write: bool) -> Element {
    const POLL_SECS: u64 = 15;
    let mut reload = use_signal(|| 0u32);
    let selected = use_signal(|| None::<String>);
    let screen_busy = use_signal(|| false);

    let pins_resource = use_resource(move || async move {
        let _ = reload();
        api::pins::pins(device_pk).await
    });
    let pinout_resource = use_resource(move || async move {
        let _ = reload();
        api::pins::pinout(device_pk).await
    });

    // Polling auto-entretenu (pattern PinsPanel).
    let mut polling = use_signal(|| false);
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    let data = match pins_resource.value().read().as_ref() {
        Some(Err(_)) | None => None,
        Some(Ok(response)) => Some(response.clone()),
    };
    let pinout = match pinout_resource.value().read().as_ref() {
        Some(Err(_)) | None => None,
        Some(Ok(response)) => Some(response.clone()),
    };

    // No v2 profile → legacy card grid (v1 boards).
    if pinout.as_ref().is_some_and(|p| p.board.is_none()) {
        return rsx! {
            crate::components::pins_panel::PinsPanel { device_pk, can_write }
        };
    }

    let connected = pinout.as_ref().is_some_and(|p| p.connected);
    let on_changed = Callback::new(move |_: ()| reload.with_mut(|r| *r += 1));
    // Légende de câblage de l'écran choisi (rôle → broche du profil).
    let wiring = pinout.as_ref().and_then(|p| {
        let kind = p.board.as_ref().and_then(screen_state_kind);
        p.board
            .as_ref()
            .and_then(|b| screen_wiring(b, &p.pins, kind.as_deref()))
    });

    rsx! {
        div { class: "p-6",
            BoardHeader {
                pinout: pinout.clone(),
                device_pk,
                screen_busy,
                can_write,
                on_changed,
            }
            if let Some((name, rows)) = wiring {
                ScreenWiring { name, rows }
            }
            BoardLayout {
                pinout,
                pins_data: data,
                selected,
                device_pk,
                connected,
                can_write,
                on_changed,
            }
        }
    }
}

/// Layout: full-width SVG board, selected-pin configuration in a right
/// drawer (`pnex-device-redesign.html` mockup).
#[component]
fn BoardLayout(
    pinout: Option<api::pins::Pinout>,
    pins_data: Option<api::pins::PinsResponse>,
    mut selected: Signal<Option<String>>,
    device_pk: i64,
    connected: bool,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    let Some(pinout) = pinout else {
        return rsx! {};
    };
    let Some(board) = pinout.board.clone() else {
        return rsx! {};
    };
    let views = pin_views(&board, &pinout.pins, selected().as_deref());
    let per_side = layout_per_side(&board.layout);
    let ratio = board_ratio(&board.layout);
    let svg_h = svg_height(per_side);
    let svg_w = svg_width(per_side, ratio);
    // Keep the rendered scale of the former fixed-width board (480 px for
    // a 550-unit viewBox): a wider PCB widens the SVG instead of shrinking it.
    let svg_max_w = (svg_w * 480.0 / 550.0).round();
    // Chip label: chip_label → soc → pretty_name (pinout payload).
    let chip_label = board
        .chip_label
        .clone()
        .or(board.soc.clone())
        .or(board.pretty_name.clone())
        .unwrap_or_default();
    let legend_items = legend_of(&views);

    // Export SVG autonome (fichier partageable/imprimable) — le dessin est
    // déjà un `<svg>` autonome (viewBox + attributs), on sérialise le DOM.
    let on_export = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                if let Some(doc) = window.document() {
                    if let Some(el) = doc.get_element_by_id("board-pinout-svg") {
                        let svg = el.outer_html();
                        let name = format!(
                            "pinout-{}.svg",
                            board.profile_id.clone().unwrap_or_else(|| "board".into())
                        );
                        crate::util::save_blob(&name, svg.as_bytes());
                    }
                }
            }
        }
    };

    rsx! {
        div { class: "space-y-4",
            div { class: "flex justify-end",
                button {
                    class: "inline-flex items-center gap-1 rounded-lg border border-gray-300 bg-white px-2.5 py-1 text-xs font-medium text-gray-600 hover:border-blue-400 hover:text-blue-600 transition-colors",
                    r#type: "button",
                    onclick: on_export,
                    {t!("board-export-svg")}
                }
            }
            div { class: "overflow-x-auto",
                svg {
                    id: "board-pinout-svg",
                    // Phone: legible minimum width, the wrapper scrolls.
                    class: "mx-auto block h-auto w-full min-w-[560px] sm:min-w-0",
                    style: "max-width: {svg_max_w}px",
                    xmlns: "http://www.w3.org/2000/svg",
                    view_box: "0 0 {svg_w} {svg_h}",
                    role: "group",
                    BoardSvg {
                        views,
                        per_side,
                        ratio,
                        selected_label: selected,
                        chip_label,
                    }
                }
            }
            Legend { items: legend_items }
        }
        PinDrawer {
            selected,
            pinout_pins: pinout.pins.clone(),
            pins_data,
            device_pk,
            connected,
            can_write,
            on_changed,
        }
    }
}
