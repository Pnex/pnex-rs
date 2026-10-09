//
// pnex-transport — implémentation (cf. pnex_transport.h pour le contrat et
// la règle de linkage config.h).
//
#include "pnex_transport.h"

#include "pnex_status.h"  // publication TX (compteur data, no-op sans écran)

// ESP8266 et ESP32 (C3…) — le reste de la lib est de l'Arduino neutre
// (WiFi.begin/status, pnex_ws, ChaCha) : seul l'en-tête WiFi
// diverge, et le watchdog du connect bloquant.
#if defined(ESP32)
#include <WiFi.h>
#else
#include <ESP8266WiFi.h>
#endif

// SEULE unité de traduction incluant config.h à côté des projets qui
// n'utilisent PAS pnex-transport (tft_dev).
#include "pnex_config.h"
#include "pnex_crypto.h"
#include "pnex_tls.h"
#include "pnex_ws.h"

// ───────────────────────── État interne ─────────────────────────

static PnexWsClient s_client;

// Buffers de la config décodée (bornes = validation API : 100 caractères
// SSID/pass, hôte et device_id 64 — parité avec les buffers des mains
// d'origine).
static char s_ssid[101];
static char s_password[101];
static char s_host[65];
static char s_device_id[65];
static char s_conn[256];
// Same URL with the token masked: the only form ever printed (serial logs
// get pasted into forums and tickets — SEC-W7).
static char s_conn_log[256];

static PnexTransportInit s_init;

// Dernier signe de vie serveur (text « PONG » ou pong WS), réarmé au
// connect. Base du PONG timeout (brick0 : 15 s pour le pin_slave).
static unsigned long s_last_pong_ms = 0;

// Noise link of the current connection (D156): pnex_ws_connect() sends the
// first message and waits for the server's answer before returning, so the
// main only sees an established link (announce).
static pnex_noise::Link s_link;
static bool s_handshaking = false;
// Set once the server's answer established the link of this attempt: a
// close right after it is a session refusal (4003 anti-clone while a stale
// session holds the lease), not a handshake failure.
static bool s_handshake_done = false;
static unsigned long s_handshake_started_ms = 0;
// A server that does not answer the handshake in time is not ours.
constexpr unsigned long HANDSHAKE_TIMEOUT_MS = 10000;

// Prototypes
static void on_frame(const char* data, size_t len, bool binary);
static void on_event(PnexWsEvent event);

#if defined(ESP8266)
// ── ESP8266 wss: lean BearSSL buffers ──
// A default WiFiClientSecure takes the BearSSL default buffers (~16 KB
// RX): with the screen + app state the heap then OOMs on the first ~1 KB
// server frame. When the server supports MFLN (nginx/OpenSSL does on
// TLS 1.2), the WS link runs over 2048/512 buffers instead (same budget as
// the OTA HTTPS client).
constexpr int LEAN_TLS_RX = 2048;
constexpr int LEAN_TLS_TX = 512;

// MFLN result, cached for the boot: 0 = unknown, 1 = yes, 2 = no.
static uint8_t s_mfln = 0;

// "host[:port]" → host + port (default 443).
static void split_host_port(const char* hostport, char* host, size_t n,
                            int& port) {
    port = 443;
    snprintf(host, n, "%s", hostport);
    char* colon = strrchr(host, ':');
    if (colon) {
        *colon = '\0';
        port = atoi(colon + 1);
    }
}

// wss connect over lean buffers; false = caller falls back to the default
// buffers (server without MFLN — the core has no probe API, so the first
// lean handshake is the probe: not negotiated → close, remember, fall
// back).
static bool lean_wss_connect(bool& ok) {
    if (s_mfln == 2) {
        return false;
    }
    char host[65];
    int port = 443;
    split_host_port(s_host, host, sizeof(host), port);
    // Fresh TCP client per attempt, owned by s_client from here on.
    auto* tcp = new WiFiClientSecure();
    pnex_tls_apply(*tcp);  // CA pin when provided, insecure otherwise
    tcp->setBufferSizes(LEAN_TLS_RX, LEAN_TLS_TX);
    char path[224];
    snprintf(path, sizeof(path), "%s?device_id=%s", s_init.ws_path, device_id);
    ok = s_client.connect(tcp, host, (uint16_t)port, path);
    if (!ok) {
        pnex_tls_log_error(*tcp);
    }
    if (s_mfln == 0 && ok) {
        s_mfln = tcp->getMFLNStatus() ? 1 : 2;
        Serial.printf("[TLS] MFLN %d on %s:%d: %s\n", LEAN_TLS_RX, host, port,
                      s_mfln == 1 ? "negotiated (lean buffers)"
                                  : "refused (16 KB buffers)");
        if (s_mfln == 2) {
            s_client.close();
            ok = false;
            return false;
        }
    }
    return true;
}
#endif

// ───────────────────────── Setup ─────────────────────────

