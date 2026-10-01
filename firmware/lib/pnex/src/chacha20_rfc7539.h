//
// ChaCha20 (RFC 7539, variante IETF nonce 96 bits) — implémentation
// « constant-time by design » (rotations/additions uniquement, pas de
// branchement sur le secret) vendue pour les cores SANS BearSSL : le
// core ESP8266 l'embarque (br_chacha20_ct_run, chacha_crypto.cpp), le
// core arduino-esp32 non. Miroir exact du serveur (RustCrypto chacha20,
// compteur initial 0, cf. ws_ingest.rs) : XOR du keystream, sans
// Poly1305 (pas d'AEAD, décision D8).
//
// Header-only et SANS dépendance Arduino : inclus par chacha_crypto.cpp
// (firmware ESP32) ET par le test hôte core-cpp-tests (vecteurs RFC
// 7539 §2.3.2/§2.4.2 rejoués en CI — le miroir ne peut pas dévier sans
// casser la CI, même école que les golden vectors pnex-core-cpp).
//
#ifndef CHACHA20_RFC7539_H
#define CHACHA20_RFC7539_H

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace pnex_crypto {

inline uint32_t rotl32(uint32_t v, int c) { return (v << c) | (v >> (32 - c)); }

inline uint32_t load32_le(const uint8_t* p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) |
           ((uint32_t)p[3] << 24);
}

inline void store32_le(uint8_t* p, uint32_t v) {
    p[0] = (uint8_t)(v & 0xff);
    p[1] = (uint8_t)((v >> 8) & 0xff);
    p[2] = (uint8_t)((v >> 16) & 0xff);
    p[3] = (uint8_t)((v >> 24) & 0xff);
}

inline void quarter_round(uint32_t& a, uint32_t& b, uint32_t& c, uint32_t& d) {
    a += b; d ^= a; d = rotl32(d, 16);
    c += d; b ^= c; b = rotl32(b, 12);
    a += b; d ^= a; d = rotl32(d, 8);
    c += d; b ^= c; b = rotl32(b, 7);
}

/// Un bloc de keystream (64 octets) — RFC 7539 §2.3.
inline void chacha20_block(const uint8_t key[32], uint32_t counter,
                           const uint8_t nonce[12], uint8_t out[64]) {
    const uint8_t SIGMA[16] = {'e', 'x', 'p', 'a', 'n', 'd', ' ', '3',
                               '2', '-', 'b', 'y', 't', 'e', ' ', 'k'};
    uint32_t st[16];
    for (int i = 0; i < 4; ++i) st[i] = load32_le(SIGMA + 4 * i);
    for (int i = 0; i < 8; ++i) st[4 + i] = load32_le(key + 4 * i);
    st[12] = counter;
    for (int i = 0; i < 3; ++i) st[13 + i] = load32_le(nonce + 4 * i);

    uint32_t x[16];
    memcpy(x, st, sizeof(x));
    for (int r = 0; r < 10; ++r) {
        quarter_round(x[0], x[4], x[8], x[12]);
        quarter_round(x[1], x[5], x[9], x[13]);
        quarter_round(x[2], x[6], x[10], x[14]);
        quarter_round(x[3], x[7], x[11], x[15]);
        quarter_round(x[0], x[5], x[10], x[15]);
        quarter_round(x[1], x[6], x[11], x[12]);
        quarter_round(x[2], x[7], x[8], x[13]);
        quarter_round(x[3], x[4], x[9], x[14]);
    }
    for (int i = 0; i < 16; ++i) store32_le(out + 4 * i, x[i] + st[i]);
}

/// XOR in-place du keystream (chiffre ET déchiffre) — équivalent exact de
/// br_chacha20_ct_run(key, nonce, counter, data, len) de BearSSL.
inline void chacha20_xor(const uint8_t key[32], uint32_t counter,
                         const uint8_t nonce[12], uint8_t* data, size_t len) {
    uint8_t ks[64];
    size_t off = 0;
    while (off < len) {
        chacha20_block(key, counter++, nonce, ks);
        size_t n = (len - off < 64) ? len - off : 64;
        for (size_t i = 0; i < n; ++i) data[off + i] ^= ks[i];
        off += n;
    }
}

}  // namespace pnex_crypto

#endif  // CHACHA20_RFC7539_H
