//
// pnex-transport — transport layer shared by the firmwares (edge-model.md §3,
// roadmap F1): WiFi + WebSocket + ChaCha framing + PONG bookkeeping.
//
// The lib holds the shared MECHANICS; the POLICY stays in the main of
// each firmware (backoff, safe-states, proto announce, display):
//   - pnex_wifi_connect()  : WiFi wait loop (number of attempts as a
//     parameter — 40 for the generic one), wdtFeed during
//     the wait;
//   - pnex_ws_connect()    : one WS attempt (clean close if already
//     open, last_pong re-armed) — backoff/retries belong to the main;
//   - pnex_ws_poll()       : client.poll() then clock RE-READ after the poll
//     (lesson 2026-09-03: millis() advances during the callback, a
//     comparison with a pre-poll `now` wraps as unsigned and
//     triggers a ghost "PONG timeout") + PONG timeout detection
//     (close; the safe-states stay in the main on the next round);
//   - internal PONG bookkeeping: text "PONG" (server) AND WS pong
//     (GotPong) re-arm the counter; the timeout is configurable
//     (0 = disabled).
//
// Consumers: the generic firmwares (pin_slave profile) and custom sketches.
//
// pnex_config.h is private to the lib: pnex_transport.cpp is the only
// translation unit that includes it; sketches use the getters
// (pnex_host(), pnex_device_id(), pnex_token()…).
//
#ifndef PNEX_TRANSPORT_H
#define PNEX_TRANSPORT_H

#include <Arduino.h>

// Optional callbacks (null pointer = ignored).
typedef void (*PnexOnMessageFn)(const String& plain);  // decrypted frame, PONG excluded
typedef void (*PnexOnPongFn)();                        // server text "PONG"
typedef void (*PnexOnConnectedFn)();                   // event ConnectionOpened
typedef void (*PnexOnClosedFn)();                      // event ConnectionClosed

// Optional tick during the WiFi wait — pnex.cpp passes the screen tick
// (the transport stays display-agnostic: bare pointer, no screen include).
typedef void (*PnexWaitTickFn)();

struct PnexTransportInit {
    // WebSocket path, `/ws/device`. The scheme (ws/wss) and the
    // token/device_id params (base64, decode_param contract) are internal.
    const char* ws_path;

    // Server silence > pong_timeout_ms → close (pnex_ws_poll). 0 = off.
    unsigned long pong_timeout_ms;

    // GotPing (WS protocol ping) → automatic pong reply. True for the
    // generic one (client.ping() on GotPing).
    bool reply_ws_ping;

    PnexOnMessageFn on_message;
    PnexOnPongFn on_pong;
    PnexOnConnectedFn on_connected;
    PnexOnClosedFn on_closed;
};

/// Decodes the compiled config (base64 WIFI_SSID/WIFI_PASSWORD/HOST/
/// TOKEN/DEVICE_ID), loads the Noise pre-shared key (ENCRYPTION_KEY), the
/// CA pin and the TLS client identity, builds the wss URL and registers
/// the callbacks. Call once from setup().
void pnex_transport_setup(const PnexTransportInit& init);

// Decoded config (the mains no longer include config.h — see rule above).
const char* pnex_host();
const char* pnex_device_id();
const char* pnex_conn_string();  // Connection URL for logs, token masked (SEC-W7)
bool pnex_crypto_ready();        // valid Noise pre-shared key loaded

// Device token (decoded), sent as `Authorization: Bearer` by every
// device client (WS, camera, OTA download — D154).
const char* pnex_token();

// Ed25519 public key (hex) that OTA images must be signed with (SEC-18);
// empty when the firmware was built without one.
const char* pnex_ota_pubkey();

/// Blocking WiFi wait loop (delay 500 ms/attempt, wdtFeed, progress
/// dots) — returns the final state. `tick` (optional) is called on
/// each iteration BEFORE the delay: the screen stays alive during the
/// blocking connect without this layer knowing about it.
bool pnex_wifi_connect(unsigned max_attempts, PnexWaitTickFn tick = nullptr);

/// One connection attempt: clean close if already open (delay 100 ms),
/// socket connect, then the Noise handshake (D156, bounded by 10 s). True
/// = link established, ready to announce; the retries stay in the main.
bool pnex_ws_connect();

/// poll() + re-read clock + PONG timeout (close when exceeded — see above).
/// The PING is left to the main (pnex_ws_send_ping + the main's timer): the
/// exact cadence (>= 5 s vs > 5 s) and display effects already diverge
/// between firmwares.
void pnex_ws_poll();

/// Encrypted "PING" — schedule and timer in the main.
void pnex_ws_send_ping();

/// Socket open AND Noise link established.
bool pnex_ws_available();
/// Plain text, sealed on the Noise link when sent (D156); no-op when empty
/// or before the link is established. The JSON size limit stays in the
/// main (sendJson).
void pnex_ws_send(const char* plain);
void pnex_ws_close();

#endif  // PNEX_TRANSPORT_H
