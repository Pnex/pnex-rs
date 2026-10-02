//! ESP32 DevKit V1 (DOIT, 36 pins): the 30 pins of the V1 + GPIO6–11
//! (flash, shown but excluded at admission) + extra GND/VIN.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-devkit-v1-36p",
    soc: Some(Soc::Esp32),
    pretty_name: Some("ESP32 DevKit V1 (DOIT, 36 pins)"),
    pio_board: Some("esp32dev"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-devkit-v1-36p".into(),
        name: Some("ESP32 DevKit V1 (DOIT, 36 pins)".into()),
        chip_label: Some("ESP-WROOM-32".into()),
        layout: dual_inline(18, 28.0, 57.0),
        peripherals: screens(vec![oled_ssd1306(21, 22), tft_st7735(18, 23, 5, 2, 4)]),
        pins: vec![
            io("G0", 0, R, 17),
            io("G1", 1, R, 16),
            io("G2", 2, R, 15),
            io("G3", 3, R, 14),
            io("G4", 4, R, 13),
            io("G5", 5, R, 12),
            io("G6", 6, R, 11),
            io("G7", 7, R, 10),
            io("G8", 8, R, 9),
            io("G9", 9, R, 8),
            io("G10", 10, R, 7),
            io("G11", 11, R, 6),
            io("G12", 12, R, 5),
            io("G13", 13, R, 4),
            io("G14", 14, R, 3),
            io("G15", 15, R, 2),
            io("G16", 16, R, 1),
            io("G17", 17, R, 0),
            io("G18", 18, L, 17),
            io("G19", 19, L, 16),
            io("G21", 21, L, 15),
            io("G22", 22, L, 14),
            io("G23", 23, L, 13),
            io("G25", 25, L, 12),
            io("G26", 26, L, 11),
            io("G27", 27, L, 10),
            io("G32", 32, L, 9),
            io("G33", 33, L, 8),
            io("G34", 34, L, 7),
            io("G35", 35, L, 6),
            io("G36", 36, L, 5),
            io("G39", 39, L, 4),
            en("EN", L, 3),
            gnd("GND", L, 2),
            power("3V3", L, 1),
            power("VIN", L, 0),
        ],
    }
}
