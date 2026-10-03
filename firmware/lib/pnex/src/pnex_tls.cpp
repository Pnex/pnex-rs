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

#include "chacha_crypto.h"  // cryptoB64Decode

#include <time.h>

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
#if !defined(ESP32)
    // Certificate dates need a clock (x509_now); syncs once WiFi is up.
    configTime(0, 0, "pool.ntp.org", "time.google.com");
#endif
}

bool pnex_tls_pinned() {
    return s_ca_len > 0;
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
