//! Predefined devices (`predefined_devices` rows): what the user picks when
//! registering a device.

use super::boards;
use super::types::DeviceType;
use super::CatalogProduct;

/// Shop / documentation links of a product sold by PNeX.
#[derive(Debug, Clone, Copy)]
pub struct ShopLinks {
    pub device_doc_url: &'static str,
    pub prestashop_product_id: &'static str,
    pub prestashop_buy_url: &'static str,
    pub byod_doc_url: &'static str,
    pub image_source_url: &'static str,
    pub stl_files_url: &'static str,
}

/// Generic firmware product: pins driven from the UI, no capability.
const fn generic(
    name: &'static str,
    pretty_name: &'static str,
    board: &'static super::CatalogBoard,
    description: &'static str,
    description_i18n: &'static str,
) -> CatalogProduct {
    CatalogProduct {
        name,
        pretty_name,
        revision: "1.0.0",
        device_type: DeviceType::Mixed,
        capabilities: &[],
        board,
        shop: None,
        description,
        description_i18n,
    }
}

/// Every predefined device, in seed order.
pub static ALL: &[CatalogProduct] = &[
    // Generic firmware, compiled per device. The device announces itself on
    // /ws/device; admission is checked against the chip caps + the board
    // profile (no auto-creation at announce).
    generic(
        "generic_esp8266",
        "Generic ESP8266 (NodeMCU / D1 mini)",
        &boards::ESP8266_NODEMCU,
        "ESP8266 dev board on the generic firmware: pins driven from the UI (modes, write, subscribe), no code to write.",
        "device-predef-generic-esp8266-desc",
    ),
    generic(
        "generic_esp32c3",
        "Generic ESP32-C3 (Seeed XIAO)",
        &boards::XIAO_ESP32C3,
        "ESP32-C3 (Seeed XIAO) dev board on the generic firmware: pins driven from the UI (modes, write, subscribe), D0–D10 overlay admission, ADC1 (D0–D2) and strapping-safe outputs.",
        "device-predef-generic-esp32c3-desc",
    ),
    generic(
        "generic_esp32",
        "Generic ESP32 (DevKit WROOM)",
        &boards::ESP32_DEVKIT_V1_30P,
        "ESP32 (DevKit WROOM) dev board on the generic firmware: pins driven from the UI, ADC1 (GPIO32-39), GPIO34-39 input-only, strapping-safe outputs; DevKit V1 30p / 36p / TFT ST7735 variants.",
        "device-predef-generic-esp32-desc",
    ),
    generic(
        "generic_esp32s3",
        "Generic ESP32-S3 (DevKitC-1)",
        &boards::ESP32S3_DEVKITC1,
        "ESP32-S3 (DevKitC-1) dev board on the generic firmware: pins driven from the UI, ADC1 (GPIO1-10), GPIO46 input-only, strapping-safe outputs; OLED 0.96\" / TFT 1.77\" screens optional.",
        "device-predef-generic-esp32s3-desc",
    ),
    generic(
        "generic_esp32c6",
        "Generic ESP32-C6 (Waveshare C6-Zero)",
        &boards::WAVESHARE_ESP32C6_ZERO,
        "ESP32-C6 (Waveshare C6-Zero) dev board on the generic firmware: pins driven from the UI, ADC1 (GPIO0-6), strapping-safe outputs; OLED 0.96\" / TFT 1.77\" screens optional.",
        "device-predef-generic-esp32c6-desc",
    ),
    // Generic camera firmware (camera-video.md D77): same /ws/device pin
    // slave as generic_esp32 plus the camera uplink /ws/camera.
    generic(
        "generic_esp32cam",
        "Generic ESP32-CAM (AI-Thinker)",
        &boards::ESP32CAM_AI_THINKER,
        "AI-Thinker ESP32-CAM (OV2640, PSRAM) on the generic firmware: live video to the server (resolution, quality, fps from the UI), flash LED (GPIO4) and red LED (GPIO33) driven from the UI. Flash with an ESP32-CAM-MB carrier or an FTDI adapter (GPIO0 to GND).",
        "device-predef-generic-esp32cam-desc",
    ),
    // Edge agent (D95, edge-agent.md): a Rust daemon on a PC / server / Pi,
    // not a microcontroller. No firmware, no pins, no OTA.
    CatalogProduct {
        name: crate::devices::EDGE_AGENT_PREDEF,
        pretty_name: "Edge agent (PC / server / Raspberry Pi)",
        revision: "1.0.0",
        device_type: DeviceType::Agent,
        capabilities: &[],
        board: &boards::GENERIC,
        shop: None,
        description: "Rust agent running on a computer (Linux, Windows, Raspberry Pi): any script pushes free-form values to a local HTTP API; the agent buffers them on disk and forwards them securely to PNeX.",
        description_i18n: "device-predef-edge-agent-desc",
    },
];
