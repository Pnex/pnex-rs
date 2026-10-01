//! SVG drawing of the board: shared silhouette (USB on top convention),
//! interactive pin chips and the capacity legend.

use super::*;

use super::view::{board_h as pcb_h, board_w, PinView, BOARD_X, CHIP_H, CHIP_W};

/// PCB + labeled chip + USB connector at the TOP — the shared silhouette
/// used by the interactive editor (`BoardSvg`) and the wizard preview
/// (`BoardPreviewModal`). CONVENTION (décision utilisateur 2026-09-21) :
/// tout est stocké et dessiné **USB en haut** — les profils board posent
/// `pos.index` 0 = coin USB et `side` = côté affiché, le rendu est donc un
/// 1:1 sans aucune rotation (le YAML se lit comme la photo comme le SVG).
#[component]
fn BoardSvgBase(views: Vec<PinView>, per_side: f64, ratio: f64, chip_label: String) -> Element {
    // PCB sized on the layout's per-side count (longest column of an
    // asymmetric board, e.g. nodemcu 15/13).
    let board_h = pcb_h(per_side);
    let bw = board_w(per_side, ratio);
    let cx = BOARD_X + bw / 2.0;
    let chip_x = cx - 32.0;
    let chip_y = 14.0 + (board_h - 14.0) / 2.0 - 32.0;
    let label = if chip_label.chars().count() > 10 {
        chip_label.chars().take(12).collect::<String>()
    } else {
        chip_label.clone()
    };
    let font = if label.chars().count() > 9 {
        "8.5"
    } else {
        "10"
    };
    rsx! {
        g {
            // PCB
            rect {
                x: "{BOARD_X}", y: "14", width: "{bw}", height: "{board_h}",
                rx: "14", fill: "#10203a", stroke: "#1e3a5f", stroke_width: "1.5",
            }
            // Inner copper border (mockup style)
            rect {
                x: "{BOARD_X + 7.0}", y: "21", width: "{bw - 14.0}", height: "{board_h - 14.0}",
                rx: "11", fill: "none", stroke: "#3a4a5a", stroke_width: "1",
                stroke_dasharray: "2 4", opacity: "0.5",
            }
            // Chip: pin-1 marker + label + legs
            rect {
                x: "{chip_x}", y: "{chip_y}", width: "64", height: "64",
                rx: "6", fill: "#16212e", stroke: "#25333f", stroke_width: "1.5",
            }
            circle {
                cx: "{chip_x + 11.0}", cy: "{chip_y + 11.0}", r: "3",
                fill: "#3a4a5a",
            }
            text {
                x: "{cx}", y: "{chip_y + 38.0}",
                text_anchor: "middle", fill: "#7d8b99",
                font_size: "{font}", font_family: "ui-monospace, monospace",
                {label}
            }
            for i in 0..5u32 {
                rect {
                    key: "{i}",
                    x: "{chip_x - 6.0}", y: "{chip_y + 12.0 + i as f64 * 11.0}",
                    width: "6", height: "4", rx: "1", fill: "#3a4a5a",
                }
                rect {
                    x: "{chip_x + 64.0}", y: "{chip_y + 12.0 + i as f64 * 11.0}",
                    width: "6", height: "4", rx: "1", fill: "#3a4a5a",
                }
            }
            // Copper traces above/below the chip
            for i in 0..5u32 {
                line {
                    key: "{i}",
                    x1: "{cx - 20.0 + i as f64 * 10.0}", y1: "{chip_y - 26.0}", x2: "{cx - 20.0 + i as f64 * 10.0}", y2: "{chip_y}",
                    stroke: "#3a4a5a", stroke_width: "1", opacity: "0.5",
                }
                line {
                    x1: "{cx - 20.0 + i as f64 * 10.0}", y1: "{chip_y + 64.0}", x2: "{cx - 20.0 + i as f64 * 10.0}", y2: "{chip_y + 90.0}",
                    stroke: "#3a4a5a", stroke_width: "1", opacity: "0.5",
                }
            }
            // USB connector (top, centered) — no label, the drawing speaks
            // for itself. Convention USB en haut : jamais de rotation ici.
            rect {
                x: "{cx - 26.0}", y: "18", width: "52", height: "22",
                rx: "8", fill: "#8b97a4", stroke: "#1e3a5f", stroke_width: "1.2",
            }
            rect {
                x: "{cx - 20.0}", y: "24", width: "40", height: "10",
                rx: "5", fill: "#5b6572",
            }
        }
    }
}

/// Interactive board: silhouette + clickable pin chips. Key = position
/// (`side-index`), never the label: power/GND pins share their label
/// (3× `GND` sur le 38p) and duplicate keyed siblings panic the diff.
#[component]
pub(super) fn BoardSvg(
    views: Vec<PinView>,
    per_side: f64,
    ratio: f64,
    selected_label: Signal<Option<String>>,
    chip_label: String,
) -> Element {
    rsx! {
        BoardSvgBase { views: views.clone(), per_side, ratio, chip_label }
        for v in views {
            PinChip {
                key: "{v.side}-{v.index}",
                view: v,
                selected_label,
            }
        }
    }
}

