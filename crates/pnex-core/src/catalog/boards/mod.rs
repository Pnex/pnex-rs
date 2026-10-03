//! One module per physical board variant (`mcu_boards` row). A device
//! freezes its variant at registration; the model keeps the default one.
//!
//! Adding a board: a new module here + its line in `ALL`.

mod esp32_devkit_38p_txd;
mod esp32_devkit_tft_st7735;
mod esp32_devkit_v1_30p;
mod esp32_devkit_v1_36p;
mod esp32_devkitc_v4_38p;
mod esp32cam_ai_thinker;
mod esp32s3_devkitc1;
mod esp8266_nodemcu;
mod generic;
mod nodemcu_oled;
mod nodemcu_v3_oled;
mod waveshare_esp32c6_zero;
mod xiao_esp32c3;

use super::CatalogBoard;

pub const ESP8266_NODEMCU: CatalogBoard = esp8266_nodemcu::BOARD;
pub const NODEMCU_OLED: CatalogBoard = nodemcu_oled::BOARD;
pub const XIAO_ESP32C3: CatalogBoard = xiao_esp32c3::BOARD;
pub const ESP32_DEVKIT_V1_30P: CatalogBoard = esp32_devkit_v1_30p::BOARD;
pub const ESP32_DEVKIT_V1_36P: CatalogBoard = esp32_devkit_v1_36p::BOARD;
pub const ESP32_DEVKIT_TFT_ST7735: CatalogBoard = esp32_devkit_tft_st7735::BOARD;
pub const ESP32S3_DEVKITC1: CatalogBoard = esp32s3_devkitc1::BOARD;
pub const ESP32_DEVKITC_V4_38P: CatalogBoard = esp32_devkitc_v4_38p::BOARD;
pub const ESP32_DEVKIT_38P_TXD: CatalogBoard = esp32_devkit_38p_txd::BOARD;
pub const NODEMCU_V3_OLED: CatalogBoard = nodemcu_v3_oled::BOARD;
pub const ESP32CAM_AI_THINKER: CatalogBoard = esp32cam_ai_thinker::BOARD;
pub const WAVESHARE_ESP32C6_ZERO: CatalogBoard = waveshare_esp32c6_zero::BOARD;
pub const GENERIC: CatalogBoard = generic::BOARD;

/// Every board, in seed order (stable ids on a fresh database).
pub static ALL: &[CatalogBoard] = &[
    ESP8266_NODEMCU,
    NODEMCU_OLED,
    XIAO_ESP32C3,
    ESP32_DEVKIT_V1_30P,
    ESP32_DEVKIT_V1_36P,
    ESP32_DEVKIT_TFT_ST7735,
    ESP32S3_DEVKITC1,
    ESP32_DEVKITC_V4_38P,
    ESP32_DEVKIT_38P_TXD,
    NODEMCU_V3_OLED,
    ESP32CAM_AI_THINKER,
    WAVESHARE_ESP32C6_ZERO,
    GENERIC,
];
