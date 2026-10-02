//! NodeMCU ESP8266 (v1.0, ESP-12E).
//!
//! A0 = wire id 17 (ADC convention, no physical GPIO 17). External screens,
//! one picked per device: I2C OLED on D2/D1 (4/5, NodeMCU standard), SPI TFT
//! on HSPI (SCK 14/D5, MOSI 13/D7, CS 15/D8 — boot-LOW strapping, fine as
//! CS) + DC 4/D2, RST 5/D1. Picked → its pins are reserved.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp8266",
    soc: Some(Soc::Esp8266),
    pretty_name: Some("NodeMCU ESP8266 (v1.0)"),
    pio_board: Some("nodemcuv2"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "nodemcu".into(),
        name: Some("NodeMCU ESP8266 (v1.0)".into()),
        chip_label: Some("ESP-12E".into()),
        layout: dual_inline(8, 28.0, 55.0),
        peripherals: screens(vec![oled_ssd1306(4, 5), tft_st7735(14, 13, 15, 4, 5)]),
        pins: vec![
            io("A0", 17, R, 7).note("ADC channel — wire id 17 (no physical GPIO 17)"),
            io("D0", 16, R, 6).note("no PWM nor pull-up (pulldown only)"),
            io("D5", 14, R, 5),
            io("D6", 12, R, 4),
            io("D7", 13, R, 3),
            io("D8", 15, R, 2),
            io("RX", 3, R, 1),
            io("TX", 1, R, 0),
            power("3V3", L, 7),
            gnd("GND", L, 6),
            io("D1", 5, L, 5),
            io("D2", 4, L, 4),
            io("D3", 0, L, 3),
            io("D4", 2, L, 2).note("LED onboard"),
            en("EN", L, 1),
            power("VIN", L, 0),
        ],
    }
}
