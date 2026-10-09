// Signed OTA images (SEC-18): Ed25519 check of the message bound to the
// device, the version and the image digest. Mirror of pnex_core::ota_sig
// (Rust), golden vectors in common_libs/pnex-core-cpp/ota_sig_goldens.h.
// Portable (no Arduino): built by the host tests too.
#pragma once

#include <stddef.h>
#include <stdint.h>

// True when `sig_hex` (128 hex chars) is the signature, by the key
// `pubkey_hex` (64 hex chars), of
// "PNEX-OTA-1" ‖ len(device_id) ‖ device_id ‖ len(version) ‖ version ‖ digest.
// Any malformed input (empty key, bad hex, field longer than 255) → false.
bool pnex_ota_sig_verify(const char* pubkey_hex,
                         const char* device_id,
                         const char* version,
                         const uint8_t digest[32],
                         const char* sig_hex);

// Decodes exactly `len` bytes of hex (any case). False on a bad length or
// character.
bool pnex_hex_decode(const char* hex, uint8_t* out, size_t len);
