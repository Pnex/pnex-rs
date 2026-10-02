//! Seeed XIAO ESP32-C3.
//!
//! D0–D10; D0–D2 = ADC1, D3 (GPIO5 = ADC2) digital only (ADC2 broken with
//! WiFi), D4/D5 = I2C, D6/D7 = UART0, D8/D9 = boot-HIGH strapping
//! (safe_state low refused as digital_out). External screens: I2C OLED on
//! D4/D5 (6/7, XIAO Wire), SPI TFT on D8/D10 + D1/D2/D3 (SCK 8 = boot-HIGH
//! strapping, fine as idle SCK; CS 3, DC 4, RST 5).

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-c3",
    soc: Some(Soc::Esp32C3),
    pretty_name: Some("Seeed XIAO ESP32-C3"),
    pio_board: Some("seeed_xiao_esp32c3"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "xiao_esp32c3".into(),
        name: Some("Seeed XIAO ESP32-C3".into()),
        chip_label: Some("ESP32-C3".into()),
        layout: dual_inline(7, 17.8, 21.0),
        peripherals: screens(vec![oled_ssd1306(6, 7), tft_st7735(8, 10, 3, 4, 5)]),
        pins: vec![
            io("D0", 2, R, 6),
            io("D1", 3, R, 5),
            io("D2", 4, R, 4),
            io("D3", 5, R, 3),
            io("D4", 6, R, 2),
            io("D5", 7, R, 1),
            io("D6", 21, R, 0),
            power("5V", L, 6),
            gnd("GND", L, 5),
            power("3V3", L, 4),
            io("D10", 10, L, 3),
            io("D9", 9, L, 2),
            io("D8", 8, L, 1),
            io("D7", 20, L, 0),
        ],
    }
}
