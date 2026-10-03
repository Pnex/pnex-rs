//! Waveshare ESP32-C6-Zero (ESP32-C6FH8, 8 MB in-package flash, native
//! USB-C, 9+9 pins, 18 x 25.5 mm).
//!
//! Front view, USB on top. Left: 5V, GND, 3V3(OUT), GP0–GP5 (ADC1). Right:
//! TX (GPIO16), RX (GPIO17), GP14, GP15, GP18–GP22. The console is the
//! native USB-CDC port (GPIO12/13), so UART0 TX/RX stay plain gpios. The
//! castellated pads under the board (GP6/7/8/9/12/13/23) are not modeled:
//! GPIO8 drives the onboard WS2812 RGB LED, GPIO9 is the BOOT button.
//! External screens, one picked per device: I2C OLED on GP21/GP22, SPI TFT
//! on GP19 (SCK), GP20 (MOSI), GP18 (CS), GP14 (DC), GP15 (RST).

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-c6-zero",
    soc: Some(Soc::Esp32C6),
    pretty_name: Some("Waveshare ESP32-C6-Zero"),
    pio_board: Some("esp32-c6-devkitc-1"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "waveshare_esp32c6_zero".into(),
        name: Some("Waveshare ESP32-C6-Zero".into()),
        chip_label: Some("ESP32-C6".into()),
        layout: dual_inline(9, 18.0, 25.5),
        peripherals: screens(vec![oled_ssd1306(21, 22), tft_st7735(19, 20, 18, 14, 15)]),
        pins: vec![
            power("5V", L, 0),
            gnd("GND", L, 1),
            power("3V3", L, 2).note("3.3 V output"),
            io("GP0", 0, L, 3).note("ADC1"),
            io("GP1", 1, L, 4).note("ADC1"),
            io("GP2", 2, L, 5).note("ADC1"),
            io("GP3", 3, L, 6).note("ADC1"),
            io("GP4", 4, L, 7).note("ADC1 — strapping (MTMS), no boot-mode risk"),
            io("GP5", 5, L, 8).note("ADC1 — strapping (MTDI), no boot-mode risk"),
            io("TX", 16, R, 0).note("UART0 TX — free (console on native USB)"),
            io("RX", 17, R, 1).note("UART0 RX — free (console on native USB)"),
            io("GP14", 14, R, 2),
            io("GP15", 15, R, 3).note("strapping (JTAG source), no boot-mode risk"),
            io("GP18", 18, R, 4),
            io("GP19", 19, R, 5),
            io("GP20", 20, R, 6),
            io("GP21", 21, R, 7),
            io("GP22", 22, R, 8),
        ],
    }
}
