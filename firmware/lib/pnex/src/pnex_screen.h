// pnex_screen — local debug screen for the PNeX generic firmwares.
//
// One tiny facade, two drivers behind compile-time gates:
//   PNEX_SCREEN_SSD1306=1 → 0.96" OLED, I2C (U8g2, HW I2C on
//   PNEX_SCREEN_SDA/SCL) — simple text pages, no animation.
//   PNEX_SCREEN_ST7735=1  → 1.77" TFT, SPI (Adafruit ST7735, canvas gerbe
//   animation ported from tft_st7735_demo; degraded direct-draw path on
//   ESP8266 — the 40 KB GFXcanvas16 does not fit the heap there).
//
// Both gates are 0 → everything is a no-op and no display library gets
// linked (driver #includes live inside the gates; PlatformIO LDF only
// links what is referenced).
//
// V2 (pnex-tft-mockups.html): status bar at the bottom from boot on
// (device id, wifi bars, server dot, TX/RX), connection timeline
// WiFi → WS → Reg above it while connecting, MAIN page (data placeholder)
// after "Registered". The screen state machine lives HERE and only here
// (compiled out when both gates are 0); it PULLS the network state from
// pnex_status::state() at each tick — the network side publishes, it never
// touches this facade. Redraws are change-only / partial so the pnex loop
// stays responsive.
//
// Contract of the defines (docs/architecture/firmware-build.md §2.1):
// PNEX_SCREEN_{KIND} = 0/1, PNEX_SCREEN_{ROLE} = gpio or -1. The builder
// always pushes all of them; manual builds get the same defaults in
// pnex_status.h / pnex_screen.cpp.

#ifndef PNEX_SCREEN_H
#define PNEX_SCREEN_H

#include <Arduino.h>

namespace pnex_screen {

// Init the compiled-in driver. No-op when both gates are 0.
void begin();

// Called every pnex loop turn AND during blocking network waits (wifi
// connect, reconnect backoff). Reads pnex_status::state() and redraws only
// what changed. No-op when both gates are 0.
void tick();

// Device id shown in the status bar (copied).
void set_device_id(const char* id);

}  // namespace pnex_screen

#endif  // PNEX_SCREEN_H
