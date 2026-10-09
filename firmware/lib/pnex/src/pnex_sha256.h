//
// SHA-256 (FIPS 180-4), HMAC-SHA256 (RFC 2104) and the Noise HKDF
// (Noise spec §4.3) — the hash half of Noise_NNpsk0_25519_ChaChaPoly_SHA256
// (D156; X25519 and ChaCha20-Poly1305 come from the vendored Monocypher).
//
// Header-only and free of Arduino: included by pnex_noise.cpp on every chip
// and by the host test (firmware/core-cpp-tests), which replays the FIPS and
// RFC 4231 vectors — the code that runs on the boards is the code under test.
//
#ifndef PNEX_SHA256_H
#define PNEX_SHA256_H

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace pnex_crypto {

constexpr size_t SHA256_LEN = 32;
constexpr size_t SHA256_BLOCK = 64;

struct Sha256 {
    uint32_t h[8];
    uint8_t buf[SHA256_BLOCK];
    size_t buf_len;
    uint64_t total;

    static uint32_t rotr(uint32_t v, int c) { return (v >> c) | (v << (32 - c)); }

    void init() {
        static const uint32_t iv[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                       0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
        memcpy(h, iv, sizeof(h));
        buf_len = 0;
        total = 0;
    }

    void block(const uint8_t* p) {
        static const uint32_t k[64] = {
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
            0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
            0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
            0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
            0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
            0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
            0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
            0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
            0xc67178f2};
        uint32_t w[64];
        for (int i = 0; i < 16; ++i) {
            w[i] = ((uint32_t)p[4 * i] << 24) | ((uint32_t)p[4 * i + 1] << 16) |
                   ((uint32_t)p[4 * i + 2] << 8) | (uint32_t)p[4 * i + 3];
        }
        for (int i = 16; i < 64; ++i) {
            uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
        for (int i = 0; i < 64; ++i) {
            uint32_t s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            uint32_t ch = (e & f) ^ (~e & g);
            uint32_t t1 = hh + s1 + ch + k[i] + w[i];
            uint32_t s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
            uint32_t t2 = s0 + maj;
            hh = g;
            g = f;
            f = e;
            e = d + t1;
            d = c;
            c = b;
            b = a;
            a = t1 + t2;
        }
        h[0] += a;
        h[1] += b;
        h[2] += c;
        h[3] += d;
        h[4] += e;
        h[5] += f;
        h[6] += g;
        h[7] += hh;
    }

    void update(const uint8_t* data, size_t len) {
        total += len;
        while (len > 0) {
            size_t take = SHA256_BLOCK - buf_len;
            if (take > len) take = len;
            memcpy(buf + buf_len, data, take);
            buf_len += take;
            data += take;
            len -= take;
            if (buf_len == SHA256_BLOCK) {
                block(buf);
                buf_len = 0;
            }
        }
    }

    void finish(uint8_t out[SHA256_LEN]) {
        uint64_t bits = total * 8;
        uint8_t pad = 0x80;
        update(&pad, 1);
        uint8_t zero = 0;
        while (buf_len != 56) update(&zero, 1);
        uint8_t len_be[8];
        for (int i = 0; i < 8; ++i) len_be[i] = (uint8_t)(bits >> (56 - 8 * i));
        update(len_be, 8);
        for (int i = 0; i < 8; ++i) {
            out[4 * i] = (uint8_t)(h[i] >> 24);
            out[4 * i + 1] = (uint8_t)(h[i] >> 16);
            out[4 * i + 2] = (uint8_t)(h[i] >> 8);
            out[4 * i + 3] = (uint8_t)h[i];
        }
        memset(this, 0, sizeof(*this));
    }
};

inline void sha256(const uint8_t* data, size_t len, uint8_t out[SHA256_LEN]) {
    Sha256 s;
    s.init();
    s.update(data, len);
    s.finish(out);
}

/// HMAC-SHA256 over the concatenation of up to two message parts.
inline void hmac_sha256(const uint8_t* key, size_t key_len, const uint8_t* m1, size_t m1_len,
                        const uint8_t* m2, size_t m2_len, uint8_t out[SHA256_LEN]) {
    uint8_t k0[SHA256_BLOCK] = {0};
    if (key_len > SHA256_BLOCK) {
        sha256(key, key_len, k0);
    } else {
        memcpy(k0, key, key_len);
    }
    uint8_t pad[SHA256_BLOCK];
    uint8_t inner[SHA256_LEN];
    Sha256 s;
    for (size_t i = 0; i < SHA256_BLOCK; ++i) pad[i] = k0[i] ^ 0x36;
    s.init();
    s.update(pad, SHA256_BLOCK);
    if (m1_len) s.update(m1, m1_len);
    if (m2_len) s.update(m2, m2_len);
    s.finish(inner);
    for (size_t i = 0; i < SHA256_BLOCK; ++i) pad[i] = k0[i] ^ 0x5c;
    s.init();
    s.update(pad, SHA256_BLOCK);
    s.update(inner, SHA256_LEN);
    s.finish(out);
    memset(k0, 0, sizeof(k0));
    memset(pad, 0, sizeof(pad));
    memset(inner, 0, sizeof(inner));
}

/// Noise HKDF (spec §4.3): two or three 32-byte outputs (`out3` may be null).
inline void noise_hkdf(const uint8_t ck[SHA256_LEN], const uint8_t* ikm, size_t ikm_len,
                       uint8_t out1[SHA256_LEN], uint8_t out2[SHA256_LEN],
                       uint8_t* out3) {
    uint8_t temp[SHA256_LEN];
    hmac_sha256(ck, SHA256_LEN, ikm, ikm_len, nullptr, 0, temp);
    const uint8_t one = 0x01, two = 0x02, three = 0x03;
    hmac_sha256(temp, SHA256_LEN, &one, 1, nullptr, 0, out1);
    hmac_sha256(temp, SHA256_LEN, out1, SHA256_LEN, &two, 1, out2);
    if (out3) {
        hmac_sha256(temp, SHA256_LEN, out2, SHA256_LEN, &three, 1, out3);
    }
    memset(temp, 0, sizeof(temp));
}

}  // namespace pnex_crypto

#endif  // PNEX_SHA256_H
