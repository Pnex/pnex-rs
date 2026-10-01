//
// Generic ESP32-S3 firmware (DevKitC-1, Tier 1 preset) — 100% server
// driven. Same PneX lib as the C3, ESP32 and 8266 (no protocol
// divergence): pin map pushed in the ProvisionAck (the board overlay
// stays the authority — this sketch declares NOTHING), SetMode/Write/
// Subscribe commands with Ack, paced reads as StateReports, safe-states
// + backoff.
//
// ESP32-S3 specifics (absorbed in the lib + server chip-caps, repeated
// here for the record):
// - PNEX_CHIP = "esp32-s3" (CONFIG_IDF_TARGET_ESP32S3, detected in the
//   lib) — the server chip-caps validation applies the S3 rule grid;
// - ADC1 = GPIO1-10 (ADC2 = 11-20, broken with WiFi active);
// - GPIO46 is input-only; strapping boot-HIGH on 0/3/45/46;
// - native USB-JTAG/serial on 19/20 (reserved) — the monitor goes through
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
