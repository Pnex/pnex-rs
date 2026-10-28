//
// PneX — generic lib of the PNeX project's ESP firmwares (publishable PIO
// package). Holds the MECHANICS: transport (WiFi + WS + ChaCha20
// framing, ex pnex-transport), compiled config (pnex_config.h, -D
// b64 defines — contract firmware-build.md §2.1) and the `pin_slave` profile
// (announces/proto: pin map pushed in the ProvisionAck, RPCs
// SetMode/Write/Subscribe + Ack, timed reads as StateReport).
//
// Sketch policy (custom firmware, edge-model.md §2 bis): the sketch owns
// its pins — the ones declared here (`addInput`/`addOutput`/
// `addAnalogInput`) travel in `Announce.pins`, the server validates them
// (chip-caps: flash/strapping/ADC/input-only), persists and drives them;
// none declared = an empty pin map. Generic models declare nothing: the
// server's board overlay is the authority and the pin map arrives in the
// ProvisionAck.
//
// Wire contract: crates/pnex-core/src/proto.rs (ArduinoJson mirror).
// Any loss (close, PONG timeout, WiFi) → outputs to safe-state then
// reconnect backoff 1 s → 60 s.
//
#ifndef PNEX_H
#define PNEX_H

#include <Arduino.h>
#include <vector>

// Private signatures carrying JsonDocument — the lib brings it itself
// (dependency declared in library.json).
#include <ArduinoJson.h>

#if defined(ESP8266)
#include <ESP8266WiFi.h>
#else
#include <WiFi.h>
#endif

#include "pnex_transport.h"

// ─────────────────── Announce identity (overridable with -D) ───────────────────

// Chip announced to the server — drives the chip-caps validation server-side.
#ifndef PNEX_CHIP
#if defined(ESP8266)
#define PNEX_CHIP "esp8266"
#elif defined(CONFIG_IDF_TARGET_ESP32C3)
#define PNEX_CHIP "esp32-c3"
#elif defined(CONFIG_IDF_TARGET_ESP32C6)
#define PNEX_CHIP "esp32-c6"
#elif defined(CONFIG_IDF_TARGET_ESP32S3)
#define PNEX_CHIP "esp32-s3"
#else
#define PNEX_CHIP "esp32"
#endif
#endif

// Announced board (informative only) — overridden with -D in the
// project's platformio.ini.
#ifndef PNEX_BOARD_NAME
#if defined(ESP8266)
#define PNEX_BOARD_NAME "nodemcu"
#else
#define PNEX_BOARD_NAME "generic"
#endif
#endif

// Announced firmware version — keep in step with library.json.
#ifndef PNEX_FW_VERSION
#define PNEX_FW_VERSION "1.0.0"
#endif

// OTA support gate (0/1): when 1, the announce carries the `ota` cap
// (server-side admission signal) and the firmware accepts
// ServerMsg::OtaAvailable. Only the server-built generic projects define it
// (sketches built elsewhere stay implicit 0 — they answer "unknown message").
#ifndef PNEX_OTA_ENABLE
#define PNEX_OTA_ENABLE 0
#endif

// Safeguard (NOT a functional limit): beyond it, the add* refuse —
// protects the static announce buffer.
#ifndef PNEX_MAX_PINS
#define PNEX_MAX_PINS 32
#endif

// Upper bound of the extra announce caps registered by optional modules
// (addCap) — static table, no heap.
#ifndef PNEX_MAX_EXTRA_CAPS
#define PNEX_MAX_EXTRA_CAPS 4
#endif

// Upper bound of custom metrics (addMetric, D87) — static table, no heap;
// each one adds ~45 bytes to the announce.
#ifndef PNEX_MAX_METRICS
#define PNEX_MAX_METRICS 16
#endif

// Upper bound of custom commands (onCommand, D88) — static table, no heap.
#ifndef PNEX_MAX_COMMANDS
#define PNEX_MAX_COMMANDS 8
#endif

// Custom command handler (D88): receives the free JSON `args` sent by the
// server; returns true on success (Ack ok) or false (Ack err
// "command_failed"). Runs inside the WS callback: keep it short, record the
// work and run it from loop() when it is slow. A capture-less lambda
// converts to this pointer type.
typedef bool (*PnexCommandHandler)(JsonVariantConst args);

// Hook for server messages the lib does not handle itself (e.g. the camera
// module consumes `camera_config`). Returns true when the message was
// consumed; it runs inside the WS callback, so it must stay short (record
// the work, run it from loop — same lesson as the deferred OTA).
typedef bool (*PnexServerMsgHook)(JsonDocument& doc);

// ───────────────────────── Pin table ─────────────────────────

enum PnexPinMode : uint8_t {
    PNEX_DIGITAL_IN = 0,
    PNEX_DIGITAL_OUT = 1,
    PNEX_ADC_IN = 2,
    PNEX_PWM_OUT = 3,
};

struct PnexPin {
    uint8_t gpio = 0;
    uint8_t mode = PNEX_DIGITAL_IN;
    bool pullup = false;
    bool safe_high = false;
    uint32_t interval_ms = 0;
    unsigned long last_read_ms = 0;
    bool declared = false;  // declared by the sketch → Announce.pins
    String label;
    uint8_t duty_pct = 0;  // PWM: last written duty % (0..=100)
};

