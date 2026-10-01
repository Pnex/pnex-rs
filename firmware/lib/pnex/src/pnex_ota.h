//
// pnex-ota — device-side OTA client: pull HTTP(S) of the per-device app
// image, sha256-verified, written to the inactive slot (ESP32 app0/app1)
// or the eboot staging area (ESP8266), then reboot.
//
// Contract (edge-model.md §9, docs/architecture/ota.md):
// - the URL is a PATH — scheme + host come from the compiled config
//   (plain http = LAN/on-prem reference path);
// - the download is streamed in ~1 KB chunks into Update.h, digest
//   compared to the pushed sha256 BEFORE Update.end flips the boot slot;
// - progress flows back via DeviceMsg::OtaState frames (ESP32 keeps the
//   WS open; ESP8266 closes it first — one TLS context fits in ~40 KB);
// - a device-side downgrade guard: ESP8266 (no bootloader rollback)
//   refuses a strictly older numeric version.
//

#ifndef PNEX_OTA_H
#define PNEX_OTA_H

#include <Arduino.h>

// Hooks: `tick` keeps the debug screen alive during blocking stretches
// (no-op without screen); `progress` emits DeviceMsg::OtaState frames.
struct PnexOtaHooks {
    void (*tick)() = nullptr;
    void (*progress)(const char* phase, uint8_t pct, const char* err) = nullptr;
};

// Run a full OTA: download `url_path` (+ token/device_id query), verify
// against `sha_hex`, flash, return true when the device must reboot.
// On failure fills `err` and the device stays on its current firmware.
bool pnex_ota_run(const char* url_path,
                  const char* sha_hex,
                  const PnexOtaHooks& hooks,
                  char* err,
                  size_t errsz);

#endif  // PNEX_OTA_H
