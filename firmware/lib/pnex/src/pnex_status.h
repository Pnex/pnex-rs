//
// pnex-status — network state published for the local debug screen.
//
// Strict decoupling (screen contract, pnex_screen.h): the network PUBLISHES here
// (O(1) setters, never blocking, never on the data path);
// pnex_screen CONSUMES at tick. This header references no display
// driver — the network logic knows nothing about the screen.
//
// Single-threaded Arduino loop (WS callbacks fire inside poll() on
// the same thread): plain statics, no atomics.
//
// PNEX_HAS_SCREEN=0 (no screen gate compiled) → every setter falls
// back to a no-op: zero indicator update in a build without a screen,
// identical data path.
//

#ifndef PNEX_STATUS_H
#define PNEX_STATUS_H

#include <Arduino.h>

// Screen gates (0 by default — same defaults as pnex_screen.cpp, the builder
// always pushes them through ${sysenv.*}).
#ifndef PNEX_SCREEN_SSD1306
#define PNEX_SCREEN_SSD1306 0
#endif
#ifndef PNEX_SCREEN_ST7735
#define PNEX_SCREEN_ST7735 0
#endif

#define PNEX_HAS_SCREEN (PNEX_SCREEN_SSD1306 == 1 || PNEX_SCREEN_ST7735 == 1)

namespace pnex_status {

// Displayed connection steps: WiFi → Link (WS) → Registered.
enum class Step : uint8_t {
    Wifi = 0,       // connecting to the access point
    Link = 1,       // WiFi OK, connecting to the server (WS)
    Registered = 2, // provision_ack received — registered server-side
};

// Pin modes — mirror of the lib's PnexPinMode enum (Pnex.h). This header
// must not pull the whole transport stack, so the values are duplicated
// here; pnex.cpp static_asserts the mirror stays in sync.
enum class PinMode : uint8_t {
    DigitalIn = 0,
    DigitalOut = 1,
    AdcIn = 2,
    PwmOut = 3,
};

// One provisioned pin as shown in the MAIN pin panel. Static config only —
// values are NOT stored here: the screen samples the gpios directly at draw
// time (live, subscription-free).
struct PinEntry {
    uint8_t gpio;
    PinMode mode;
    uint8_t duty_pct;  // PWM: last written duty 0..=100
    char label[9];     // overlay label, truncated (8 chars)
};

// Snapshot capacity — mirrors PNEX_MAX_PINS (the lib announce guard).
constexpr size_t PIN_SLOTS = 32;

struct NetState {
    Step step = Step::Wifi;
    bool wifi_ok = false;
    uint8_t wifi_level = 0;  // 0-4 bars (mapped RSSI)
    bool link_ok = false;    // WS open
    uint32_t tx_count = 0;   // data frames sent (PING excluded)
    uint32_t rx_count = 0;   // data frames received (PONG excluded)
    // OTA deployment (a future screen page can render a progress bar).
    char ota_phase[12] = {0};  // "" | "downloading" | "flashing" | "failed"
    uint8_t ota_progress = 0;  // 0..=100
    // Provisioned pin table (ProvisionAck) — the board overlay already
    // excludes screen gpios server-side, so this is exactly the available
    // pins set ("screen pins excluded").
    PinEntry pins[PIN_SLOTS];
    uint8_t pin_count = 0;
};

// Storage (magic static — thread-safe init, single-threaded here anyway).
inline NetState& rw() {
    static NetState s;
    return s;
}

// Setters — called from the network path; the guard is a compiled constant
// (no-op fallback in a build without a screen).
inline void set_step(Step s) {
    if (PNEX_HAS_SCREEN) {
        rw().step = s;
    }
}

inline void set_wifi(bool ok, uint8_t level) {
    if (PNEX_HAS_SCREEN) {
        rw().wifi_ok = ok;
        rw().wifi_level = ok ? level : 0;
    }
}

inline void set_link(bool ok) {
    if (PNEX_HAS_SCREEN) {
        rw().link_ok = ok;
    }
}

inline void bump_tx() {
    if (PNEX_HAS_SCREEN) {
        ++rw().tx_count;
    }
}

inline void bump_rx() {
    if (PNEX_HAS_SCREEN) {
        ++rw().rx_count;
    }
}

inline void set_ota(const char* phase, uint8_t progress) {
    if (PNEX_HAS_SCREEN) {
        auto& s = rw();
        strncpy(s.ota_phase, phase, sizeof(s.ota_phase) - 1);
        s.ota_phase[sizeof(s.ota_phase) - 1] = '\0';
        s.ota_progress = progress;
    }
}

// Full-table republish (ProvisionAck, SetMode, PWM write — the panel only
// needs static config; duty travels with the next PWM write).
inline void set_pins(const PinEntry* entries, size_t n) {
    if (!PNEX_HAS_SCREEN) {
        return;
    }
    auto& s = rw();
    if (n > PIN_SLOTS) {
        n = PIN_SLOTS;  // = PNEX_MAX_PINS (the lib guard, mirrored by the array)
    }
    memcpy(s.pins, entries, n * sizeof(PinEntry));
    s.pin_count = (uint8_t)n;
}

// Screen-side read (at tick).
inline const NetState& state() {
    return rw();
}

}  // namespace pnex_status

#endif  // PNEX_STATUS_H
