//
// Generic ESP32 firmware (DevKit WROOM, Tier 1 preset) — 100 % driven
// by the server. Same PneX lib as the C3 and the 8266 (no protocol
// divergence): pin map pushed in the ProvisionAck (the board overlay
// stays the authority — this sketch declares NOTHING),
// SetMode/Write/Subscribe commands with Ack, timed reads as StateReport,
// safe-states + backoff.
//
// Classic ESP32 specifics (absorbed in the lib, reminder here):
// - analogRead on the pin's GPIO (12-bit) — chip-caps restrict analog_in
//   to GPIO32–39 (ADC1: ADC2 is broken with WiFi on);
// - GPIO34–39 input-only (no output driver, no internal pull-up);
// - the monitor goes through the GPIO1/3 UART bridge (reserved for console);
// - no user watchdog armed — safe-states + backoff cover the
//   link losses (§3/§8).
//
// Device config: -D b64 defines via ${sysenv.*} (server build per
// device, contract docs/architecture/firmware-build.md §2.1).
// PNEX_BOARD_NAME distinguishes the announce (wire id of the variant).
//

#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.begin();
}

void loop() {
    pnex.loop();
}
