//
// pnex-tls — single TLS trust configuration point, shared by the WS
// transport and the OTA download client (docs/architecture/ota.md).
//
// Posture: the optional PNEX_CA_CERT (-D, base64 PEM — same injection
// school as the other config vars) pins the server chain; empty (default)
// keeps the current setInsecure() posture unchanged. When the real PNeX
// wss certificate lands, build with its ISSUING CA — renewals under the
// same CA need no device update, and both WS + OTA switch to real
// verification with no firmware rework.
//
// CA rotation needs one rebuild+OTA (or USB for the first hop) — same
// chicken-and-egg as any pinning; documented in ota.md.
//

#ifndef PNEX_TLS_H
#define PNEX_TLS_H

#include <Arduino.h>

#if defined(ESP32)
#include <WiFiClientSecure.h>
#else
#include <ESP8266WiFi.h>  // brings the WiFiClientSecure (BearSSL) typedef
#endif

// Largest pinnable PEM, terminating NUL included. Mirrors
// pnex_core::firmware::device_ca_max_pem_bytes (server refuses bigger CAs
// at build time with build_ca_too_large): 4 KB on ESP32 (two roots fit),
// 2 KB on the ESP8266 (heap budget, one root).
#ifndef PNEX_CA_PEM_MAX
#if defined(ESP32)
#define PNEX_CA_PEM_MAX 4096
#else
#define PNEX_CA_PEM_MAX 2048
#endif
#endif

// Decode the CA once (call from pnex_transport_setup, before any
// pnex_tls_apply). `ca_pem_b64` = PNEX_CA_CERT macro value ("" = none).
// A CA larger than PNEX_CA_PEM_MAX - 1 bytes is never written nor pinned.
void pnex_tls_init(const char* ca_pem_b64);

// True when a CA was provided (real verification posture).
bool pnex_tls_pinned();

// Device TLS client identity (D153): certificate + PKCS#8 private key
// issued by the org CA, base64 PEM (PNEX_CLIENT_CERT / PNEX_CLIENT_KEY,
// "" = none). Decoded once; presented by every TLS client of the device.
void pnex_tls_set_client_identity(const char* cert_pem_b64, const char* key_pem_b64);

// True when a client certificate is compiled in.
bool pnex_tls_has_client_identity();

// Apply the shared trust posture to a client.
// - unpinned  → setInsecure() (both cores).
// - ESP32 pinned → setCACert.
// - ESP8266 pinned → BearSSL X509List trust anchor + MFLN buffers
//   (setBufferSizes 2048/512 — the ~40 KB heap budget), plus a heap log.
// Used by both the WS client (pnex_ws) and the OTA download.
void pnex_tls_apply(WiFiClientSecure& client);

#if !defined(ESP32)
// Logs BearSSL's last error after a failed TLS connect (no-op when none).
void pnex_tls_log_error(WiFiClientSecure& client);
#endif

#endif  // PNEX_TLS_H
