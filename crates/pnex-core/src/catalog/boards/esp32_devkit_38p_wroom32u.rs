//! ESP32 38 pins with an ESP32-WROOM-32U module (external antenna, U.FL
//! connector; checked on a photo 2026-10-04, USB on top, front view):
//! bare-number silkscreen, symmetric 19 + 19 pins.
//!
//! - left header (USB → antenna): CLK, D0, D1, 15, 2, 0, 4, 16, 17, 5, 18,
//!   19, GND, 21, RX, TX, 22, 23, GND;
//! - right header (USB → antenna): 5V, CMD, D3, D2, 13, GND, 12, 14, 27,
//!   26, 25, 33, 32, 35, 34, VN, VP, EN, 3V3.
//!
//! Same pin map as `esp32-devkit-38p-txd` (only the silkscreen and the
//! module differ). The WROOM-32U has no PCB antenna: without an antenna on
//! the U.FL connector, WiFi is weak or absent. The six flash pins (6–11) sit
//! at the USB corner; shown but excluded at admission — never wire them.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-devkit-38p-wroom32u",
    soc: Some(Soc::Esp32),
    pretty_name: Some("ESP32 DevKit 38 pins (WROOM-32U, external antenna)"),
    pio_board: Some("esp32dev"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-devkit-38p-wroom32u".into(),
        name: Some("ESP32 DevKit 38 pins (WROOM-32U, external antenna)".into()),
        chip_label: Some("ESP32-WROOM-32U".into()),
        layout: dual_inline(19, 28.0, 55.0),
        peripherals: screens(vec![oled_ssd1306(21, 22), tft_st7735(18, 23, 5, 2, 4)]),
        pins: vec![
            io("CLK", 6, L, 0).note("flash SPI — excluded from admission, never wire it"),
            io("D0", 7, L, 1).note("flash SPI — excluded from admission, never wire it"),
            io("D1", 8, L, 2).note("flash SPI — excluded from admission, never wire it"),
            io("15", 15, L, 3).note("strapping boot-HIGH — output: safe_state high only"),
            io("2", 2, L, 4).note("strapping boot-LOW — onboard LED on some clones"),
            io("0", 0, L, 5).note("strapping — BOOT button on the board"),
            io("4", 4, L, 6).note("ADC2 (broken with WiFi)"),
            io("16", 16, L, 7).note("UART2 RX2"),
            io("17", 17, L, 8).note("UART2 TX2"),
            io("5", 5, L, 9).note("strapping boot-HIGH (VSPI CS)"),
            io("18", 18, L, 10).note("VSPI SCK"),
            io("19", 19, L, 11).note("VSPI MISO"),
            gnd("GND", L, 12),
            io("21", 21, L, 13).note("default I2C SDA"),
            io("RX", 3, L, 14).note("console UART"),
            io("TX", 1, L, 15).note("console UART"),
            io("22", 22, L, 16).note("default I2C SCL"),
            io("23", 23, L, 17).note("VSPI MOSI"),
            gnd("GND", L, 18),
            power("5V", R, 0),
            io("CMD", 11, R, 1).note("flash SPI — excluded from admission, never wire it"),
            io("D3", 10, R, 2).note("flash SPI — excluded from admission, never wire it"),
            io("D2", 9, R, 3).note("flash SPI — excluded from admission, never wire it"),
            io("13", 13, R, 4),
            gnd("GND", R, 5),
            io("12", 12, R, 6).note("strapping boot-LOW — output: safe_state low only"),
            io("14", 14, R, 7).note("strapping (HSPI) — output: safe_state high only"),
            io("27", 27, R, 8).note("ADC2 (broken with WiFi)"),
            io("26", 26, R, 9).note("ADC2 (broken with WiFi) — DAC"),
            io("25", 25, R, 10).note("ADC2 (broken with WiFi) — DAC"),
            io("33", 33, R, 11).note("ADC1"),
            io("32", 32, R, 12).note("ADC1"),
            io("35", 35, R, 13).note("input-only — ADC1"),
            io("34", 34, R, 14).note("input-only — ADC1"),
            io("VN", 39, R, 15).note("input-only — ADC1"),
            io("VP", 36, R, 16).note("input-only — ADC1"),
            en("EN", R, 17),
            power("3V3", R, 18),
        ],
    }
}
