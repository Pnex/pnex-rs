//
// pnex-ws — implementation (see pnex_ws.h).
//
#include "pnex_ws.h"

#if defined(ESP32)
#include <WiFi.h>
#include <WiFiClientSecure.h>
#include <esp_random.h>
#else
#include <ESP8266WiFi.h>
#endif

#include "pnex_crypto.h"  // cryptoB64Encode
#include "pnex_tls.h"

namespace {

constexpr uint8_t OP_CONT = 0x0;
constexpr uint8_t OP_TEXT = 0x1;
constexpr uint8_t OP_BINARY = 0x2;
constexpr uint8_t OP_CLOSE = 0x8;
constexpr uint8_t OP_PING = 0x9;
constexpr uint8_t OP_PONG = 0xA;

// Frames handled per poll(): keeps the loop responsive under a burst.
constexpr int MAX_FRAMES_PER_POLL = 8;

uint32_t random32() {
#if defined(ESP32)
    return esp_random();
#else
    return ESP.random();
#endif
}

// ── SHA-1 (RFC 3174), only for the Sec-WebSocket-Accept check ──

uint32_t rol(uint32_t v, int n) { return (v << n) | (v >> (32 - n)); }

void sha1_block(uint32_t h[5], const uint8_t* p) {
    uint32_t w[80];
    for (int i = 0; i < 16; ++i) {
        w[i] = (uint32_t)p[4 * i] << 24 | (uint32_t)p[4 * i + 1] << 16 |
               (uint32_t)p[4 * i + 2] << 8 | p[4 * i + 3];
    }
    for (int i = 16; i < 80; ++i) {
        w[i] = rol(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
    }
    uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4];
    for (int i = 0; i < 80; ++i) {
        uint32_t f, k;
        if (i < 20) {
            f = (b & c) | (~b & d);
            k = 0x5A827999;
        } else if (i < 40) {
            f = b ^ c ^ d;
            k = 0x6ED9EBA1;
        } else if (i < 60) {
            f = (b & c) | (b & d) | (c & d);
            k = 0x8F1BBCDC;
        } else {
            f = b ^ c ^ d;
            k = 0xCA62C1D6;
        }
        const uint32_t t = rol(a, 5) + f + e + k + w[i];
        e = d;
        d = c;
        c = rol(b, 30);
        b = a;
        a = t;
    }
    h[0] += a;
    h[1] += b;
    h[2] += c;
    h[3] += d;
    h[4] += e;
}

void sha1(const uint8_t* msg, size_t len, uint8_t out[20]) {
    uint32_t h[5] = {0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0};
    uint8_t block[64];
    size_t off = 0;
    for (; off + 64 <= len; off += 64) {
        sha1_block(h, msg + off);
    }
    size_t rest = len - off;
    memcpy(block, msg + off, rest);
    block[rest++] = 0x80;
    if (rest > 56) {
        memset(block + rest, 0, 64 - rest);
        sha1_block(h, block);
        rest = 0;
    }
    memset(block + rest, 0, 56 - rest);
    const uint64_t bits = (uint64_t)len * 8;
    for (int i = 0; i < 8; ++i) {
        block[56 + i] = (uint8_t)(bits >> (56 - 8 * i));
    }
    sha1_block(h, block);
    for (int i = 0; i < 20; ++i) {
        out[i] = (uint8_t)(h[i / 4] >> (24 - 8 * (i % 4)));
    }
}

// Case-insensitive "starts with" for header names.
bool header_is(const char* line, const char* name) {
    return strncasecmp(line, name, strlen(name)) == 0;
}

}  // namespace

// ───────────────────────── Lifecycle ─────────────────────────

PnexWsClient::~PnexWsClient() {
    reset_message();
    if (tcp_) {
        tcp_->stop();
    }
}

void PnexWsClient::emit(PnexWsEvent event) {
    if (on_event_) {
        on_event_(event);
    }
}

void PnexWsClient::reset_message() {
    free(rx_);
    rx_ = nullptr;
    rx_len_ = 0;
    rx_active_ = false;
}

