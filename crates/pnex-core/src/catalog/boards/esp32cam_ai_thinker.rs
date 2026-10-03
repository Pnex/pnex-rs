//! AI-Thinker ESP32-CAM (OV2640 + 4 MB PSRAM), generic camera firmware
//! (camera-video.md D77).
//!
//! Two 8-pin headers. No USB: index 0 = the 5V / 3V3 end, where the
//! ESP32-CAM-MB carrier plugs its USB bridge. Almost every GPIO is taken by
//! the module, so only GPIO4 (flash LED) and GPIO33 (red LED, active LOW,
//! safe_state high = off) are admitted, both provisioned as outputs out of
//! the box. Everything else is listed without a gpio (rendered, never
//! provisioned): camera bus (0 = XCLK + boot strap, 5, 18, 19, 21–23, 25–27,
//! 32 = PWDN, 34–36, 39), PSRAM CS 16, flash 6–11, console UART 1/3, SD card
//! bus 2/4/12–15 (GPIO12 = VDD_SDIO strapping). GPIO33 is not on a header: it
//! sits on a virtual row 8, hence per_side 9.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;
use crate::proto::{Mode, SafeState};

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32cam-ai-thinker",
    soc: Some(Soc::Esp32),
    pretty_name: Some("AI-Thinker ESP32-CAM (OV2640)"),
    pio_board: Some("esp32cam"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32cam-ai-thinker".into(),
        name: Some("AI-Thinker ESP32-CAM".into()),
        chip_label: Some("ESP32-S".into()),
        layout: dual_inline(9, 27.0, 40.5),
        peripherals: screens(vec![]),
        pins: vec![
            power("5V", L, 0),
            gnd("GND", L, 1),
            unwired("IO12", L, 2).note("SD DATA2 + VDD_SDIO strapping — reserved"),
            unwired("IO13", L, 3).note("SD DATA3 — reserved"),
            unwired("IO15", L, 4).note("SD CMD + strapping — reserved"),
            unwired("IO14", L, 5).note("SD CLK — reserved"),
            unwired("IO2", L, 6).note("SD DATA0 + strapping — reserved"),
            io("Flash LED", 4, L, 7).default_mode(Mode::DigitalOut).safe_state(SafeState::Low).pull_down().note("IO4 — onboard flash LED (bright) — output; also SD DATA1"),
            io("Red LED", 33, L, 8).default_mode(Mode::DigitalOut).safe_state(SafeState::High).active_low().note("GPIO33 — onboard red LED, not on a header — output, active LOW (safe_state high = off)"),
            power("3V3", R, 0),
            unwired("IO16", R, 1).note("PSRAM CS — reserved"),
            unwired("IO0", R, 2).note("camera XCLK + boot strap (GND = flash mode) — reserved"),
            gnd("GND", R, 3),
            power("VCC", R, 4).note("3.3 V / 5 V output (solder jumper)"),
            unwired("U0R", R, 5).note("GPIO3 console UART RX — reserved"),
            unwired("U0T", R, 6).note("GPIO1 console UART TX — reserved"),
            gnd("GND", R, 7),
        ],
    }
}
