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

namespace websockets {
class WebsocketsClient;
}

// Decode the CA once (call from pnex_transport_setup, before any
// pnex_tls_apply). `ca_pem_b64` = PNEX_CA_CERT macro value ("" = none).
void pnex_tls_init(const char* ca_pem_b64);

// True when a CA was provided (real verification posture).
bool pnex_tls_pinned();

// Apply the shared trust posture to a client.
// - unpinned  → setInsecure() (both cores).
// - ESP32 pinned → setCACert / ws lib CA hook.
// - ESP8266 pinned → BearSSL X509List trust anchor + MFLN buffers
//   (setBufferSizes 2048/512 — the ~40 KB heap budget), plus a heap log.
//   If the WS library cannot take a CA on this core, falls back to
//   setInsecure with a loud Serial warning (documented limitation).
void pnex_tls_apply(websockets::WebsocketsClient& client);
void pnex_tls_apply(WiFiClientSecure& client);

#endif  // PNEX_TLS_H