// Link gone (or given up): stop TCP, drop any partial message, Closed once.
void PnexWsClient::teardown() {
    reset_message();
    if (tcp_) {
        tcp_->stop();
    }
    if (open_) {
        open_ = false;
        emit(PnexWsEvent::Closed);
    }
}

bool PnexWsClient::connect(Client* tcp, const char* host, uint16_t port, const char* path) {
    teardown();
    tcp_.reset(tcp);
    if (!tcp_ || !tcp_->connect(host, port)) {
        Serial.printf("[WS] TCP connect to %s:%u failed\n", host, (unsigned)port);
        if (tcp_) {
            tcp_->stop();
        }
        return false;
    }
    if (!handshake(host, path)) {
        tcp_->stop();
        return false;
    }
    open_ = true;
    emit(PnexWsEvent::Opened);
    return true;
}

bool PnexWsClient::available() {
    if (open_ && !(tcp_ && tcp_->connected())) {
        teardown();
    }
    return open_;
}

void PnexWsClient::close() {
    if (open_ && tcp_ && tcp_->connected()) {
        const uint8_t normal[2] = {0x03, 0xE8};  // 1000
        write_frame(OP_CLOSE, normal, sizeof(normal));
    }
    teardown();
}

// ───────────────────────── Handshake ─────────────────────────

// One header line (CRLF stripped, truncated to `cap`), waiting for bytes
// until `deadline` and yielding meanwhile.
bool PnexWsClient::read_line(char* buf, size_t cap, unsigned long deadline) {
    size_t n = 0;
    while ((long)(millis() - deadline) < 0) {
        const int ch = tcp_->read();
        if (ch < 0) {
            if (!tcp_->connected()) {
                return false;
            }
            delay(1);  // nothing yet: let WiFi/TCP run (and the WDT)
            continue;
        }
        if (ch == '\n') {
            if (n > 0 && buf[n - 1] == '\r') {
                --n;
            }
            buf[n] = '\0';
            return true;
        }
        if (n + 1 < cap) {
            buf[n++] = (char)ch;
        }
    }
    return false;
}

bool PnexWsClient::handshake(const char* host, const char* path) {
    uint8_t nonce[16];
    for (int i = 0; i < 16; i += 4) {
        const uint32_t r = random32();
        memcpy(nonce + i, &r, 4);
    }
    char key[32];
    cryptoB64Encode(nonce, sizeof(nonce), key);

    // Host header without the port: same request line as before (the
    // server routes on the path only).
    tcp_->printf(
        "GET %s HTTP/1.1\r\n"
        "Host: %s\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        "Sec-WebSocket-Key: %s\r\n"
        "Sec-WebSocket-Version: 13\r\n",
        path, host, key);
    if (auth_token_ != nullptr && auth_token_[0] != '\0') {
        tcp_->printf("Authorization: Bearer %s\r\n", auth_token_);
    }
    tcp_->print("\r\n");

    // Expected Sec-WebSocket-Accept = base64(SHA-1(key + RFC GUID)).
    char concat[64];
    snprintf(concat, sizeof(concat), "%s258EAFA5-E914-47DA-95CA-C5AB0DC85B11", key);
    uint8_t digest[20];
    sha1((const uint8_t*)concat, strlen(concat), digest);
    char expected[32];
    cryptoB64Encode(digest, sizeof(digest), expected);

    const unsigned long deadline = millis() + PNEX_WS_HANDSHAKE_TIMEOUT_MS;
    char line[160];
    if (!read_line(line, sizeof(line), deadline)) {
        Serial.println("[WS] handshake: no answer");
        return false;
    }
    if (strncmp(line, "HTTP/1.1 101", 12) != 0) {
        Serial.printf("[WS] handshake refused: %s\n", line);
        return false;
    }
    bool accepted = false;
    while (true) {
        if (!read_line(line, sizeof(line), deadline)) {
            Serial.println("[WS] handshake: headers cut");
            return false;
        }
        if (line[0] == '\0') {
            break;
        }
        if (header_is(line, "Sec-WebSocket-Accept:")) {
            const char* v = line + strlen("Sec-WebSocket-Accept:");
            while (*v == ' ' || *v == '\t') {
                ++v;
            }
            accepted = strcmp(v, expected) == 0;
        }
    }
    if (!accepted) {
        Serial.println("[WS] handshake: bad Sec-WebSocket-Accept");
    }
    return accepted;
}

