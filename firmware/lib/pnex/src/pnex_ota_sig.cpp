#include "pnex_ota_sig.h"

#include <string.h>

#include "monocypher/monocypher-ed25519.h"

static int hex_nibble(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

bool pnex_hex_decode(const char* hex, uint8_t* out, size_t len) {
    if (hex == nullptr || strlen(hex) != len * 2) return false;
    for (size_t i = 0; i < len; i++) {
        const int hi = hex_nibble(hex[2 * i]);
        const int lo = hex_nibble(hex[2 * i + 1]);
        if (hi < 0 || lo < 0) return false;
        out[i] = static_cast<uint8_t>((hi << 4) | lo);
    }
    return true;
}

bool pnex_ota_sig_verify(const char* pubkey_hex,
                         const char* device_id,
                         const char* version,
                         const uint8_t digest[32],
                         const char* sig_hex) {
    static const char DOMAIN[] = "PNEX-OTA-1";
    uint8_t pubkey[32];
    uint8_t sig[64];
    if (!pnex_hex_decode(pubkey_hex, pubkey, sizeof(pubkey))) return false;
    if (!pnex_hex_decode(sig_hex, sig, sizeof(sig))) return false;
    if (device_id == nullptr || version == nullptr) return false;
    const size_t dev_len = strlen(device_id);
    const size_t ver_len = strlen(version);
    if (dev_len > 255 || ver_len > 255) return false;

    // Domain (10) + 2 length bytes + both fields (≤ 255 each) + digest.
    // Static, not on the stack: the ESP8266 loop stack is 4 KB and the
    // Ed25519 check itself needs a good part of it.
    static uint8_t msg[10 + 2 + 255 + 255 + 32];
    size_t n = 0;
    memcpy(msg + n, DOMAIN, sizeof(DOMAIN) - 1);
    n += sizeof(DOMAIN) - 1;
    msg[n++] = static_cast<uint8_t>(dev_len);
    memcpy(msg + n, device_id, dev_len);
    n += dev_len;
    msg[n++] = static_cast<uint8_t>(ver_len);
    memcpy(msg + n, version, ver_len);
    n += ver_len;
    memcpy(msg + n, digest, 32);
    n += 32;
    return crypto_ed25519_check(sig, pubkey, msg, n) == 0;
}
