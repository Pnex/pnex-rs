//
// Generic ESP32-C6 firmware (Waveshare ESP32-C6-Zero, Tier 1 preset) —
// 100% server driven. Same PneX lib as the C3, S3, ESP32 and 8266 (no
// protocol divergence): pin map pushed in the ProvisionAck (the board
// overlay stays the authority — this sketch declares NOTHING), SetMode/
// Write/Subscribe commands with Ack, paced reads as StateReports,
// safe-states + backoff.
//
// ESP32-C6 specifics (absorbed in the lib + server chip-caps, repeated
// here for the record):
// - Arduino core 3.x only (pioarduino platform, pinned in platformio.ini);
//   the lib's LEDC calls go through its core 2/3 shim;
// - PNEX_CHIP = "esp32-c6" (CONFIG_IDF_TARGET_ESP32C6);
// - ADC1 = GPIO0-6 (no ADC2); strapping boot-HIGH on 8/9;
// - native USB-JTAG/serial on 12/13 (reserved) — the monitor goes through
//   that port with ARDUINO_USB_CDC_ON_BOOT=1.
//
// Device config: b64 -D defines via ${sysenv.*} (per-device server build,
// contract in docs/architecture/firmware-build.md §2.1). PNEX_BOARD_NAME
// distinguishes the announce (wire id of the board variant).
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
