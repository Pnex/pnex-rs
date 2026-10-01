//! Pin view model and layout geometry for the board pinout SVG: view
//! precomputation, capacity colors, sub-labels and legend ordering.

use super::*;

/// Géométrie de rendu (héritée du prototype `pnex-pinout.html`).
pub(super) const ROW_H: f64 = 34.0;
pub(super) const PAD_TOP: f64 = 54.0;
pub(super) const BOARD_X: f64 = 180.0;
pub(super) const CHIP_W: f64 = 124.0;
pub(super) const CHIP_H: f64 = 25.0;

/// Hauteur du viewBox : le PCB (`board_h` = 2×PAD_TOP + rows + 14) commence
/// à y=14 et doit rester entièrement visible — l'ancienne formule
/// (`PAD_TOP + rows + PAD_BOT`) rognait le bas du PCB.
pub(super) fn svg_height(per_side: f64) -> f64 {
    2.0 * PAD_TOP + per_side * ROW_H + 34.0
}

/// PCB height for a layout's per-side pin count (starts at y=14).
pub(super) fn board_h(per_side: f64) -> f64 {
    2.0 * PAD_TOP + per_side * ROW_H + 14.0
}

/// Width/length ratio of the PCB, from the profile's `layout.size_mm`
/// (seeded from the board YAML). Profiles snapshotted before that field
/// existed fall back to typical sizes: devkits (ESP32 / ESP8266) are
/// ~28-32 x 55-60 mm, small modules (XIAO C3/C6, <= 7 pins per side)
/// are ~18 x 23 mm.
pub(super) fn board_ratio(layout: &serde_json::Value) -> f64 {
    let size = &layout["size_mm"];
    match (size["w"].as_f64(), size["l"].as_f64()) {
        (Some(w), Some(l)) if w > 0.0 && l > 0.0 => w / l,
        _ => {
            if layout_per_side(layout) <= 7.0 {
                18.0 / 23.0
            } else {
                30.0 / 57.5
            }
        }
    }
}

/// Pins per side of a layout (8 when absent).
pub(super) fn layout_per_side(layout: &serde_json::Value) -> f64 {
    layout.get("per_side").and_then(|v| v.as_u64()).unwrap_or(8) as f64
}

/// PCB width: the drawn height scaled by the real proportions, so the
/// board is not rendered as a thin strip.
pub(super) fn board_w(per_side: f64, ratio: f64) -> f64 {
    (board_h(per_side) * ratio).round()
}

/// ViewBox width: PCB + room for the pin chips on both sides.
pub(super) fn svg_width(per_side: f64, ratio: f64) -> f64 {
    BOARD_X + board_w(per_side, ratio) + 180.0
}

/// Couleurs de capacité (prototype : information, pas de la déco).
pub(super) fn cap_color(fns: &[String], kind: Option<&str>) -> (&'static str, &'static str) {
    let key = match kind {
        Some("power") => "power",
        Some("gnd") => "gnd",
        Some("en") => "en",
        _ => {
            let has = |n: &str| fns.iter().any(|f| f == n);
            if has("dac") {
                "dac"
            } else if has("touch") {
                "touch"
            } else if has("adc1") || has("adc2") {
                "adc"
            } else if has("i2c_sda") || has("i2c_scl") {
                "i2c"
            } else if has("spi_mosi") || has("spi_miso") || has("spi_clk") || has("spi_cs") {
                "spi"
            } else if has("uart_tx") || has("uart_rx") {
                "uart"
            } else {
                "gpio"
            }
        }
    };
    match key {
        "power" => ("#e11d48", "board-legend-power"),
        "gnd" => ("#334155", "board-legend-gnd"),
        "en" => ("#a1a1aa", "board-legend-en"),
        "adc" => ("#f59e0b", "board-legend-adc"),
        "touch" => ("#ec4899", "board-legend-touch"),
        "dac" => ("#f97316", "board-legend-dac"),
        "i2c" => ("#14b8a6", "board-legend-i2c"),
        "spi" => ("#8b5cf6", "board-legend-spi"),
        "uart" => ("#0ea5e9", "board-legend-uart"),
        _ => ("#64748b", "board-legend-gpio"),
    }
}

