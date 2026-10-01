//
// pnex-tls — implementation. pnex_config.h stays private to
// pnex_transport.cpp (single-TU linkage rule): the PNEX_CA_CERT macro is
// re-fallbacked here per TU (macro, no linkage impact).
//
#include "pnex_tls.h"

#include <ArduinoWebsockets.h>

#if defined(ESP32)
#include <WiFiClientSecure.h>
#else
#include <ESP8266WiFi.h>  // BearSSL
#endif

#include "chacha_crypto.h"  // cryptoB64Decode

// Optional CA, base64 PEM (empty default = setInsecure posture).
#ifndef PNEX_CA_CERT
#define PNEX_CA_CERT ""
#endif

// Decoded PEM + parse-once state.
static char s_ca_pem[2048];
static unsigned int s_ca_len = 0;
static bool s_init_done = false;

void pnex_tls_init(const char* ca_pem_b64) {
    s_init_done = true;
    s_ca_len = (ca_pem_b64 && *ca_pem_b64)
                   ? cryptoB64Decode(ca_pem_b64, (unsigned char*)s_ca_pem)
                   : 0;
    s_ca_pem[s_ca_len] = '\0';
    if (s_ca_len == 0) return;
    if (s_ca_len >= sizeof(s_ca_pem) - 1) {
        Serial.printf("[TLS] CA truncated at %u bytes — NOT pinned\n", s_ca_len);
        s_ca_len = 0;
        return;
    }
    Serial.printf("[TLS] CA pinned (%u bytes PEM)\n", s_ca_len);
}

bool pnex_tls_pinned() {
    return s_ca_len > 0;
}

// ───────────────────────── apply — WS client ─────────────────────────

void pnex_tls_apply(websockets::WebsocketsClient& client) {
    if (!pnex_tls_pinned() || s_ca_pem[0] == '\0') {
        client.setInsecure();
        return;
    }
#if defined(ESP32)
    client.setCACert(s_ca_pem);
#else
    // ArduinoWebsockets on ESP8266 cannot take a CA → loud fallback (the
    // deployment TLS terminates in front; LAN installs use plain http).
    Serial.println("[TLS] WS CA pinning unsupported on ESP8266 — insecure WS");
    client.setInsecure();
#endif
}

// ─────────────────────── apply — direct HTTPS client ─────────────────

void pnex_tls_apply(WiFiClientSecure& client) {
    if (!pnex_tls_pinned() || s_ca_pem[0] == '\0') {
        client.setInsecure();
        return;
    }
#if defined(ESP32)
    client.setCACert(s_ca_pem);
#else
    // BearSSL: parse the PEM once (trust anchors cost a few KB of heap —
    // logged) and shrink the TLS buffers via MFLN to fit ~40 KB devices.
    static BearSSL::X509List* s_trust = nullptr;
    if (s_trust == nullptr) {
        s_trust = new BearSSL::X509List(s_ca_pem);
        Serial.printf("[TLS] 8266 trust anchor ready, heap=%u\n",
                      (unsigned)ESP.getFreeHeap());
    }
    client.setTrustAnchors(s_trust);
    client.setBufferSizes(2048, 512);  // MFLN (requires server support)
#endif
}
