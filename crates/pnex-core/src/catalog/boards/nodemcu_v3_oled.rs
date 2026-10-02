//! NodeMCU V3 (LoLin, 15+15 pins, CH340G, USB-C) + soldered 0.96" OLED
//! (SSD1315, ssd1306-compatible, 0x3C).
//!
//! Generic Chinese board (ideaspark ESP8266MOD, "CH340G ESP-12F TYPE-C",
//! photo 2026-09-20): silkscreen "VU", "EN RST", row S = ESP-12F flash pins.
//! The screen is forced (`builtin`) on this family's NON-standard wiring:
//! SDA = D6 (GPIO12), SCL = D5 (GPIO14) — not the D2/D1 pair of external
//! OLED shields. The six flash pins (SK/S0/SC/S1/S2/S3 = GPIO 6/7/11/8/9/10)
//! are shown but excluded at admission — never wire them. Orientation: front
//! view, USB on top, antenna at the bottom (the product sheet is mirrored).

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "nodemcu_v3_oled",
    soc: Some(Soc::Esp8266),
    pretty_name: Some("NodeMCU V3 + OLED 0.96\" (soldered, CH340G)"),
    pio_board: Some("nodemcuv2"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "nodemcu_v3_oled".into(),
        name: Some("NodeMCU V3 + OLED 0.96\"".into()),
        chip_label: Some("ESP-12F".into()),
        layout: dual_inline(15, 31.0, 58.0),
        peripherals: screens(vec![oled_ssd1306(12, 14).builtin()]),
        pins: vec![
            power("3V", L, 0),
            gnd("G", L, 1),
            io("TX", 1, L, 2).note("console UART"),
            io("RX", 3, L, 3).note("console UART"),
            io("D8", 15, L, 4).note("strapping boot-LOW (pulldown) — CS HSPI"),
            io("D7", 13, L, 5),
            io("D6", 12, L, 6).note("I2C screen SDA (soldered, always reserved)"),
            io("D5", 14, L, 7).note("I2C screen SCL (soldered, always reserved)"),
            gnd("G", L, 8),
            power("3V", L, 9),
            io("D4", 2, L, 10).note("strapping boot-HIGH — LED onboard"),
            io("D3", 0, L, 11).note("strapping — FLASH button on the board"),
            io("D2", 4, L, 12),
            io("D1", 5, L, 13),
            io("D0", 16, L, 14).note("no PWM nor pull-up (pulldown only)"),
            power("VIN", R, 0),
            gnd("G", R, 1),
            en("RST", R, 2),
            en("EN", R, 3),
            power("3V", R, 4),
            gnd("G", R, 5),
            io("SK", 6, R, 6).note("flash SPI (SCLK) — excluded from admission, never wire it"),
            io("S0", 7, R, 7).note("flash SPI (SDD0) — excluded from admission, never wire it"),
            io("SC", 11, R, 8).note("flash SPI (SDCMD) — excluded from admission, never wire it"),
            io("S1", 8, R, 9).note("flash SPI (SDD1) — excluded from admission, never wire it"),
            io("S2", 9, R, 10).note("flash SPI (SDD2) — excluded from admission, never wire it"),
            io("S3", 10, R, 11).note("flash SPI (SDD3) — excluded from admission, never wire it"),
            power("VU", R, 12).note("5 V USB"),
            gnd("G", R, 13),
            io("A0", 17, R, 14).note("ADC channel — wire id 17 (no physical GPIO 17)"),
        ],
    }
}
