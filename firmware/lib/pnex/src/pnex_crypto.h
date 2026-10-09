//
// Device-side encryption of the WebSocket links: the Noise link (D156,
// pnex_noise.h) keyed by the 32-byte pre-shared key compiled as
// ENCRYPTION_KEY, plus the base64 helpers of the firmware.
//
// No key, no link: a build without a valid ENCRYPTION_KEY never connects
// (the former "empty key = clear frames" mode is gone, SEC-19 / D157).
//
#ifndef PNEX_CRYPTO_H
#define PNEX_CRYPTO_H

#include <Arduino.h>

#include "pnex_noise.h"

/// Decodes the base64 pre-shared key (32 bytes); false when absent or
/// invalid (the device then refuses to connect).
bool cryptoSetKey(const char* b64Key);

/// True when a valid key is loaded.
bool cryptoReady();

/// Starts the Noise handshake of `link` with a fresh ephemeral key:
/// base64 of the first message for a text link ("" without a key).
String cryptoLinkStartText(pnex_noise::Link& link, const char* device_id);

/// Same, raw 48 bytes into `msg1` for a binary link (camera).
bool cryptoLinkStartBinary(pnex_noise::Link& link, const char* device_id,
                           uint8_t msg1[pnex_noise::MSG1_LEN]);

/// Reads the server's second message (base64 text); true = link ready.
bool cryptoLinkFinishText(pnex_noise::Link& link, const char* wire);

/// Same for a binary link.
bool cryptoLinkFinishBinary(pnex_noise::Link& link, const uint8_t* data, size_t len);

/// Plain text → base64 sealed frame; "" when the link is not ready or the
/// payload exceeds the per-chip budget (never sent in clear).
String cryptoSealText(pnex_noise::Link& link, const char* plain);

/// Base64 sealed frame → plain text; "" when forged, replayed, oversized or
/// the link is not ready.
String cryptoOpenText(pnex_noise::Link& link, const char* wire);

/// Seals `n` bytes in place in `buf`, which must hold
/// pnex_noise::Link::sealed_len(n) bytes; returns the sealed size (0 when
/// the link is not ready).
size_t cryptoSealBinaryInPlace(pnex_noise::Link& link, uint8_t* buf, size_t n);

/// Base64 decode through the wrapped lib (densaugeo). Its header is
/// header-only and NOT inline: included by two translation units, its
/// functions are defined twice and the link fails — it is only included in
/// pnex_crypto.cpp and the whole firmware goes through this wrapper.
/// Does NOT null-terminate the output; returns the decoded length.
unsigned int cryptoB64Decode(const char* b64, unsigned char* out);

/// Returned by cryptoB64DecodeBounded when the decoded value does not fit.
#define PNEX_B64_TOO_LONG 0xFFFFFFFFu

/// Bounded base64 decode: computes the decoded length first and writes
/// nothing when it does not fit `cap` bytes WITH its terminating NUL
/// (returns PNEX_B64_TOO_LONG, `out` set to ""). Otherwise decodes,
/// null-terminates and returns the decoded length.
unsigned int cryptoB64DecodeBounded(const char* b64, char* out, unsigned int cap);

/// Base64 encode through the same wrapped lib (single-TU rule above).
/// `out` must hold 4 * ceil(n / 3) + 1 bytes; it is null-terminated.
/// Returns the encoded length.
unsigned int cryptoB64Encode(const unsigned char* in, unsigned int n, char* out);

#endif  // PNEX_CRYPTO_H
