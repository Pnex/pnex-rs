//! ESP32 DevKitC V4 ("DevKit 38 pins" clone, 19 per side): the classic 30
//! pins + the internal SPI flash pins brought out (CLK/SD0/SD1/SD2/SD3/CMD =
//! GPIO6–11) + 5V/GND. Flash pins are shown but excluded at admission
//! (touching them reboot-loops the board). VP/VN (36/39) = ADC1 input-only.

use crate::boards::BoardProfileV2;
use crate::caps::Soc;
use crate::catalog::pins::*;
use crate::catalog::CatalogBoard;

pub const BOARD: CatalogBoard = CatalogBoard {
    name: "esp32-devkitc-v4-38p",
    soc: Some(Soc::Esp32),
    pretty_name: Some("ESP32 DevKitC V4 (38 pins)"),
    pio_board: Some("esp32dev"),
    profile: Some(profile),
};

fn profile() -> BoardProfileV2 {
    BoardProfileV2 {
        board: "esp32-devkitc-v4-38p".into(),
        name: Some("ESP32 DevKitC V4 (38 pins)".into()),
        chip_label: Some("ESP-WROOM-32".into()),
        layout: dual_inline(19, 28.0, 55.0),
        peripherals: screens(vec![oled_ssd1306(21, 22), tft_st7735(18, 23, 5, 2, 4)]),
        pins: vec![
            en("EN", R, 18),
            io("VP", 36, R, 17).note("input-only — ADC1"),
            io("VN", 39, R, 16).note("input-only — ADC1"),
            io("D34", 34, R, 15).note("input-only — ADC1"),
            io("D35", 35, R, 14).note("input-only — ADC1 (PSRAM on WROVER)"),
            io("D32", 32, R, 13).note("ADC1"),
            io("D33", 33, R, 12).note("ADC1"),
            io("D25", 25, R, 11).note("ADC2 (broken with WiFi) — DAC"),
            io("D26", 26, R, 10).note("ADC2 (broken with WiFi) — DAC"),
            io("D27", 27, R, 9).note("ADC2 (broken with WiFi)"),
            io("D14", 14, R, 8).note("strapping (HSPI) — output: safe_state high only"),
            io("D12", 12, R, 7).note("strapping boot-LOW — output: safe_state low only"),
            io("D13", 13, R, 6),
            gnd("GND", R, 5),
            power("VIN", R, 4),
            io("CLK", 6, R, 3).note("flash SPI — excluded from admission, never wire it"),
            io("SD0", 7, R, 2).note("flash SPI — excluded from admission, never wire it"),
            io("SD1", 8, R, 1).note("flash SPI — excluded from admission, never wire it"),
            io("SD2", 9, R, 0).note("flash SPI — excluded from admission, never wire it"),
            io("SD3", 10, L, 18).note("flash SPI — excluded from admission, never wire it"),
            io("CMD", 11, L, 17).note("flash SPI — excluded from admission, never wire it"),
            io("D23", 23, L, 16).note("VSPI MOSI"),
            io("D22", 22, L, 15).note("default I2C SCL"),
            io("TX0", 1, L, 14).note("console UART"),
            io("RX0", 3, L, 13).note("console UART"),
            io("D21", 21, L, 12).note("default I2C SDA"),
            io("D19", 19, L, 11).note("VSPI MISO"),
            io("D18", 18, L, 10).note("VSPI SCK"),
            io("D5", 5, L, 9).note("strapping boot-HIGH (VSPI CS)"),
            io("TX2", 17, L, 8),
            io("RX2", 16, L, 7),
            io("D4", 4, L, 6).note("ADC2 (broken with WiFi)"),
            io("D2", 2, L, 5).note("strapping boot-LOW — onboard LED on some clones"),
            io("D15", 15, L, 4).note("strapping boot-HIGH — output: safe_state high only"),
            gnd("GND", L, 3),
            power("3V3", L, 2),
            power("5V", L, 1),
            gnd("GND", L, 0),
        ],
    }
}
