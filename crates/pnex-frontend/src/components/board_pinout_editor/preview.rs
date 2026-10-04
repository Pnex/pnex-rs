//! Read-only board preview modal opened from the wizard: rendered from the
//! board catalog (no device state), purely visual pin selection.

use super::*;

use super::svg::{BoardSvg, Legend};
use super::view::{
    board_ratio, board_w, cap_color, layout_per_side, svg_height, svg_width, PinView, BOARD_X,
    CHIP_W, PAD_TOP, ROW_H,
};
use crate::components::icons;
use crate::components::modal::Modal;

/// Vue de pins depuis le catalogue `GET /boards` (DTO wizard) — même
/// géométrie que `pin_views`, sans état device (rien de configuré/réservé).
fn preview_views(board: &api::boards::Board) -> Vec<PinView> {
    let bw = board_w(layout_per_side(&board.layout), board_ratio(&board.layout));
    let mut views = Vec::with_capacity(board.pins.len());
    for pin in &board.pins {
        let Some(pos) = pin.pos.as_object() else {
            continue;
        };
        let side = match pos.get("side").and_then(|v| v.as_str()) {
            Some("right") => "right",
            _ => "left",
        };
        let index = pos.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
        let (pad_x, chip_x) = if side == "left" {
            (BOARD_X + 4.0, BOARD_X + 4.0 - 16.0 - CHIP_W)
        } else {
            (BOARD_X + bw - 4.0, BOARD_X + bw - 4.0 + 16.0)
        };
        let y = PAD_TOP + index * ROW_H + ROW_H / 2.0 + 30.0;
        let (color, legend_key) = cap_color(&pin.fns, Some(&pin.kind));
        let mut badges: Vec<&'static str> = Vec::new();
        if pin.flags.iter().any(|f| f == "strapping") {
            badges.push("strapping");
        }
        if pin.flags.iter().any(|f| f == "input_only") {
            badges.push("input_only");
        }
        views.push(PinView {
            label: pin.label.clone(),
            gpio: pin.gpio,
            kind: pin.kind.clone(),
            side,
            index,
            x: pad_x,
            y,
            chip_x,
            color,
            legend_key,
            badges,
            selected: false,
            configured: false,
            reserved: false,
            mode_label: None,
            last_value_label: None,
        });
    }
    views
}

/// Popup « Voir le pinout » du wizard : board pleine taille avec labels,
/// badges (strapping / input-only) et légende — rendue depuis le catalogue
/// (`GET /boards`), sans état device (rien de configuré/réservé). La
/// sélection d'un pin est purement visuelle (aucun panneau de config).
#[component]
pub fn BoardPreviewModal(board: api::boards::Board, on_close: Callback<()>) -> Element {
    let views = preview_views(&board);
    let selected = use_signal(|| None::<String>);
    let per_side = layout_per_side(&board.layout);
    let ratio = board_ratio(&board.layout);
    let svg_h = svg_height(per_side);
    let svg_w = svg_width(per_side, ratio);
    // Same scale as the former fixed-width board (340 px for 550 units).
    let svg_max_w = (svg_w * 340.0 / 550.0).round();
    let title = board
        .pretty_name
        .clone()
        .unwrap_or_else(|| board.name.clone());
    let chip_label = board
        .chip_label
        .clone()
        .or(Some(board.soc.clone()))
        .unwrap_or_default();
    let legend_items = legend_of(&views);
    let zoom = use_signal(|| ZOOM_LEVELS[0]);
    rsx! {
        Modal { title, max_width: "max-w-xl".to_string(), on_close,
            div { class: "space-y-3",
                ZoomBar { zoom }
                // Phone: keep a legible minimum width and scroll sideways
                // instead of shrinking the pin labels to ~6 px.
                div { class: "overflow-x-auto",
                    svg {
                        class: "mx-auto block w-full min-w-[560px] sm:min-w-0",
                        style: zoom_style(zoom(), svg_max_w),
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
        }
    }
}

// (La miniature dans les chips a été remplacée par `BoardPreviewModal` :
// pleine taille, zoomable au scroll de la modale, tous les pins labellisés.)

/// Zoom steps of the pinout drawing, in percent of the fitted width.
pub(super) const ZOOM_LEVELS: [u32; 4] = [100, 150, 200, 300];

/// Inline style of the pinout `<svg>`: fitted (capped at `max_w`) at 100 %,
/// otherwise a wider drawing that the `overflow-x-auto` wrapper scrolls —
/// pin labels stay readable on phones where pinch-zoom is not available.
pub(super) fn zoom_style(zoom: u32, max_w: f64) -> String {
    if zoom <= 100 {
        format!("max-width: {max_w}px")
    } else {
        let min_w = 560 * zoom / 100;
        format!("width: {zoom}%; min-width: {min_w}px; max-width: none")
    }
}

/// − / level / + buttons stepping through [`ZOOM_LEVELS`]; the level button
/// resets to 100 %.
#[component]
pub(super) fn ZoomBar(mut zoom: Signal<u32>) -> Element {
    let current = zoom();
    let idx = ZOOM_LEVELS.iter().position(|z| *z == current).unwrap_or(0);
    let can_out = idx > 0;
    let can_in = idx + 1 < ZOOM_LEVELS.len();
    let btn = "inline-flex items-center justify-center rounded-lg border border-gray-300 bg-white px-2 py-1 text-xs font-medium text-gray-600 hover:border-blue-400 hover:text-blue-600 disabled:opacity-40 disabled:hover:border-gray-300 disabled:hover:text-gray-600 transition-colors";
    rsx! {
        div { class: "flex items-center justify-end gap-1",
            button {
                class: btn,
                r#type: "button",
                disabled: !can_out,
                title: t!("board-zoom-out"),
                aria_label: t!("board-zoom-out"),
                onclick: move |_| zoom.set(ZOOM_LEVELS[idx.saturating_sub(1)]),
                icons::ZoomOut { class: "h-4 w-4" }
            }
            button {
                class: "{btn} min-w-[3.5rem] tabular-nums",
                r#type: "button",
                title: t!("board-zoom-reset"),
                aria_label: t!("board-zoom-reset"),
                onclick: move |_| zoom.set(ZOOM_LEVELS[0]),
                "{current} %"
            }
            button {
                class: btn,
                r#type: "button",
                disabled: !can_in,
                title: t!("board-zoom-in"),
                aria_label: t!("board-zoom-in"),
                onclick: move |_| zoom.set(ZOOM_LEVELS[(idx + 1).min(ZOOM_LEVELS.len() - 1)]),
                icons::ZoomIn { class: "h-4 w-4" }
            }
        }
    }
}