void pnex_transport_setup(const PnexTransportInit& init) {
    s_init = init;

    // Décodage base64 de la config compilée — les macros WIFI_SSID/HOST/…
    // arrivent en base64 depuis child_env (env.rs), parité build.sh.
    // Bounded decodes: an oversized value is dropped (empty), never written
    // past its buffer.
    if (cryptoB64DecodeBounded(WIFI_SSID, s_ssid, sizeof(s_ssid)) == PNEX_B64_TOO_LONG)
        Serial.println("[pnex] WIFI_SSID too long, ignored");
    if (cryptoB64DecodeBounded(WIFI_PASSWORD, s_password, sizeof(s_password)) == PNEX_B64_TOO_LONG)
        Serial.println("[pnex] WIFI_PASSWORD too long, ignored");
    if (cryptoB64DecodeBounded(HOST, s_host, sizeof(s_host)) == PNEX_B64_TOO_LONG)
        Serial.println("[pnex] HOST too long, ignored");
    // TOKEN et DEVICE_ID restent en base64 : le contrat d'auth des routes WS
    // (`decode_param`) est « paramètre b64 → décodage serveur → lookup » —
    // l'URL porte les macros telles quelles (envoyés en clair, le rejet
    // 4002 arrive avant l'annonce — leçon du 2026-09-02).
    if (cryptoB64DecodeBounded(DEVICE_ID, s_device_id, sizeof(s_device_id)) == PNEX_B64_TOO_LONG)
        Serial.println("[pnex] DEVICE_ID too long, ignored");

    // Pre-shared key of the Noise link: without a valid one the device
    // never connects (no clear-text fallback, SEC-19).
    if (!cryptoSetKey(ENCRYPTION_KEY)) {
        Serial.println("[CRYPTO] ENCRYPTION_KEY missing or invalid — the device will not connect");
    }

    // Shared TLS posture (WS + future OTA client): decode the optional CA
    // pin once — empty macro = setInsecure posture, unchanged behavior.
    pnex_tls_init(PNEX_CA_CERT);
    pnex_tls_set_client_identity(PNEX_CLIENT_CERT, PNEX_CLIENT_KEY);

    // URL selon WS_SSL compilé (port implicite : 443/80, comme le custom).
    // The token never travels in the URL (D154): Authorization header.
    snprintf(s_conn, sizeof(s_conn), "%s://%s%s?device_id=%s",
             pnex_use_tls() ? "wss" : "ws", s_host, init.ws_path, device_id);
    s_client.setAuthToken(token);
    snprintf(s_conn_log, sizeof(s_conn_log), "%s://%s%s?device_id=%s",
             pnex_use_tls() ? "wss" : "ws", s_host, init.ws_path, device_id);

    // TLS posture is applied per connect (pnex_ws_open builds the client).
    s_client.onMessage(on_frame);
    s_client.onEvent(on_event);
}

// ───────────────────────── Getters ─────────────────────────

const char* pnex_host() { return s_host; }
const char* pnex_device_id() { return s_device_id; }
const char* pnex_conn_string() { return s_conn_log; }

// Raw base64 credentials for the OTA download URL (see pnex_transport.h).
const char* pnex_token_b64() { return token; }
const char* pnex_device_id_b64() { return device_id; }

const char* pnex_ota_pubkey() { return PNEX_OTA_PUBKEY; }

bool pnex_use_tls() {
    return ws_use_tls();
}

bool pnex_crypto_ready() {
    return cryptoReady();
}

// ───────────────────────── WiFi / WS ─────────────────────────

bool pnex_wifi_connect(unsigned max_attempts, PnexWaitTickFn tick) {
    Serial.printf("[WiFi] connexion à %s ", s_ssid);
    WiFi.begin(s_ssid, s_password);
    unsigned attempts = 0;
    while (WiFi.status() != WL_CONNECTED && attempts < max_attempts) {
        // Same 500 ms per attempt, sliced so the screen animation keeps
        // ~20 fps while waiting (tick is a no-op when nullptr).
        for (int slice = 0; slice < 10; ++slice) {
            if (tick) {
                tick();
            }
            delay(50);
        }
        Serial.print(".");
        ++attempts;
#if defined(ESP32)
        yield();  // pas de WDT arme sur la loop Arduino-ESP32
#else
        ESP.wdtFeed();
#endif
    }
    bool ok = WiFi.status() == WL_CONNECTED;
    Serial.println(ok ? " OK" : " ECHEC");
    return ok;
}

// Noise handshake right after the socket opened (D156): sends the first
// message, then polls until the server's answer established the link
// (on_frame) or HANDSHAKE_TIMEOUT_MS elapsed. The mains keep their contract:
// pnex_ws_connect() true = ready to announce.
static bool noise_handshake() {
    const String msg1 = cryptoLinkStartText(s_link, s_device_id);
    if (msg1.length() == 0) {
        Serial.println("[NOISE] no valid ENCRYPTION_KEY — not connecting");
        s_client.close();
        return false;
    }
    s_handshaking = true;
    s_handshake_done = false;
    s_handshake_started_ms = millis();
    if (!s_client.send(msg1)) {
        s_handshaking = false;
        s_client.close();
        return false;
    }
    while (s_handshaking && s_client.available() &&
           millis() - s_handshake_started_ms < HANDSHAKE_TIMEOUT_MS) {
        s_client.poll();
        delay(5);
#if defined(ESP8266)
        ESP.wdtFeed();
#endif
    }
    if (!s_link.ready()) {
        if (s_handshake_done) {
            Serial.println("[WS] session refused by the server after the handshake "
                           "(device already connected? retrying)");
        } else if (!s_client.available()) {
            Serial.println("[NOISE] closed by the server before its answer (key refused?)");
        } else {
            Serial.println("[NOISE] handshake timed out — closing");
        }
        s_handshaking = false;
        s_link.reset();
        s_client.close();
        return false;
    }
    return true;
}

