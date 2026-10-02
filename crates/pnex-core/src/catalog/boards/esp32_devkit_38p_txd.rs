//! ESP32 38 pins, YD-ESP32-like clone (checked on the board 2026-09-21, USB
//! on top, front view): G-labels silkscreen, symmetric 19 + 19 pins.
//!
//! - G header (left in the UI, USB → antenna): CLK, SD0, SD1, G15, G2, G0,
//!   G4, G16, G17, G5, G18, G19, GND, G21, RXD, TXD, G22, G23, GND;
//! - power header (right, USB → antenna): 5V, CMD, SD3, SD2, G13, GND, G12,
//!   G14, G27, G26, G25, G33, G32, G35, G34, VN, VP, EN, 3V3.
//!
//! The six flash pins (6–11) all sit at the USB corner; shown but excluded at
//! admission — never wire them (reboot loop). Mirrored-silkscreen "38 pins"
//! clones exist (G header on the right, GND at the USB end): a distinct
//! variant if ever needed. Real silkscreen RX0/TX0 and SVP/SVN, normalized
//! here as RXD/TXD and VP/VN. TFT wiring proven on this board
//! (18/23/5/2/4).

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-devkit-38p-txd",
    soc: Some(Soc::Esp32),
    pretty_name: Some("ESP32 DevKit 38 pins (TXD/RXD)"),
    pio_board: Some("esp32dev"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-devkit-38p-txd".into(),
        name: Some("ESP32 DevKit 38 pins (TXD/RXD)".into()),
        chip_label: Some("ESP-WROOM-32".into()),
        layout: dual_inline(19, 28.0, 55.0),
        peripherals: screens(vec![oled_ssd1306(21, 22), tft_st7735(18, 23, 5, 2, 4)]),
        pins: vec![
            io("CLK", 6, L, 0).note("flash SPI — excluded from admission, never wire it"),
            io("SD0", 7, L, 1).note("flash SPI — excluded from admission, never wire it"),
            io("SD1", 8, L, 2).note("flash SPI — excluded from admission, never wire it"),
            io("G15", 15, L, 3).note("strapping boot-HIGH — output: safe_state high only"),
            io("G2", 2, L, 4).note("strapping boot-LOW — onboard LED on some clones"),
            io("G0", 0, L, 5).note("strapping — BOOT button on the board"),
            io("G4", 4, L, 6).note("ADC2 (broken with WiFi)"),
            io("G16", 16, L, 7).note("UART2 RX2"),
            io("G17", 17, L, 8).note("UART2 TX2"),
            io("G5", 5, L, 9).note("strapping boot-HIGH (VSPI CS)"),
            io("G18", 18, L, 10).note("VSPI SCK"),
            io("G19", 19, L, 11).note("VSPI MISO"),
            gnd("GND", L, 12),
            io("G21", 21, L, 13).note("default I2C SDA"),
            io("RXD", 3, L, 14).note("console UART"),
            io("TXD", 1, L, 15).note("console UART"),
            io("G22", 22, L, 16).note("default I2C SCL"),
            io("G23", 23, L, 17).note("VSPI MOSI"),
            gnd("GND", L, 18),
            power("5V", R, 0),
            io("CMD", 11, R, 1).note("flash SPI — excluded from admission, never wire it"),
            io("SD3", 10, R, 2).note("flash SPI — excluded from admission, never wire it"),
            io("SD2", 9, R, 3).note("flash SPI — excluded from admission, never wire it"),
            io("G13", 13, R, 4),
            gnd("GND", R, 5),
            io("G12", 12, R, 6).note("strapping boot-LOW — output: safe_state low only"),
            io("G14", 14, R, 7).note("strapping (HSPI) — output: safe_state high only"),
            io("G27", 27, R, 8).note("ADC2 (broken with WiFi)"),
            io("G26", 26, R, 9).note("ADC2 (broken with WiFi) — DAC"),
            io("G25", 25, R, 10).note("ADC2 (broken with WiFi) — DAC"),
            io("G33", 33, R, 11).note("ADC1"),
            io("G32", 32, R, 12).note("ADC1"),
            io("G35", 35, R, 13).note("input-only — ADC1 (PSRAM on WROVER)"),
            io("G34", 34, R, 14).note("input-only — ADC1"),
            io("VN", 39, R, 15).note("input-only — ADC1 (silkscreen SVN)"),
            io("VP", 36, R, 16).note("input-only — ADC1 (silkscreen SVP)"),
            en("EN", R, 17),
            power("3V3", R, 18),
        ],
    }
}
