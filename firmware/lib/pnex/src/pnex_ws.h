//
// pnex-ws — minimal RFC 6455 WebSocket client, PneX's own code (Apache-2.0
// like the rest of the lib). It replaces ArduinoWebsockets (GPL-3.0): every
// third-party brick linked into a firmware must stay permissively licensed.
//
// Scope = what the PneX device links need, nothing more:
// - client only, over any Arduino `Client` (WiFiClient / WiFiClientSecure)
//   built by the caller, so the TLS posture (CA pinning, lean BearSSL
//   buffers on the ESP8266) stays outside this file;
// - patient handshake: the HTTP 101 answer gets PNEX_WS_HANDSHAKE_TIMEOUT_MS
//   and the read yields while waiting (O20: a 1 s budget dropped slow WiFi
//   links, a busy loop trips the ESP8266 soft watchdog), and the
//   Sec-WebSocket-Accept value is checked;
// - text / binary messages, server fragments reassembled, pings answered
//   automatically, close handshake;
// - writes go out in masked chunks of PNEX_WS_TX_CHUNK bytes and partial
//   writes are retried, so any message size works over TLS (no 16 KB
//   record truncation for camera frames).
//
// Single-threaded: call everything from the Arduino loop.
//

#ifndef PNEX_WS_H
#define PNEX_WS_H

#include <Arduino.h>
#include <Client.h>

#include <memory>

#ifndef PNEX_WS_HANDSHAKE_TIMEOUT_MS
#define PNEX_WS_HANDSHAKE_TIMEOUT_MS 5000
#endif

// Budget for the rest of a frame once its first byte arrived, and for a
// stalled write.
#ifndef PNEX_WS_IO_TIMEOUT_MS
#define PNEX_WS_IO_TIMEOUT_MS 5000
#endif

// Largest message accepted from the server (bigger = close 1009). Server
// frames are small (encrypted commands): a few KB at most.
#ifndef PNEX_WS_MAX_MESSAGE
#if defined(ESP32)
#define PNEX_WS_MAX_MESSAGE 16384
#else
#define PNEX_WS_MAX_MESSAGE 4096
#endif
#endif

// Size of one masked write (header included). Matches the lean BearSSL TX
// buffer on the ESP8266; bigger on ESP32 for the camera throughput.
#ifndef PNEX_WS_TX_CHUNK
#if defined(ESP32)
#define PNEX_WS_TX_CHUNK 4096
#else
#define PNEX_WS_TX_CHUNK 512
#endif
#endif

enum class PnexWsEvent : uint8_t { Opened, Closed, GotPing, GotPong };

// `data` is null-terminated (handy for text), `len` excludes the terminator.
using PnexWsMessageFn = void (*)(const char* data, size_t len, bool binary);
using PnexWsEventFn = void (*)(PnexWsEvent event);

class PnexWsClient {
public:
    PnexWsClient() = default;
    ~PnexWsClient();
    PnexWsClient(const PnexWsClient&) = delete;
    PnexWsClient& operator=(const PnexWsClient&) = delete;

    void onMessage(PnexWsMessageFn fn) { on_message_ = fn; }
    // Device credential sent as `Authorization: Bearer <token>` on the
    // upgrade request (D154: never in the URL). The pointer must outlive
    // the client (compiled config).
    void setAuthToken(const char* token) { auth_token_ = token; }
    void onEvent(PnexWsEventFn fn) { on_event_ = fn; }

    // Takes ownership of `tcp` (already configured by the caller), opens
    // the TCP/TLS connection and upgrades it. Any previous connection is
    // closed first. true = open (Opened was fired).
    bool connect(Client* tcp, const char* host, uint16_t port, const char* path);

    // Open and the TCP link still up. A dropped link is noticed here and
    // in poll(): Closed fires once.
    bool available();

    // Reads the pending frames and fires the callbacks.
    void poll();

    bool send(const char* text, size_t len);
    bool send(const String& text) { return send(text.c_str(), text.length()); }
    bool sendBinary(const uint8_t* data, size_t len);
    bool ping();

    // Close frame (best effort) + TCP stop; Closed fires if it was open.
    void close();

    // The transport the client owns (nullptr before the first connect).
    Client* tcp() { return tcp_.get(); }

private:
    bool handshake(const char* host, const char* path);
    const char* auth_token_ = nullptr;
    bool read_line(char* buf, size_t cap, unsigned long deadline);
    bool read_exact(uint8_t* buf, size_t n);
    bool write_all(const uint8_t* data, size_t n);
    bool write_frame(uint8_t opcode, const uint8_t* data, size_t len);
    bool read_frame();
    void reset_message();
    void teardown();
    void emit(PnexWsEvent event);

    std::unique_ptr<Client> tcp_;
    bool open_ = false;
    PnexWsMessageFn on_message_ = nullptr;
    PnexWsEventFn on_event_ = nullptr;

    // Message being reassembled from fragments.
    uint8_t* rx_ = nullptr;
    size_t rx_len_ = 0;
    bool rx_binary_ = false;
    bool rx_active_ = false;
};

// Opens `url` ("ws[s]://host[:port]/path?query") on `client` over a fresh
// TCP client with the shared TLS posture (pnex_tls_apply) for wss.
bool pnex_ws_open(PnexWsClient& client, const char* url);

#endif  // PNEX_WS_H
