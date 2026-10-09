#include "pnex_crypto.h"

#include <base64.hpp>
#if !defined(ESP8266)
#include <esp_system.h>
#endif

namespace {

constexpr size_t KEY_LEN = pnex_noise::KEY_LEN;

// Longest text frames: the ProvisionAck of the generic device. Worst case
// = MAX_PINS (32) specs with long custom labels ≈ 3.5 KB in clear → 4096
// on the ESP32 family (large RAM). ESP8266: counted heap (~40 KB) and
// overlays ≤ 11 pins (~900 B) → 1024 kept, a larger ack is dropped cleanly
// (never an overflow: size checks before any decode).
constexpr size_t MAX_PLAIN =
#if defined(ESP8266)
    1024;
#else
    4096;
#endif
constexpr size_t MAX_SEALED = MAX_PLAIN + pnex_noise::TAG_LEN;
constexpr size_t MAX_WIRE = 4 * ((MAX_SEALED + 2) / 3);  // padded base64

uint8_t s_key[KEY_LEN];
bool s_ready = false;

// Distinct static tx/rx buffers — counted ESP8266 stack (~4 KB); sends run
// from the loop, receptions from the poll of the same loop.
uint8_t s_tx[MAX_SEALED];
uint8_t s_txWire[MAX_WIRE + 1];
uint8_t s_rx[MAX_WIRE + 1];

/// Hardware RNG (valid while the radio is on — links are opened with WiFi up).
void fillRandom(uint8_t* out, size_t len) {
    for (size_t i = 0; i < len; i += 4) {
#if defined(ESP8266)
        uint32_t r = ESP.random();
#else
        uint32_t r = esp_random();
#endif
        size_t take = len - i < 4 ? len - i : 4;
        memcpy(out + i, &r, take);
    }
}

bool startLink(pnex_noise::Link& link, const char* device_id, uint8_t msg1[pnex_noise::MSG1_LEN]) {
    if (!s_ready) {
        return false;
    }
    uint8_t e[KEY_LEN];
    fillRandom(e, sizeof(e));
    link.start(s_key, device_id, e, msg1);
    memset(e, 0, sizeof(e));
    return true;
}

}  // namespace

bool cryptoSetKey(const char* b64Key) {
    s_ready = false;
    if (!b64Key || !*b64Key) {
        return false;
    }
    // Size check before decoding into the fixed key buffer.
    if (decode_base64_length((const unsigned char*)b64Key) != KEY_LEN) {
        return false;
    }
    decode_base64((const unsigned char*)b64Key, s_key);
    s_ready = true;
    return true;
}

bool cryptoReady() {
    return s_ready;
}

String cryptoLinkStartText(pnex_noise::Link& link, const char* device_id) {
    uint8_t msg1[pnex_noise::MSG1_LEN];
    if (!startLink(link, device_id, msg1)) {
        return String();
    }
    char out[4 * ((pnex_noise::MSG1_LEN + 2) / 3) + 1];
    encode_base64(msg1, sizeof(msg1), (unsigned char*)out);
    return String(out);
}

bool cryptoLinkStartBinary(pnex_noise::Link& link, const char* device_id,
                           uint8_t msg1[pnex_noise::MSG1_LEN]) {
    return startLink(link, device_id, msg1);
}

bool cryptoLinkFinishText(pnex_noise::Link& link, const char* wire) {
    if (!wire || strlen(wire) > 4 * ((pnex_noise::MSG2_LEN + 2) / 3)) {
        return false;
    }
    uint8_t msg2[pnex_noise::MSG2_LEN + 2];
    unsigned int len = decode_base64_length((const unsigned char*)wire);
    if (len != pnex_noise::MSG2_LEN) {
        return false;
    }
    decode_base64((const unsigned char*)wire, msg2);
    return link.finish(msg2, len);
}

bool cryptoLinkFinishBinary(pnex_noise::Link& link, const uint8_t* data, size_t len) {
    return link.finish(data, len);
}

String cryptoSealText(pnex_noise::Link& link, const char* plain) {
    size_t len = strlen(plain);
    if (!link.ready() || len == 0 || len > MAX_PLAIN) {
        // Nothing sent: never a clear frame on the wire.
        return String();
    }
    const size_t n = link.seal((const uint8_t*)plain, len, s_tx);
    encode_base64(s_tx, n, s_txWire);  // null-terminated by the lib
    return String((char*)s_txWire);
}

String cryptoOpenText(pnex_noise::Link& link, const char* wire) {
    if (!wire || !link.ready()) {
        return String();
    }
    // Bound BEFORE any decode: a base64 longer than the wire capacity is
    // dropped here, never decoded past s_rx.
    if (strlen(wire) > MAX_WIRE) {
        return String();
    }
    const unsigned int len = decode_base64((const unsigned char*)wire, s_rx);
    const size_t n = link.open(s_rx, len, s_rx);
    if (n == SIZE_MAX) {
        return String();
    }
    s_rx[n] = '\0';
    return String((char*)s_rx);
}

size_t cryptoSealBinaryInPlace(pnex_noise::Link& link, uint8_t* buf, size_t n) {
    if (!link.ready() || buf == nullptr) {
        return 0;
    }
    return link.seal(buf, n, buf);
}

unsigned int cryptoB64Decode(const char* b64, unsigned char* out) {
    return decode_base64((const unsigned char*)b64, out);
}

unsigned int cryptoB64DecodeBounded(const char* b64, char* out, unsigned int cap) {
    if (cap == 0) return PNEX_B64_TOO_LONG;
    out[0] = '\0';
    if (!b64 || !*b64) return 0;
    // Size check BEFORE writing: the decoder itself never bounds its output.
    unsigned int need = decode_base64_length((const unsigned char*)b64);
    if (need >= cap) return PNEX_B64_TOO_LONG;
    unsigned int n = decode_base64((const unsigned char*)b64, (unsigned char*)out);
    out[n] = '\0';
    return n;
}

unsigned int cryptoB64Encode(const unsigned char* in, unsigned int n, char* out) {
    return encode_base64(in, n, (unsigned char*)out);
}
