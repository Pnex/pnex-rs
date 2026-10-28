//
// Generic ESP32-C3 firmware (Seeed XIAO ESP32C3) — Tier 1 preset,
// 100 % server-driven. Same PneX lib as the ESP8266 (ex-mirror of
// generic_esp8266, NO protocol divergence anymore): pin map pushed
// in the ProvisionAck (the board overlay stays the authority — this sketch
// declares NOTHING), SetMode/Write/Subscribe commands with Ack, timed
// reads as StateReport, safe-states + backoff.
//
// C3-specific (absorbed in the lib, reminder here):
// - analogRead on the pin's GPIO (12-bit, 0-4095; the esp8266 read A0,
//   single 10-bit ADC channel) — chip-caps restrict analog_in to GPIO0-4
//   (ADC1: ADC2/GPIO5 is broken with WiFi on);
// - no user watchdog armed (loopTask not watched on
//   arduino-esp32) — link losses stay covered by the
//   safe-states + backoff (§3/§8);
// - USB-CDC (ttyACM0) is the log/monitor port (ARDUINO_USB_CDC_ON_BOOT).
//
// Device config: -D b64 defines via ${sysenv.*} (server build per
// device, contract docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distinguishes the announce ("xiao_esp32c3").
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