// ───────────────────────── Writing ─────────────────────────

bool PnexWsClient::write_all(const uint8_t* data, size_t n) {
    unsigned long last_progress = millis();
    while (n > 0) {
        const size_t w = tcp_->write(data, n);
        if (w > 0) {
            data += w;
            n -= w;
            last_progress = millis();
            continue;
        }
        if (!tcp_->connected() || millis() - last_progress > PNEX_WS_IO_TIMEOUT_MS) {
            return false;
        }
        delay(1);
    }
    return true;
}

// One complete (FIN) masked frame, written in PNEX_WS_TX_CHUNK pieces.
bool PnexWsClient::write_frame(uint8_t opcode, const uint8_t* data, size_t len) {
    if (!tcp_) {
        return false;
    }
    uint8_t* buf = (uint8_t*)malloc(PNEX_WS_TX_CHUNK);
    if (buf == nullptr) {
        Serial.println("[WS] TX buffer alloc failed");
        return false;
    }
    size_t n = 0;
    buf[n++] = 0x80 | opcode;
    if (len < 126) {
        buf[n++] = 0x80 | (uint8_t)len;
    } else if (len <= 0xFFFF) {
        buf[n++] = 0x80 | 126;
        buf[n++] = (uint8_t)(len >> 8);
        buf[n++] = (uint8_t)len;
    } else {
        buf[n++] = 0x80 | 127;
        for (int i = 7; i >= 0; --i) {
            buf[n++] = (uint8_t)((uint64_t)len >> (8 * i));
        }
    }
    const uint32_t r = random32();
    uint8_t mask[4];
    memcpy(mask, &r, 4);
    memcpy(buf + n, mask, 4);
    n += 4;

    bool ok = true;
    size_t off = 0;
    do {
        while (off < len && n < PNEX_WS_TX_CHUNK) {
            buf[n++] = data[off] ^ mask[off & 3];
            ++off;
        }
        if (!write_all(buf, n)) {
            ok = false;
            break;
        }
        n = 0;
    } while (off < len);
    free(buf);
    if (!ok) {
        Serial.println("[WS] write failed — closing");
        teardown();
    }
    return ok;
}

bool PnexWsClient::send(const char* text, size_t len) {
    return available() && write_frame(OP_TEXT, (const uint8_t*)text, len);
}

bool PnexWsClient::sendBinary(const uint8_t* data, size_t len) {
    return available() && write_frame(OP_BINARY, data, len);
}

bool PnexWsClient::ping() {
    return available() && write_frame(OP_PING, nullptr, 0);
}

// ───────────────────────── Reading ─────────────────────────

bool PnexWsClient::read_exact(uint8_t* buf, size_t n) {
    unsigned long last_progress = millis();
    while (n > 0) {
        const int r = tcp_->read(buf, n);
        if (r > 0) {
            buf += r;
            n -= (size_t)r;
            last_progress = millis();
            continue;
        }
        if (!tcp_->connected() || millis() - last_progress > PNEX_WS_IO_TIMEOUT_MS) {
            return false;
        }
        delay(1);
    }
    return true;
}