bool pnex_ws_connect() {
    if (s_client.available()) {
        s_client.close();
        delay(100);
    }
    bool ok = false;
#if defined(ESP8266)
    if (!(pnex_use_tls() && lean_wss_connect(ok))) {
        ok = pnex_ws_open(s_client, s_conn);
    }
#else
    ok = pnex_ws_open(s_client, s_conn);
#endif
    if (ok) {
        ok = noise_handshake();
    }
    if (ok) {
#if defined(ESP8266)
        Serial.printf("[WS] heap after connect: %u (max block %u)\n",
                      (unsigned)ESP.getFreeHeap(),
                      (unsigned)ESP.getMaxFreeBlockSize());
#endif
        // Base PONG réarmée à chaque connexion établie.
        s_last_pong_ms = millis();
    }
    return ok;
}

void pnex_ws_poll() {
    s_client.poll();
    // Horloge RELUE après le poll : le callback PONG peut avoir avancé
    // millis() pendant poll() — un `now` d'avant-poll ferait wrapper la
    // soustraction non-signée et déclencherait le timeout immédiatement
    // après avoir reçu… le PONG (leçon 2026-09-03 : la carte bouclait
    // reconnect toutes les ~6 s).
    unsigned long now = millis();
    if (s_init.pong_timeout_ms != 0 && now - s_last_pong_ms >= s_init.pong_timeout_ms) {
        Serial.printf("[WS] PONG timeout (%lu s) — close, safe-states au tour suivant\n",
                      s_init.pong_timeout_ms / 1000);
        s_client.close();
        s_last_pong_ms = millis();
    }
}

void pnex_ws_send_ping() {
    const String wire = cryptoSealText(s_link, "PING");
    if (wire.length() > 0) {
        s_client.send(wire);
    }
}

bool pnex_ws_available() {
    return s_client.available() && s_link.ready();
}

void pnex_ws_send(const char* plain) {
    if (plain == nullptr || plain[0] == '\0') {
        return;
    }
    // Point de passage unique du trafic data (announce/ack/state_report) —
    // les PING passent par pnex_ws_send_ping et ne comptent pas. Simple
    // incrément : jamais sur le chemin de données.
    const String wire = cryptoSealText(s_link, plain);
    if (wire.length() == 0) {
        // Link not ready or payload over the chip budget: dropped here,
        // never sent in clear.
        Serial.println("[PROTO] frame not sent (link not ready or too long)");
        return;
    }
    pnex_status::bump_tx();
    s_client.send(wire);
}

void pnex_ws_close() {
    s_client.close();
}

// ─────────────────────── Callbacks internes ───────────────────────

static void on_frame(const char* data, size_t, bool) {
    if (s_handshaking) {
        // First server frame = second Noise message.
        s_handshaking = false;
        if (!cryptoLinkFinishText(s_link, data)) {
            Serial.println("[NOISE] handshake refused (wrong key or not a PNeX server) — closing");
            s_link.reset();
            s_client.close();
            return;
        }
        Serial.println("[NOISE] link established");
        s_handshake_done = true;
        s_last_pong_ms = millis();
        if (s_init.on_connected) {
            s_init.on_connected();
        }
        return;
    }
    String msg = cryptoOpenText(s_link, data);
    if (msg.length() == 0) {
        // Forged, replayed or oversized: dropped (the link stays usable).
        Serial.println("[NOISE] unauthenticated server frame dropped");
        return;
    }
    msg.trim();
    if (msg == "PONG") {
        // Signe de vie : bookkeeping interne + hook (le générique n'a rien
        // à faire — compteur interne ; soil_sensor y branche son « premier
        // PONG » du boot).
        s_last_pong_ms = millis();
        if (s_init.on_pong) {
            s_init.on_pong();
        }
        return;
    }
    if (s_init.on_message) {
        s_init.on_message(msg);
    }
}

static void on_event(PnexWsEvent event) {
    if (event == PnexWsEvent::Opened) {
        // The Noise handshake is driven by pnex_ws_connect(); the main is
        // told once the link is established (on_frame).
    } else if (event == PnexWsEvent::Closed) {
        s_handshaking = false;
        s_link.reset();
        if (s_init.on_closed) {
            s_init.on_closed();
        }
    } else if (event == PnexWsEvent::GotPing) {
        // Le générique répond au ping WS ; soil_sensor ne le fait pas
        // (comportements d'origine préservés — knob reply_ws_ping).
        if (s_init.reply_ws_ping) {
            s_client.ping();
        }
    } else if (event == PnexWsEvent::GotPong) {
        s_last_pong_ms = millis();
    }
}