/// Sous-label d'un pin (« GPIO4 » ou le kind pour alim/masse/reset).
pub(super) fn gpio_label(gpio: Option<i32>) -> String {
    gpio.map(|g| format!("GPIO{g}")).unwrap_or_default()
}

#[derive(Clone, PartialEq)]
pub(super) struct PinView {
    pub(super) label: String,
    pub(super) gpio: Option<i32>,
    pub(super) kind: String,
    pub(super) side: &'static str,
    pub(super) index: f64,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) chip_x: f64,
    pub(super) color: &'static str,
    pub(super) legend_key: &'static str,
    pub(super) badges: Vec<&'static str>,
    pub(super) selected: bool,
    pub(super) configured: bool,
    pub(super) reserved: bool,
    pub(super) mode_label: Option<String>,
    pub(super) last_value_label: Option<String>,
}

/// Précalcul des vues de pins (AUCUN `let` dans les boucles for rsx).
pub(super) fn pin_views(
    board: &api::pins::BoardSummary,
    pins: &[api::pins::PinoutPin],
    selected: Option<&str>,
) -> Vec<PinView> {
    let bw = board_w(layout_per_side(&board.layout), board_ratio(&board.layout));
    let mut views = Vec::with_capacity(pins.len());
    for pin in pins {
        let Some(pos) = pin.pos.as_ref() else {
            continue; // hors géométrie v2 — ignoré dans le rendu SVG
        };
        let side = match pos["side"].as_str() {
            Some("right") => "right",
            _ => "left",
        };
        let index = pos["index"].as_u64().unwrap_or(0) as f64;
        let (pad_x, chip_x) = if side == "left" {
            (BOARD_X + 4.0, BOARD_X + 4.0 - 16.0 - CHIP_W)
        } else {
            (BOARD_X + bw - 4.0, BOARD_X + bw - 4.0 + 16.0)
        };
        let y = PAD_TOP + index * ROW_H + ROW_H / 2.0 + 30.0;
        let (color, legend_key) = cap_color(&pin.fns, pin.kind.as_deref());
        let mut badges: Vec<&'static str> = Vec::new();
        if pin.flags.iter().any(|f| f == "strapping") {
            badges.push("strapping");
        }
        if pin.flags.iter().any(|f| f == "input_only") {
            badges.push("input_only");
        }
        if pin.flags.iter().any(|f| f == "reserved") {
            badges.push("reserved");
        }
        if pin.reserved {
            badges.push("screen");
        }
        let configured = pin.source == "instance";
        views.push(PinView {
            label: pin.label.clone(),
            gpio: pin.gpio,
            kind: pin.kind.clone().unwrap_or_else(|| "gpio".into()),
            side,
            index,
            x: pad_x,
            y,
            chip_x,
            color,
            legend_key,
            badges,
            selected: selected == Some(pin.label.as_str()),
            configured,
            reserved: pin.reserved,
            mode_label: pin.mode.clone(),
            last_value_label: pin.last_value.as_ref().map(|v| match v {
                serde_json::Value::Bool(true) => "1".to_string(),
                serde_json::Value::Bool(false) => "0".to_string(),
                other => other.to_string(),
            }),
        });
    }
    views
}

/// Légende des capacités utilisées (dédupliquée, ordre fixe).
pub(super) fn legend_of(views: &[PinView]) -> Vec<(&'static str, &'static str)> {
    let order = [
        "board-legend-gpio",
        "board-legend-adc",
        "board-legend-touch",
        "board-legend-dac",
        "board-legend-i2c",
        "board-legend-spi",
        "board-legend-uart",
        "board-legend-power",
        "board-legend-gnd",
        "board-legend-en",
    ];
    let mut seen: Vec<&'static str> = Vec::new();
    for v in views {
        if !seen.contains(&v.legend_key) {
            seen.push(v.legend_key);
        }
    }
    order
        .into_iter()
        .filter(|k| seen.contains(k))
        .map(|k| {
            let color = views
                .iter()
                .find(|v| v.legend_key == k)
                .map(|v| v.color)
                .unwrap_or("#64748b");
            (k, color)
        })
        .collect()
}