/// Un chip de pin cliquable (pad + rect + bande couleur + label + badges).
#[component]
fn PinChip(view: PinView, mut selected_label: Signal<Option<String>>) -> Element {
    let left = view.side == "left";
    let band_x = if left {
        view.chip_x + CHIP_W - 5.0
    } else {
        view.chip_x
    };
    // Anneau « configuré » centré sur le pad (halo) — jamais décalé.
    let ring_x = view.x;
    let opacity = if view.reserved { "0.55" } else { "1" };
    let stroke = if view.selected { "#2563eb" } else { "#cbd5e1" };
    let stroke_w = if view.selected { "2.4" } else { "1.2" };
    let mut badge_x = if left {
        view.chip_x + CHIP_W - 14.0
    } else {
        view.chip_x + CHIP_W - 14.0
    };
    let sub = view
        .gpio
        .map(|g| format!("GPIO{g}"))
        .unwrap_or_else(|| view.kind.clone());
    rsx! {
        g {
            class: "cursor-pointer",
            opacity: "{opacity}",
            onclick: move |_| selected_label.set(Some(view.label.clone())),
            circle {
                cx: "{view.x}", cy: "{view.y}", r: "5",
                fill: "#1e3a5f", stroke: "#cfe0f2", stroke_width: "1.2",
            }
            if view.configured {
                circle {
                    cx: "{ring_x}", cy: "{view.y}", r: "8",
                    fill: "none", stroke: "#2563eb", stroke_width: "2",
                }
            }
            rect {
                x: "{view.chip_x}", y: "{view.y - CHIP_H / 2.0}",
                width: "{CHIP_W}", height: "{CHIP_H}", rx: "7",
                fill: "#ffffff", stroke: "{stroke}", stroke_width: "{stroke_w}",
            }
            rect {
                x: "{band_x}", y: "{view.y - CHIP_H / 2.0}",
                width: "5", height: "{CHIP_H}",
                fill: "{view.color}",
            }
            text {
                x: "{view.chip_x + 12.0}", y: "{view.y - 1.0}",
                fill: "#0f172a", font_size: "12.5", font_weight: "700",
                font_family: "ui-monospace, monospace",
                {view.label.clone()}
            }
            text {
                x: "{view.chip_x + 12.0}", y: "{view.y + 10.0}",
                fill: "#94a3b8", font_size: "8.5",
                font_family: "ui-monospace, monospace",
                {sub}
            }
            if let Some(lv) = view.last_value_label {
                circle {
                    cx: "{view.chip_x + CHIP_W - 12.0}", cy: "{view.y + 4.0}",
                    r: "3.5", fill: "#22c55e",
                }
                text {
                    x: "{view.chip_x + CHIP_W - 20.0}", y: "{view.y + 7.0}",
                    fill: "#16a34a", font_size: "8", text_anchor: "end",
                    font_family: "ui-monospace, monospace",
                    {lv}
                }
            }
            for badge in view.badges {
                {badge_circle(view.y, badge, &mut badge_x)}
            }
        }
    }
}

/// Pastille d'avertissement (strapping / input_only / reserved / screen).
fn badge_circle(y: f64, badge: &str, bx: &mut f64) -> Element {
    let cx = *bx;
    *bx -= 10.0;
    let (fill, _title) = match badge {
        "strapping" => ("#f59e0b", "strapping"),
        "input_only" => ("#334155", "input only"),
        "reserved" => ("#ef4444", "reserved"),
        _ => ("#14b8a6", "screen"),
    };
    rsx! {
        circle {
            cx: "{cx}", cy: "{y - CHIP_H / 2.0 + 6.0}", r: "3.5",
            fill: "{fill}",
        }
    }
}

/// Label i18n d'une clé de légende — t! exige un littéral, donc match
/// exhaustif (toute nouvelle capacité passe ici).
fn legend_label(key: &str) -> String {
    match key {
        "board-legend-gpio" => t!("board-legend-gpio").to_string(),
        "board-legend-adc" => t!("board-legend-adc").to_string(),
        "board-legend-touch" => t!("board-legend-touch").to_string(),
        "board-legend-dac" => t!("board-legend-dac").to_string(),
        "board-legend-i2c" => t!("board-legend-i2c").to_string(),
        "board-legend-spi" => t!("board-legend-spi").to_string(),
        "board-legend-uart" => t!("board-legend-uart").to_string(),
        "board-legend-power" => t!("board-legend-power").to_string(),
        "board-legend-gnd" => t!("board-legend-gnd").to_string(),
        "board-legend-en" => t!("board-legend-en").to_string(),
        _ => key.to_string(),
    }
}

/// Légende des capacités présentes.
#[component]
pub(super) fn Legend(items: Vec<(&'static str, &'static str)>) -> Element {
    rsx! {
        div { class: "flex flex-wrap gap-x-4 gap-y-2 mt-3 pt-3 border-t border-dashed border-gray-200",
            for (key, color) in items {
                span { key: "{key}", class: "inline-flex items-center gap-1.5 text-xs text-gray-500",
                    span { class: "inline-block w-3 h-3 rounded", style: "background:{color}" }
                    {legend_label(key)}
                }
            }
        }
    }
}
