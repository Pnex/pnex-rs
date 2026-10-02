//
// pnex-camera — implementation (contract and wiring in pnex_camera.h).
// The whole translation unit is gated: ESP8266 / C3 / S3 builds of the
// lib compile it to nothing and never include esp_camera.h.
//
#include "pnex_camera.h"

#if PNEX_CAMERA_ACTIVE

#include <ArduinoJson.h>
#include <ArduinoWebsockets.h>
#include <WiFi.h>
#include <esp_camera.h>
#include <esp_heap_caps.h>
#include <esp_wifi.h>

#include "Pnex.h"
#include "chacha_crypto.h"
#include "pnex_camera_frame.h"
#include "pnex_tls.h"
#include "pnex_transport.h"

using namespace websockets;

// ───────────────────── AI-Thinker ESP32-CAM pin map ─────────────────────
// Reserved in the server board profile (pnex-core catalog/boards/esp32cam_ai_thinker.rs): the
// provisioning never exposes them.
namespace {

constexpr int CAM_PIN_PWDN = 32;
constexpr int CAM_PIN_RESET = -1;  // not wired on the AI-Thinker
constexpr int CAM_PIN_XCLK = 0;
constexpr int CAM_PIN_SIOD = 26;
constexpr int CAM_PIN_SIOC = 27;
constexpr int CAM_PIN_D7 = 35;
constexpr int CAM_PIN_D6 = 34;
constexpr int CAM_PIN_D5 = 39;
constexpr int CAM_PIN_D4 = 36;
constexpr int CAM_PIN_D3 = 21;
constexpr int CAM_PIN_D2 = 19;
constexpr int CAM_PIN_D1 = 18;
constexpr int CAM_PIN_D0 = 5;
constexpr int CAM_PIN_VSYNC = 25;
constexpr int CAM_PIN_HREF = 23;
constexpr int CAM_PIN_PCLK = 22;

// Sensor clock. Some XCLK values jam the AI-Thinker WiFi radio (sweep on
// a real board, 2026-09-30, fb_count 1, same AP):
//   20 MHz: 100 % ping loss      16 MHz: 0 % loss, 5 fps, 83 ms per send
//   10 MHz: never associates      8 MHz: 0 % loss, 5 fps, 109 ms per send
//    5 MHz: never associates
// With the sensor off the radio is clean, so it is sensor EMI, not CPU or
// heap (see also jomjol/AI-on-the-edge-device#3558). 16 MHz keeps the most
// sensor frame rate among the clean values; override with -D if a board
// or channel misbehaves.
#ifndef PNEX_CAMERA_XCLK_HZ
#define PNEX_CAMERA_XCLK_HZ 16000000
#endif

// Frame buffers. 2 + GRAB_LATEST keeps the DMA capturing at the sensor
// rate; 1 + GRAB_WHEN_EMPTY only captures the frames we actually take.
#ifndef PNEX_CAMERA_FB_COUNT
#define PNEX_CAMERA_FB_COUNT 1
#endif

// Server-side cap on one decrypted frame (pnex-core DEFAULT_MAX_FRAME_BYTES,
// header included): bigger frames are dropped here rather than on the wire.
constexpr size_t MAX_FRAME_BYTES = 512 * 1024;

constexpr uint32_t BACKOFF_MIN_MS = 1000;
constexpr uint32_t BACKOFF_MAX_MS = 30000;
constexpr uint32_t KEEPALIVE_MS = 15000;

// ───────────────────────────── State ─────────────────────────────

PnexDevice* s_dev = nullptr;
bool s_cam_ok = false;

// Desired state (last camera_config) — default: not streaming.
bool s_enabled = false;
uint8_t s_fps = 5;

WebsocketsClient s_ws;
bool s_ws_up = false;  // tracked from the events (available() lags on close)
char s_url[256];

uint32_t s_backoff_ms = BACKOFF_MIN_MS;
// The backoff only resets once a session stayed up STABLE_MS: a server
// closing right after the upgrade (4003 anti-clone, 4002 auth) must not
// turn into a 1 s reconnect loop.
constexpr uint32_t STABLE_MS = 10000;
// Frames delivered by the current session. A session that carried frames
// was accepted by the server: losing it is a transport drop (weak radio),
// not a rejection, so the next attempt comes back after the minimum delay
// instead of a doubled one (30 s gaps in a live view otherwise).
uint32_t s_session_frames = 0;
unsigned long s_connected_since_ms = 0;
unsigned long s_next_connect_ms = 0;
unsigned long s_last_frame_ms = 0;
unsigned long s_last_keepalive_ms = 0;
uint32_t s_seq = 0;

// Encrypted frame buffer in PSRAM (grow-only): nonce(12) || header(16) ||
// JPEG, encrypted in place.
uint8_t* s_buf = nullptr;
size_t s_buf_cap = 0;

// Stats, logged every ~30 s while streaming (send time = encrypt + WS
// write, reset at each log).
uint32_t s_sent = 0;
uint32_t s_dropped = 0;
unsigned long s_last_stats_ms = 0;
uint32_t s_send_ms_total = 0;
uint32_t s_send_ms_max = 0;
uint32_t s_send_count = 0;
size_t s_bytes_total = 0;

// ─────────────────────────── Helpers ───────────────────────────

bool framesize_from_wire(const char* id, framesize_t& out) {
    struct Entry {
        const char* id;
        framesize_t size;
    };
    // Wire ids of pnex_core::camera::FrameSize.
    static const Entry TABLE[] = {
        {"qvga", FRAMESIZE_QVGA}, {"cif", FRAMESIZE_CIF},   {"vga", FRAMESIZE_VGA},
        {"svga", FRAMESIZE_SVGA}, {"xga", FRAMESIZE_XGA},   {"hd", FRAMESIZE_HD},
        {"sxga", FRAMESIZE_SXGA}, {"uxga", FRAMESIZE_UXGA},
    };
    for (const auto& e : TABLE) {
        if (strcmp(id, e.id) == 0) {
            out = e.size;
            return true;
        }
    }
    return false;
}

bool ensure_buffer(size_t need) {
    if (need <= s_buf_cap) {
        return true;
    }
    // Round up to 16 KB steps to limit reallocations while the JPEG size
    // drifts with the scene.
    const size_t cap = (need + 0x3FFF) & ~(size_t)0x3FFF;
    uint8_t* next = (uint8_t*)heap_caps_malloc(cap, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (next == nullptr) {
        Serial.printf("[CAM] PSRAM alloc of %u bytes failed\n", (unsigned)cap);
        return false;
    }
    if (s_buf != nullptr) {
        heap_caps_free(s_buf);
    }
    s_buf = next;
    s_buf_cap = cap;
    return true;
}

void ws_close() {
    if (s_ws_up || s_ws.available()) {
        s_ws.close();
        Serial.println("[CAM] camera WS closed");
    }
    s_ws_up = false;
}

void on_ws_event(WebsocketsEvent event, String data) {
    if (event == WebsocketsEvent::ConnectionOpened) {
        s_ws_up = true;
    } else if (event == WebsocketsEvent::ConnectionClosed) {
        s_ws_up = false;
    } else if (event == WebsocketsEvent::GotPing) {
        s_ws.pong(data);
    }
}

void on_ws_message(WebsocketsMessage) {
    // Uplink only: the server just answers "PONG" to the keepalive.
}

// Deferred connect (loop context, never from the WS callback: the TLS
// handshake nested in a frame handler overflowed the loopTask stack for
// the OTA — same rule here).
void try_connect(unsigned long now) {
    if ((long)(now - s_next_connect_ms) < 0) {
        return;
    }
    Serial.printf("[CAM] connecting camera WS (%s)\n", pnex_use_tls() ? "wss" : "ws");
    // Modem sleep off: with power save on, the radio only wakes on DTIM
    // beacons and every TCP round trip costs 100 ms to 2 s (measured RTT
    // avg 650 ms on an AI-Thinker) — the stream crawls below 1 fps.
    // Idempotent; re-applied on each connect in case the lib reset it.
    WiFi.setSleep(false);
    if (s_ws.connect(s_url)) {
        s_ws_up = true;
        s_session_frames = 0;
        s_connected_since_ms = millis();
        s_last_keepalive_ms = now;
        s_last_frame_ms = 0;
        Serial.println("[CAM] camera WS connected");
        return;
    }
    s_ws_up = false;
    s_next_connect_ms = millis() + s_backoff_ms;
    Serial.printf("[CAM] camera WS connect failed, retry in %u ms\n", (unsigned)s_backoff_ms);
    s_backoff_ms = s_backoff_ms * 2 > BACKOFF_MAX_MS ? BACKOFF_MAX_MS : s_backoff_ms * 2;
}

// Largest WS frame written in one go. Over wss a write is one TLS record
// (16 KB max) and the WS library ignores partial writes: a bigger frame
// went out truncated and the session died after a couple of frames (VGA
// JPEGs are 15-25 KB). Bigger payloads go out as one fragmented message
// (empty first fragment, continuations, empty final one), which the
// server reassembles into a single binary message.
constexpr size_t WS_CHUNK = 8192;

bool send_binary_chunked(const uint8_t* data, size_t len) {
    if (len <= WS_CHUNK) {
        return s_ws.sendBinary((const char*)data, len);
    }
    if (!s_ws.streamBinary("")) {
        return false;
    }
    for (size_t off = 0; off < len; off += WS_CHUNK) {
        const size_t n = len - off < WS_CHUNK ? len - off : WS_CHUNK;
        if (!s_ws.sendBinary((const char*)data + off, n)) {
            return false;
        }
    }
    return s_ws.end("");
}

void send_frame() {
    camera_fb_t* fb = esp_camera_fb_get();
    if (fb == nullptr) {
        ++s_dropped;
        return;
    }
    const size_t plain_len = pnex_camera_frame::HEADER_LEN + fb->len;
    if (fb->format != PIXFORMAT_JPEG || fb->len < 2 || plain_len > MAX_FRAME_BYTES ||
        !ensure_buffer(CRYPTO_NONCE_LEN + plain_len)) {
        esp_camera_fb_return(fb);
        ++s_dropped;
        return;
    }
    uint8_t* plain = s_buf + CRYPTO_NONCE_LEN;
    pnex_camera_frame::encode_header(plain, ++s_seq, (uint32_t)millis(), (uint16_t)fb->width,
                                     (uint16_t)fb->height);
    memcpy(plain + pnex_camera_frame::HEADER_LEN, fb->buf, fb->len);
    // Frame buffer back to the driver BEFORE the (slow) network send.
    esp_camera_fb_return(fb);

    // In place: plain aliases s_buf + 12. Without a key it moves the clear
    // frame to s_buf (mock local server).
    const unsigned long t0 = millis();
    const size_t wire_len = cryptoEncryptBinary(plain, plain_len, s_buf);
    if (wire_len == 0 || !send_binary_chunked(s_buf, wire_len)) {
        ++s_dropped;
        return;
    }
    const uint32_t took = (uint32_t)(millis() - t0);
    s_send_ms_total += took;
    s_send_count += 1;
    s_bytes_total += wire_len;
    if (took > s_send_ms_max) {
        s_send_ms_max = took;
    }
    ++s_sent;
    ++s_session_frames;
}

// ───────────────────── Server message hook ─────────────────────

// ServerMsg::CameraConfig — applies the sensor settings immediately (short
// SCCB writes) and records the stream state; the WS connect/close runs
// from pnex_camera_loop().
bool on_server_message(JsonDocument& doc) {
    const char* type = doc["t"] | "";
    if (strcmp(type, "camera_config") != 0) {
        return false;
    }
    const char* cmd_id = doc["cmd_id"] | "";
    if (!s_cam_ok) {
        s_dev->sendAck(cmd_id, false, "camera not initialized");
        return true;
    }
    framesize_t size;
    if (!framesize_from_wire(doc["framesize"] | "", size)) {
        s_dev->sendAck(cmd_id, false, "unknown framesize");
        return true;
    }
    int quality = doc["quality"] | 12;
    if (quality < 10) quality = 10;
    if (quality > 63) quality = 63;
    int fps = doc["fps"] | 5;
    if (fps < 1) fps = 1;
    if (fps > 25) fps = 25;

    sensor_t* s = esp_camera_sensor_get();
    if (s == nullptr) {
        s_dev->sendAck(cmd_id, false, "camera sensor unavailable");
        return true;
    }
    s->set_framesize(s, size);
    s->set_quality(s, quality);
    s->set_vflip(s, (doc["vflip"] | false) ? 1 : 0);
    s->set_hmirror(s, (doc["hmirror"] | false) ? 1 : 0);

    s_fps = (uint8_t)fps;
    const bool enabled = doc["enabled"] | false;
    if (enabled && !s_enabled) {
        // Fresh demand: connect on the next loop turn, no stale backoff.
        s_backoff_ms = BACKOFF_MIN_MS;
        s_next_connect_ms = millis();
    }
    s_enabled = enabled;
    Serial.printf("[CAM] config: enabled=%d framesize=%s quality=%d fps=%d\n", enabled,
                  doc["framesize"] | "?", quality, fps);
    s_dev->sendAck(cmd_id, true, nullptr);
    return true;
}

}  // namespace

// ─────────────────────────── Public API ───────────────────────────

bool pnex_camera_begin(PnexDevice& device) {
    s_dev = &device;
    // Serial is (re)started by pnex.begin(); started here too so the init
    // logs are not lost (begin twice is harmless on ESP32).
    Serial.begin(115200);
    device.onServerMessage(&on_server_message);

    if (!psramFound()) {
        Serial.println("[CAM] no PSRAM — camera disabled");
        return false;
    }

#if defined(PNEX_CAMERA_SKIP_INIT) && PNEX_CAMERA_SKIP_INIT == 1
    // Diagnostic build: radio behaviour without the sensor running.
    WiFi.setSleep(false);
    Serial.println("[CAM] PNEX_CAMERA_SKIP_INIT — sensor not started");
    return false;
#endif

    camera_config_t cfg = {};
    cfg.pin_pwdn = CAM_PIN_PWDN;
    cfg.pin_reset = CAM_PIN_RESET;
    cfg.pin_xclk = CAM_PIN_XCLK;
    cfg.pin_sccb_sda = CAM_PIN_SIOD;
    cfg.pin_sccb_scl = CAM_PIN_SIOC;
    cfg.pin_d7 = CAM_PIN_D7;
    cfg.pin_d6 = CAM_PIN_D6;
    cfg.pin_d5 = CAM_PIN_D5;
    cfg.pin_d4 = CAM_PIN_D4;
    cfg.pin_d3 = CAM_PIN_D3;
    cfg.pin_d2 = CAM_PIN_D2;
    cfg.pin_d1 = CAM_PIN_D1;
    cfg.pin_d0 = CAM_PIN_D0;
    cfg.pin_vsync = CAM_PIN_VSYNC;
    cfg.pin_href = CAM_PIN_HREF;
    cfg.pin_pclk = CAM_PIN_PCLK;
    cfg.xclk_freq_hz = PNEX_CAMERA_XCLK_HZ;
    cfg.ledc_timer = LEDC_TIMER_0;
    cfg.ledc_channel = LEDC_CHANNEL_0;
    cfg.pixel_format = PIXFORMAT_JPEG;
    // Init at the LARGEST size: the JPEG frame buffers are sized once from
    // the init framesize, so any later camera_config (up to UXGA) fits.
    cfg.frame_size = FRAMESIZE_UXGA;
    cfg.jpeg_quality = 12;
    cfg.fb_count = PNEX_CAMERA_FB_COUNT;
    cfg.fb_location = CAMERA_FB_IN_PSRAM;
    cfg.grab_mode = PNEX_CAMERA_FB_COUNT > 1 ? CAMERA_GRAB_LATEST : CAMERA_GRAB_WHEN_EMPTY;

    const esp_err_t err = esp_camera_init(&cfg);
    if (err != ESP_OK) {
        Serial.printf("[CAM] esp_camera_init failed: 0x%x — camera disabled\n", (unsigned)err);
        return false;
    }
    // Idle default until the first camera_config (VGA, server default).
    sensor_t* s = esp_camera_sensor_get();
    if (s != nullptr) {
        s->set_framesize(s, FRAMESIZE_VGA);
    }
    s_cam_ok = true;

    s_ws.onEvent(on_ws_event);
    s_ws.onMessage(on_ws_message);

    // Streaming device: no WiFi power save (applied at STA start by the
    // core, pnex.begin() runs after this).
    WiFi.setSleep(false);

    device.addCap("camera", "video");
    Serial.printf("[CAM] camera ready (OV2640, PSRAM, xclk %u Hz)\n", (unsigned)PNEX_CAMERA_XCLK_HZ);
    return true;
}

void pnex_camera_loop() {
    // Health line every 10 s, streaming or not: internal heap (WiFi / lwIP
    // buffers live there), radio and power-save state.
    static unsigned long s_last_diag_ms = 0;
    if (millis() - s_last_diag_ms >= 10000) {
        s_last_diag_ms = millis();
        wifi_ps_type_t ps = WIFI_PS_NONE;
        esp_wifi_get_ps(&ps);
        Serial.printf("[DIAG] heap_int free=%u min=%u largest=%u psram_free=%u rssi=%d ps=%d cam=%d\n",
                      (unsigned)heap_caps_get_free_size(MALLOC_CAP_INTERNAL),
                      (unsigned)heap_caps_get_minimum_free_size(MALLOC_CAP_INTERNAL),
                      (unsigned)heap_caps_get_largest_free_block(MALLOC_CAP_INTERNAL),
                      (unsigned)heap_caps_get_free_size(MALLOC_CAP_SPIRAM), (int)WiFi.RSSI(),
                      (int)ps, (int)s_cam_ok);
    }
    if (!s_cam_ok) {
        return;
    }
    // Camera WS URL: same scheme / host / b64 credentials / TLS posture as
    // /ws/device. Built on the first turn: pnex_host() and the CA are only
    // decoded by pnex.begin() (transport setup), after pnex_camera_begin().
    static bool url_ready = false;
    if (!url_ready) {
        snprintf(s_url, sizeof(s_url), "%s://%s/ws/camera?token=%s&device_id=%s",
                 pnex_use_tls() ? "wss" : "ws", pnex_host(), pnex_token_b64(),
                 pnex_device_id_b64());
        if (pnex_use_tls()) {
            pnex_tls_apply(s_ws);
        }
        url_ready = true;
    }

    // Control WS down → stop streaming and forget the demand: the server
    // re-pushes CameraConfig after the next announce.
    if (!pnex_ws_available()) {
        if (s_enabled) {
            Serial.println("[CAM] control WS down — streaming stopped");
        }
        s_enabled = false;
        ws_close();
        return;
    }

    if (!s_enabled) {
        ws_close();
        return;
    }

    const unsigned long now = millis();
    if (!s_ws_up) {
        try_connect(now);
        return;
    }

    s_ws.poll();
    if (!s_ws_up || !s_ws.available()) {
        s_ws_up = false;
        if (s_session_frames > 0) {
            s_backoff_ms = BACKOFF_MIN_MS;
        }
        s_next_connect_ms = millis() + s_backoff_ms;
        Serial.printf("[CAM] camera WS lost after %u frames, retry in %u ms\n",
                      (unsigned)s_session_frames, (unsigned)s_backoff_ms);
        s_backoff_ms = s_backoff_ms * 2 > BACKOFF_MAX_MS ? BACKOFF_MAX_MS : s_backoff_ms * 2;
        return;
    }

    if (s_backoff_ms != BACKOFF_MIN_MS && now - s_connected_since_ms >= STABLE_MS) {
        s_backoff_ms = BACKOFF_MIN_MS;
    }

    if (now - s_last_keepalive_ms >= KEEPALIVE_MS) {
        // Clear text on purpose: /ws/camera answers a plain "PING".
        s_ws.send("PING");
        s_last_keepalive_ms = now;
    }

    const unsigned long interval = 1000UL / (s_fps ? s_fps : 1);
    if (s_last_frame_ms == 0 || now - s_last_frame_ms >= interval) {
        s_last_frame_ms = now;
        send_frame();
    }

    if (now - s_last_stats_ms >= 30000) {
        s_last_stats_ms = now;
        const unsigned avg_ms = s_send_count ? (unsigned)(s_send_ms_total / s_send_count) : 0;
        const unsigned avg_kb =
            s_send_count ? (unsigned)(s_bytes_total / s_send_count / 1024) : 0;
        Serial.printf(
            "[CAM] frames sent=%u dropped=%u fps=%u send_ms avg=%u max=%u frame_kb=%u rssi=%d\n",
            (unsigned)s_sent, (unsigned)s_dropped, (unsigned)s_fps, avg_ms,
            (unsigned)s_send_ms_max, avg_kb, (int)WiFi.RSSI());
        s_send_ms_total = 0;
        s_send_ms_max = 0;
        s_send_count = 0;
        s_bytes_total = 0;
    }
}

#endif  // PNEX_CAMERA_ACTIVE