// A single PnexDevice instance per sketch (transport callbacks as
// bare pointers → static trampolines to the current instance).
class PnexDevice {
public:
    // ── Pin declarations (custom firmware: the sketch owns its pins) ──
    // The server validates (chip-caps) then drives them; without a server
    // the pin stays usable locally (applied at boot).
    bool addInput(uint8_t gpio, const char* label = nullptr, bool pullup = false);
    bool addOutput(uint8_t gpio, const char* label = nullptr, bool safe_high = false);
    /// PWM output — writes in duty % (0..=100), default frequency of the
    /// Arduino core (abstraction: no frequency config).
    bool addPwmOutput(uint8_t gpio, const char* label = nullptr, bool safe_high = false);
    /// ESP8266: the single ADC (A0) — gpio ignored. ESP32: the ADC1 GPIO.
    bool addAnalogInput(uint8_t gpio, const char* label = nullptr);

    /// Blocking: WiFi (40 attempts) + 1st WS connection + announce. A
    /// clean connection at boot announces TOO (observed e2e 2026-09-14:
    /// it only left on the "reconnect" path — ghost session).
    void begin();

    /// Call on every loop() round — WS poll, PING 5 s, timed reads
    /// of the subscribed pins → StateReport.
    void loop();

    // ── Custom metrics and commands (custom firmware, D87/D88) ──

    /// Declares a custom metric, announced as a `metric` cap: its values
    /// become the O2 series `{org}/{device}/{id}`. Call before begin(). `id`
    /// and `unit` must outlive the device (literals); `unit` may be null.
    /// False when the table is full (PNEX_MAX_METRICS) or the id is empty.
    bool addMetric(const char* id, const char* unit = nullptr);

    /// Publishes one value of a declared metric (StateReport without gpio,
    /// routed by cap_id). Unknown id = ignored with a serial log; NaN/Inf
    /// is skipped (a gap, never an invented value). Non-blocking; dropped
    /// silently while the server link is down.
    void publish(const char* id, float value);
    void publish(const char* id, double value);
    void publish(const char* id, int value);
    void publish(const char* id, long value);
    void publish(const char* id, unsigned long value);
    void publish(const char* id, bool value);
    void publish(const char* id, const char* value);

    /// Registers a command callable from PneX (announced as a `command`
    /// cap). Call before begin(). Registering the same name again replaces
    /// the handler. False when the table is full (PNEX_MAX_COMMANDS).
    bool onCommand(const char* name, PnexCommandHandler handler);

    // ── Extension points for optional modules (camera…) ──

    /// Extra announce cap `{id, family}` appended to the manifest (D47).
    /// Call before begin() so the first announce carries it. The strings
    /// must outlive the device (literals). False when the table is full.
    bool addCap(const char* id, const char* family);

    /// Registers the hook for server messages unknown to the lib (at most
    /// one; a later call replaces it). Consulted before the "unknown
    /// message" fallback only — built-in types are never forwarded.
    void onServerMessage(PnexServerMsgHook hook);

    /// DeviceMsg::Ack — public so module hooks can answer their commands.
    void sendAck(const char* cmd_id, bool ok, const char* err);

    /// Serializes then sends one JSON message on the control WS (384-byte
    /// cap, dropped with a log when longer).
    void sendJson(const JsonDocument& doc);

    /// OTA progress trampoline hook (public: the capture-less trampoline
    /// in pnex.cpp sends DeviceMsg::OtaState frames through it).
    void sendOtaState(const char* phase, uint8_t pct, const char* err);

private:
    std::vector<PnexPin> pins_;      // active table (ProvisionAck or declared)
    std::vector<PnexPin> declared_;  // sketch declarations (announce)
    // Extra caps registered by optional modules (addCap).
    const char* extra_cap_ids_[PNEX_MAX_EXTRA_CAPS] = {};
    const char* extra_cap_families_[PNEX_MAX_EXTRA_CAPS] = {};
    uint8_t extra_caps_ = 0;
    PnexServerMsgHook msg_hook_ = nullptr;
    // Custom metrics (addMetric) and commands (onCommand).
    const char* metric_ids_[PNEX_MAX_METRICS] = {};
    const char* metric_units_[PNEX_MAX_METRICS] = {};
    uint8_t metrics_ = 0;
    const char* command_names_[PNEX_MAX_COMMANDS] = {};
    PnexCommandHandler command_handlers_[PNEX_MAX_COMMANDS] = {};
    uint8_t commands_ = 0;

    friend void pnex_on_ws_closed();
    friend void pnex_on_ws_message(const String& plain);

    bool declare(PnexPin p);
    PnexPin* pin_by_gpio(uint8_t gpio);
    static bool apply_pin(PnexPin& p);
    void forceAllOff();
    void sendAnnounce();
    void sendStateReport(const PnexPin& p);
    void handleServerMessage(const String& plain);
    void applyProvisionAck(JsonDocument& doc);
    void handleSetMode(JsonDocument& doc);
    void handleWrite(JsonDocument& doc);
    void handleSubscribe(JsonDocument& doc);
    void handleOtaAvailable(JsonDocument& doc);
    void handleCommand(JsonDocument& doc);
    // Fills the metric StateReport envelope; false when `id` is unknown.
    bool metricReport(const char* id, JsonDocument& doc);
    // Runs a download recorded by handleOtaAvailable (from loop()); true
    // when one was attempted (success reboots, so true = it failed).
    bool runPendingOta();
};

// Transport trampolines (bare C pointers → single instance, see below).
void pnex_on_ws_closed();
void pnex_on_ws_message(const String& plain);

#endif  // PNEX_H
