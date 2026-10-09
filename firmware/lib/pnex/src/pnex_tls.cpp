//
// pnex-tls — implementation. pnex_config.h stays private to
// pnex_transport.cpp (single-TU linkage rule): the PNEX_CA_CERT macro is
// re-fallbacked here per TU (macro, no linkage impact).
//
#include "pnex_tls.h"

#if defined(ESP32)
#include <WiFiClientSecure.h>
#else
#include <ESP8266WiFi.h>  // BearSSL
#endif

#include "pnex_crypto.h"  // cryptoB64Decode

#include <time.h>

// CA, base64 PEM (empty = no trust anchor: every handshake refused).
#ifndef PNEX_CA_CERT
#define PNEX_CA_CERT ""
#endif

// Decoded PEM + parse-once state (PNEX_CA_PEM_MAX bytes including the NUL).
static char s_ca_pem[PNEX_CA_PEM_MAX];
static unsigned int s_ca_len = 0;
static bool s_init_done = false;
// A CA was compiled in but could not be loaded: fail closed (never fall
// back to setInsecure, the build asked for verification).
static bool s_ca_rejected = false;

void pnex_tls_init(const char* ca_pem_b64) {
    s_init_done = true;
    unsigned int n = cryptoB64DecodeBounded(ca_pem_b64, s_ca_pem, sizeof(s_ca_pem));
    if (n == PNEX_B64_TOO_LONG) {
        // The server refuses such a build (build_ca_too_large): reaching
        // this means a hand-made build. Never pin a partial chain.
        Serial.printf("[TLS] CA larger than %u bytes — refused, TLS disabled\n",
                      (unsigned)(sizeof(s_ca_pem) - 1));
        s_ca_len = 0;
        s_ca_rejected = true;
        return;
    }
    s_ca_len = n;
    if (s_ca_len == 0) return;
    Serial.printf("[TLS] CA pinned (%u bytes PEM)\n", s_ca_len);
#if !defined(ESP32)
    // Certificate dates need a clock (x509_now); syncs once WiFi is up.
    configTime(0, 0, "pool.ntp.org", "time.google.com");
#endif
}

bool pnex_tls_pinned() {
    return s_ca_len > 0;
}

// Client identity (D153): ECDSA P-256 certificate + key, a few hundred
// bytes each in PEM.
static char s_client_cert_pem[1536];
static char s_client_key_pem[512];
static bool s_has_client = false;

void pnex_tls_set_client_identity(const char* cert_pem_b64, const char* key_pem_b64) {
    const unsigned int c = cryptoB64DecodeBounded(cert_pem_b64, s_client_cert_pem, sizeof(s_client_cert_pem));
    const unsigned int k = cryptoB64DecodeBounded(key_pem_b64, s_client_key_pem, sizeof(s_client_key_pem));
    s_has_client = c != PNEX_B64_TOO_LONG && k != PNEX_B64_TOO_LONG && c > 0 && k > 0;
    if (!s_has_client) {
        // Never keep half an identity (a key would linger in RAM).
        memset(s_client_key_pem, 0, sizeof(s_client_key_pem));
        if (cert_pem_b64[0] != '\0' || key_pem_b64[0] != '\0') {
            Serial.println("[TLS] client certificate unusable — none presented");
        }
        return;
    }
    Serial.printf("[TLS] client certificate ready (%u bytes PEM)\n", c);
}

bool pnex_tls_has_client_identity() {
    return s_has_client;
}

// Presents the device certificate on `client` when one is compiled in.
static void apply_client_identity(WiFiClientSecure& client) {
    if (!s_has_client) return;
#if defined(ESP32)
    client.setCertificate(s_client_cert_pem);
    client.setPrivateKey(s_client_key_pem);
#else
    // BearSSL objects parsed once (heap logged like the trust anchor).
    static BearSSL::X509List* s_chain = nullptr;
    static BearSSL::PrivateKey* s_key = nullptr;
    if (s_chain == nullptr) {
        s_chain = new BearSSL::X509List(s_client_cert_pem);
        s_key = new BearSSL::PrivateKey(s_client_key_pem);
        Serial.printf("[TLS] 8266 client certificate parsed, heap=%u\n",
                      (unsigned)ESP.getFreeHeap());
    }
    client.setClientECCert(s_chain, s_key, BR_KEYTYPE_KEYX | BR_KEYTYPE_SIGN, BR_KEYTYPE_EC);
#endif
}

#if !defined(ESP32)
// BearSSL checks the certificate dates and the ESP8266 has no clock: with
// no time set, every pinned handshake fails (BR_ERR_X509_TIME_UNKNOWN —
// mbedTLS on ESP32 skips the dates). Time = SNTP once synced (started in
// pnex_tls_init, needs Internet), else the firmware build date: enough
// until the edge leaf is renewed after that date (397 days, renewed 30
// days before expiry) on a network without NTP.
static time_t build_epoch() {
    static const char months[] = "JanFebMarAprMayJunJulAugSepOctNovDec";
    const char* date = __DATE__;  // "Oct  3 2026"
    const int month = (int)(strstr(months, String(date).substring(0, 3).c_str()) - months) / 3 + 1;
    const int day = atoi(date + 4);
    int year = atoi(date + 7);
    const char* t = __TIME__;  // "21:34:45"
    // Days from 1970-01-01 (civil calendar, March-based year).
    year -= month <= 2;
    const int era = year / 400;
    const int yoe = year - era * 400;
    const int mp = (month + 9) % 12;
    const int doy = (153 * mp + 2) / 5 + day - 1;
    const int doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    const long days = (long)era * 146097 + doe - 719468;
    return (time_t)days * 86400 + atoi(t) * 3600 + atoi(t + 3) * 60 + atoi(t + 6);
}

static time_t x509_now() {
    const time_t now = time(nullptr);
    const time_t built = build_epoch();
    return now > built ? now : built;
}
#endif

// ───────────────────────── apply ─────────────────────────

void pnex_tls_apply(WiFiClientSecure& client) {
    if (s_ca_rejected) {
        // No trust anchor and no setInsecure: every handshake fails.
        Serial.println("[TLS] no usable CA — connection refused (rebuild the firmware)");
        return;
    }
    if (!pnex_tls_pinned() || s_ca_pem[0] == '\0') {
        // No setInsecure (D154): without a CA the handshake cannot verify
        // the server, so it is refused.
        Serial.println("[TLS] no CA compiled in — connection refused (rebuild the firmware)");
        return;
    }
    apply_client_identity(client);
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
    client.setX509Time(x509_now());
    client.setBufferSizes(2048, 512);  // MFLN (requires server support)
#endif
}

#if !defined(ESP32)
void pnex_tls_log_error(WiFiClientSecure& client) {
    char text[80];
    const int code = client.getLastSSLError(text, sizeof(text));
    if (code != 0) {
        Serial.printf("[TLS] handshake error %d: %s\n", code, text);
    }
}
#endif