// One frame; false = the link must be torn down (I/O or protocol error).
bool PnexWsClient::read_frame() {
    uint8_t head[2];
    if (!read_exact(head, 2)) {
        return false;
    }
    const bool fin = head[0] & 0x80;
    const uint8_t opcode = head[0] & 0x0F;
    const bool masked = head[1] & 0x80;
    uint64_t len = head[1] & 0x7F;
    if (len == 126) {
        uint8_t ext[2];
        if (!read_exact(ext, 2)) {
            return false;
        }
        len = (uint64_t)ext[0] << 8 | ext[1];
    } else if (len == 127) {
        uint8_t ext[8];
        if (!read_exact(ext, 8)) {
            return false;
        }
        len = 0;
        for (int i = 0; i < 8; ++i) {
            len = len << 8 | ext[i];
        }
    }
    uint8_t mask[4] = {0, 0, 0, 0};
    if (masked && !read_exact(mask, 4)) {
        return false;
    }

    if (opcode >= OP_CLOSE) {
        // Control frame: ≤ 125 bytes, never fragmented.
        if (len > 125 || !fin) {
            return false;
        }
        uint8_t payload[125];
        if (!read_exact(payload, (size_t)len)) {
            return false;
        }
        for (size_t i = 0; i < len; ++i) {
            payload[i] ^= mask[i & 3];
        }
        if (opcode == OP_CLOSE) {
            // Echo the status code, then drop the link.
            write_frame(OP_CLOSE, payload, len >= 2 ? 2 : 0);
            return false;
        }
        if (opcode == OP_PING) {
            if (!write_frame(OP_PONG, payload, (size_t)len)) {
                return false;
            }
            emit(PnexWsEvent::GotPing);
        } else if (opcode == OP_PONG) {
            emit(PnexWsEvent::GotPong);
        }
        return true;
    }

    // Data frame: start (text/binary) or continuation of a message.
    if (opcode == OP_TEXT || opcode == OP_BINARY) {
        if (rx_active_) {
            return false;  // new message inside a fragmented one
        }
        rx_active_ = true;
        rx_binary_ = opcode == OP_BINARY;
        rx_len_ = 0;
    } else if (opcode != OP_CONT || !rx_active_) {
        return false;
    }
    if (rx_len_ + len > PNEX_WS_MAX_MESSAGE) {
        Serial.printf("[WS] message over %u bytes — closing\n", (unsigned)PNEX_WS_MAX_MESSAGE);
        const uint8_t too_big[2] = {0x03, 0xF1};  // 1009
        write_frame(OP_CLOSE, too_big, sizeof(too_big));
        return false;
    }
    uint8_t* grown = (uint8_t*)realloc(rx_, rx_len_ + (size_t)len + 1);
    if (grown == nullptr) {
        Serial.println("[WS] RX alloc failed — closing");
        return false;
    }
    rx_ = grown;
    if (!read_exact(rx_ + rx_len_, (size_t)len)) {
        return false;
    }
    for (size_t i = 0; i < len; ++i) {
        rx_[rx_len_ + i] ^= mask[i & 3];
    }
    rx_len_ += (size_t)len;
    if (fin) {
        rx_[rx_len_] = '\0';
        // Detach before the callback: it may send, close or reconnect.
        uint8_t* msg = rx_;
        const size_t msg_len = rx_len_;
        const bool binary = rx_binary_;
        rx_ = nullptr;
        rx_len_ = 0;
        rx_active_ = false;
        if (on_message_) {
            on_message_((const char*)msg, msg_len, binary);
        }
        free(msg);
    }
    return true;
}

void PnexWsClient::poll() {
    if (!available()) {
        return;
    }
    for (int i = 0; i < MAX_FRAMES_PER_POLL && open_ && tcp_->available() > 0; ++i) {
        if (!read_frame()) {
            teardown();
            return;
        }
    }
}

// ───────────────────────── URL helper ─────────────────────────

bool pnex_ws_open(PnexWsClient& client, const char* url) {
    // "wss://host[:port]/path" — default port 443. wss only (D154).
    if (strncmp(url, "wss://", 6) != 0) {
        Serial.println("[WS] refused: not a wss:// URL");
        return false;
    }
    const char* rest = url + 6;
    const char* slash = strchr(rest, '/');
    const size_t hostport_len = slash ? (size_t)(slash - rest) : strlen(rest);
    char host[96];
    snprintf(host, sizeof(host), "%.*s", (int)hostport_len, rest);
    int port = 443;
    char* colon = strrchr(host, ':');
    if (colon) {
        *colon = '\0';
        port = atoi(colon + 1);
    }
    const char* path = slash ? slash : "/";

    // Shared trust posture: CA pin + client identity (on the ESP8266 the
    // pin also brings the lean MFLN buffers, as for OTA).
    auto* secured = new WiFiClientSecure();
    pnex_tls_apply(*secured);
    const bool ok = client.connect(secured, host, (uint16_t)port, path);
#if !defined(ESP32)
    if (!ok) {
        pnex_tls_log_error(*secured);
    }
#endif
    return ok;
}
