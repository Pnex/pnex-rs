//! ESP32-S3 DevKitC-1 — simplified pin map (31 useful pins, GPIO48
//! omitted); positions do not claim the exact physical topology yet.
//!
//! External screens, one picked per device: I2C OLED on 8/9 (Arduino S3
//! Wire), SPI TFT on FSPI (SCK 12, MOSI 11, CS 10; DC 13, RST 14).

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-s3",
    soc: Some(Soc::Esp32S3),
    pretty_name: Some("ESP32-S3 DevKitC-1"),
    pio_board: Some("esp32-s3-devkitc-1"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-s3-devkitc-1".into(),
        name: Some("ESP32-S3 DevKitC-1".into()),
        chip_label: Some("ESP32-S3".into()),
        layout: dual_inline(16, 28.0, 60.0),
        peripherals: screens(vec![oled_ssd1306(8, 9), tft_st7735(12, 11, 10, 13, 14)]),
        pins: vec![
            en("EN", R, 15),
            io("D4", 4, R, 14),
            io("D0", 0, R, 13).note("strapping boot-HIGH (safe_state: low refused as output)"),
            io("D45", 45, R, 12).note("strapping boot-HIGH (safe_state: low refused as output)"),
            io("D46", 46, R, 11).note("input-only + strapping (no output, no pull-up)"),
            io("D1", 1, R, 10).note("ADC1"),
            io("D2", 2, R, 9).note("ADC1"),
            io("D3", 3, R, 8).note("strapping boot-HIGH (safe_state: low refused as output)"),
            io("D42", 42, R, 7),
            io("D41", 41, R, 6),
            io("D40", 40, R, 5),
            io("D39", 39, R, 4),
            io("D38", 38, R, 3),
            io("D47", 47, R, 2),
            gnd("GND", R, 1),
            power("5V", R, 0),
            power("3V3", L, 15),
            io("D21", 21, L, 14),
            io("D18", 18, L, 13),
            io("D17", 17, L, 12),
            io("D16", 16, L, 11),
            io("D15", 15, L, 10),
            io("D14", 14, L, 9).note("TFT RST when the TFT is picked"),
            io("D13", 13, L, 8).note("TFT DC when the TFT is picked"),
            io("D12", 12, L, 7).note("TFT SCK when the TFT is picked"),
            io("D11", 11, L, 6).note("TFT MOSI when the TFT is picked"),
            io("D10", 10, L, 5).note("TFT CS when the TFT is picked"),
            io("D9", 9, L, 4).note("OLED SCL when the OLED is picked"),
            io("D8", 8, L, 3).note("OLED SDA when the OLED is picked"),
            io("D7", 7, L, 2),
            io("D6", 6, L, 1),
            io("D5", 5, L, 0),
        ],
    }
}
