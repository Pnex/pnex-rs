//! ESP32 DevKit V1 30 pins wired to a 1.77" ST7735 TFT (wiring of
//! the reference TFT wiring: SCK 18, MOSI 23, CS 5, DC 2 — strapping, Hi-Z
//! at reset —, RST 4, 160x128). The screen pins are reserved once the screen
//! is picked for the device.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-devkit-tft-st7735",
    soc: Some(Soc::Esp32),
    pretty_name: Some("ESP32 DevKit V1 + TFT 1.77\" (ST7735)"),
    pio_board: Some("esp32dev"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-devkit-tft-st7735".into(),
        name: Some("ESP32 DevKit V1 + TFT 1.77\" (ST7735)".into()),
        chip_label: Some("ESP-WROOM-32".into()),
        layout: dual_inline(15, 28.0, 55.0),
        peripherals: screens(vec![tft_st7735(18, 23, 5, 2, 4)]),
        pins: vec![
            en("EN", R, 14),
            io("VP", 36, R, 13),
            io("VN", 39, R, 12),
            io("D34", 34, R, 11),
            io("D35", 35, R, 10),
            io("D32", 32, R, 9),
            io("D33", 33, R, 8),
            io("D25", 25, R, 7),
            io("D26", 26, R, 6),
            io("D27", 27, R, 5),
            io("D14", 14, R, 4),
            io("D12", 12, R, 3),
            io("D13", 13, R, 2),
            gnd("GND", R, 1),
            power("VIN", R, 0),
            io("D23", 23, L, 14).note("screen SDA/MOSI (SPI wiring)"),
            io("D22", 22, L, 13),
            io("TX0", 1, L, 12),
            io("RX0", 3, L, 11),
            io("D21", 21, L, 10),
            io("D19", 19, L, 9),
            io("D18", 18, L, 8).note("screen SCK (SPI wiring)"),
            io("D5", 5, L, 7).note("screen CS (SPI wiring)"),
            io("TX2", 17, L, 6),
            io("RX2", 16, L, 5),
            io("D4", 4, L, 4).note("screen RST (SPI wiring)"),
            io("D2", 2, L, 3).note("screen DC — strapping, Hi-Z at reset"),
            io("D15", 15, L, 2),
            gnd("GND", L, 1),
            power("3V3", L, 0),
        ],
    }
}
