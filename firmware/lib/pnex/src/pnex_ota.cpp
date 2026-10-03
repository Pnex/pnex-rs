//
// pnex-ota — implementation. See pnex_ota.h for the contract.
//
#include "pnex_ota.h"

#if defined(ESP32)
#include <Update.h>  // built-in Update library (see env lib_deps)
#include <WiFi.h>  // WiFiClient (core 3.x headers no longer pull it in)
#include <WiFiClientSecure.h>
#include <HTTPClient.h>
#include <esp_ota_ops.h>
#else
#include <Updater.h>  // core header (ESP8266)
#include <ESP8266HTTPClient.h>
#include <WiFiClientSecure.h>
#endif

#include "pnex_transport.h"
#include "pnex_tls.h"
#include "pnex_status.h"

// ───────────────────────── SHA-256 adapter ─────────────────────────

#if defined(ESP32)
#include <mbedtls/sha256.h>

struct PnexSha256 {
    mbedtls_sha256_context ctx;
    void begin() {
        mbedtls_sha256_init(&ctx);
        mbedtls_sha256_starts(&ctx, 0);
    }
    void update(const uint8_t* data, size_t len) {
        mbedtls_sha256_update(&ctx, data, len);
    }
    void end(uint8_t* out /*32 bytes*/) {
        mbedtls_sha256_finish(&ctx, out);
        mbedtls_sha256_free(&ctx);
    }
};
#else


struct PnexSha256 {
    BearSSL::HashSHA256 ctx;
    void begin() {
        ctx.begin();
    }
    void update(const uint8_t* data, size_t len) {
        ctx.add(data, len);
    }
    void end(uint8_t* out) {
        ctx.end();
        memcpy(out, ctx.hash(), 32);
    }
};
#endif

static void hex32(const uint8_t* digest, char* out /*65 bytes*/) {
    for (int i = 0; i < 32; ++i) {
        sprintf(out + 2 * i, "%02x", digest[i]);
    }
    out[64] = '\0';
}

// Case-insensitive compare against the pushed digest.
static bool sha_matches(const char* expected, const uint8_t* digest) {
    char got[65];
    hex32(digest, got);
    return strcasecmp(got, expected) == 0;
}

// ───────────────────────── Updater API shims ─────────────────────────

static void ota_abort() {
#if defined(ESP32)
    Update.abort();
#else
    Update.end();  // 8266: end() without success releases the staging area
#endif
}

static void ota_error_msg(char* err, size_t errsz, const char* what) {
#if defined(ESP32)
    snprintf(err, errsz, "%s: %s", what, Update.errorString());
#else
    const String e = Update.getErrorString();
    snprintf(err, errsz, "%s: %s", what, e.c_str());
#endif
}

// ───────────────────────── Run ─────────────────────────

bool pnex_ota_run(const char* url_path,
                  const char* sha_hex,
                  const PnexOtaHooks& hooks,
                  char* err,
                  size_t errsz) {
    snprintf(err, errsz, "unknown");
    // URL = scheme://host + path + the same b64 query auth as /ws/device.
    char url[256];
    snprintf(url, sizeof(url), "%s://%s%s?token=%s&device_id=%s",
             pnex_use_tls() ? "https" : "http",
             pnex_host(), url_path, pnex_token_b64(), pnex_device_id_b64());
    Serial.printf("[OTA] %s\n", url);

    // Both clients live for the whole download: HTTPClient keeps a
    // reference (the plain one used to die at the end of its else block).
    WiFiClientSecure tls;
    WiFiClient plain;

    HTTPClient http;
    if (pnex_use_tls()) {
        pnex_tls_apply(tls);
        http.begin(tls, url);
    } else {
        // Plain HTTP (LAN/on-prem reference path).
        http.begin(plain, url);
    }
    const int code = http.GET();
    if (code != HTTP_CODE_OK) {
        snprintf(err, errsz, "HTTP %d", code);
        http.end();
        return false;
    }
    const int total = http.getSize();  // -1 when chunked
    Serial.printf("[OTA] %d octets annoncés\n", total);

    if (!Update.begin(total > 0 ? total : 0)) {
        ota_error_msg(err, errsz, "begin");
        http.end();
        return false;
    }

    PnexSha256 sha;
    sha.begin();

    uint8_t buf[1024];
    size_t got = 0;
    unsigned long last_progress = 0;
    uint8_t last_pct = 0;      // screen publish watermark (every step)
    uint8_t last_sent_pct = 0; // WS frame watermark (5 s anti-spam throttle)
    WiFiClient* stream = http.getStreamPtr();

    pnex_status::set_ota("downloading", 0);
    if (hooks.progress) hooks.progress("downloading", 0, nullptr);

    while (http.connected() && (total < 0 || got < (size_t)total)) {
        const size_t avail = stream->available();
        if (avail > 0) {
            const size_t n = stream->readBytes(buf, min(avail, sizeof(buf)));
            Update.write(buf, n);
            sha.update(buf, n);
            got += n;
            if (total > 0) {
                const uint8_t pct = (uint8_t)((got * 100ULL) / total);
                if (pct != last_pct) {
                    last_pct = pct;
                    // Screen publish on EVERY step — the WS frame below stays
                    // throttled (server-side log/UI cadence).
                    pnex_status::set_ota("downloading", pct);
                }
                const unsigned long now = millis();
                if (pct != last_sent_pct && (now - last_progress) >= 5000) {
                    last_progress = now;
                    last_sent_pct = pct;
                    if (hooks.progress) hooks.progress("downloading", pct, nullptr);
                }
            }
        } else {
            delay(5);
        }
        if (hooks.tick) {
            hooks.tick();  // screen repaint (change-only) — also on fast LAN streams
        }
#if defined(ESP8266)
        ESP.wdtFeed();
#else
        yield();
#endif
    }

    if (total > 0 && got != (size_t)total) {
        snprintf(err, errsz, "truncated: %u/%d", (unsigned)got, total);
        ota_abort();
        http.end();
        return false;
    }
    http.end();

    // Integrity gate BEFORE Update.end flips the boot slot.
    uint8_t digest[32];
    sha.end(digest);
    if (!sha_matches(sha_hex, digest)) {
        snprintf(err, errsz, "sha mismatch");
        pnex_status::set_ota("failed", 0);
        if (hooks.progress) hooks.progress("failed", 0, "sha mismatch");
        ota_abort();
        return false;
    }

    if (!Update.end(true)) {
        ota_error_msg(err, errsz, "end");
        pnex_status::set_ota("failed", 0);
        if (hooks.progress) hooks.progress("failed", 0, err);
        return false;
    }

    Serial.printf("[OTA] ok, %u octets — reboot\n", (unsigned)got);
    pnex_status::set_ota("flashing", 100);
    if (hooks.progress) hooks.progress("flashing", 100, nullptr);
    return true;
}
