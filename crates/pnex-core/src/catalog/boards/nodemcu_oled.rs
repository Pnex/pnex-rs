//! NodeMCU + 0.96" SSD1306 OLED (I2C) soldered on the board.
//!
//! `builtin` screen: forced on for every device of this variant (GPIO4/5
//! permanently reserved, disabling refused, UI picker locked). The build
//! passes `-D PNEX_SCREEN_SSD1306=1`.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "nodemcu_oled",
    soc: Some(Soc::Esp8266),
    pretty_name: Some("NodeMCU + OLED 0.96\" (I2C)"),
    pio_board: Some("nodemcuv2"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "nodemcu_oled".into(),
        name: Some("NodeMCU + OLED 0.96\"".into()),
        chip_label: Some("ESP-12E".into()),
        layout: dual_inline(8, 30.0, 58.0),
        peripherals: screens(vec![oled_ssd1306(4, 5).builtin()]),
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
            io("D1", 5, L, 5).note("I2C screen SCL (soldered, always reserved)"),
            io("D2", 4, L, 4).note("I2C screen SDA (soldered, always reserved)"),
            io("D3", 0, L, 3),
            io("D4", 2, L, 2).note("LED onboard"),
            en("EN", L, 1),
            power("VIN", L, 0),
        ],
    }
}
