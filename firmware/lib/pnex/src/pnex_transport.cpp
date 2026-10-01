//
// pnex-transport — implémentation (cf. pnex_transport.h pour le contrat et
// la règle de linkage config.h).
//
#include "pnex_transport.h"

#include "pnex_status.h"  // publication TX (compteur data, no-op sans écran)

// ESP8266 et ESP32 (C3…) — le reste de la lib est de l'Arduino neutre
// (WiFi.begin/status, ArduinoWebsockets, ChaCha) : seul l'en-tête WiFi
// diverge, et le watchdog du connect bloquant.
#if defined(ESP32)
#include <WiFi.h>
#else
#include <ESP8266WiFi.h>
#endif
#include <ArduinoWebsockets.h>

// SEULE unité de traduction incluant config.h à côté des projets qui
// n'utilisent PAS pnex-transport (tft_dev).
#include "pnex_config.h"
#include "chacha_crypto.h"
#include "pnex_tls.h"

using namespace websockets;

// ───────────────────────── État interne ─────────────────────────

static WebsocketsClient s_client;

// Buffers de la config décodée (bornes = validation API : 100 caractères
// SSID/pass, hôte et device_id 64 — parité avec les buffers des mains
// d'origine).
static char s_ssid[101];
static char s_password[101];
static char s_host[65];
static char s_device_id[65];
static char s_conn[256];

static PnexTransportInit s_init;

// Dernier signe de vie serveur (text « PONG » ou pong WS), réarmé au
// connect. Base du PONG timeout (brick0 : 15 s pour le pin_slave).
static unsigned long s_last_pong_ms = 0;

// Prototypes
static void on_frame(WebsocketsMessage message);
static void on_event(WebsocketsEvent event, String data);

#if defined(ESP8266)
// ── ESP8266 wss: lean BearSSL buffers ──
// ArduinoWebsockets builds its own WiFiClientSecure for "wss://" URLs with
// the BearSSL default buffers (~16 KB RX): with the screen + app state the
// heap then OOMs on the first ~1 KB server frame (std::string copy in
// WebsocketsMessage). When the server supports MFLN (nginx/OpenSSL does on
// TLS 1.2), we hand the WS client our own secured TCP client with
// 2048/512 buffers instead (same budget as the OTA HTTPS client). CA
// pinning is unsupported on the 8266 WS path anyway → insecure, as before.
constexpr int LEAN_TLS_RX = 2048;
constexpr int LEAN_TLS_TX = 512;

class LeanTlsTcpClient : public network::SecuredEsp8266TcpClient {
public:
    LeanTlsTcpClient() {
        this->client.setInsecure();
        this->client.setBufferSizes(LEAN_TLS_RX, LEAN_TLS_TX);
    }
    // Whether the server accepted the reduced fragment length.
    bool mfln() { return this->client.getMFLNStatus() != 0; }
};

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

// wss connect through the lean client; false = caller falls back to the
// library path (server without MFLN — the core has no probe API, so the
// first lean handshake is the probe: not negotiated → close, remember,
// fall back).
static bool lean_wss_connect(bool& ok) {
    if (s_mfln == 2) {
        return false;
    }
    char host[65];
    int port = 443;
    split_host_port(s_host, host, sizeof(host), port);
    // Fresh TCP client per attempt (the library does the same for URLs);
    // the callbacks are re-bound on the new WebsocketsClient.
    auto tcp = std::make_shared<LeanTlsTcpClient>();
    s_client = WebsocketsClient(tcp);
    s_client.onMessage(on_frame);
    s_client.onEvent(on_event);
    char path[224];
    snprintf(path, sizeof(path), "%s?token=%s&device_id=%s", s_init.ws_path,
             token, device_id);
    ok = s_client.connect(host, port, path);
    if (s_mfln == 0 && ok) {
        s_mfln = tcp->mfln() ? 1 : 2;
        Serial.printf("[TLS] MFLN %d on %s:%d: %s\n", LEAN_TLS_RX, host, port,
                      s_mfln == 1 ? "negotiated (lean buffers)"
                                  : "refused (16 KB buffers)");
        if (s_mfln == 2) {
            s_client.close();
            s_client = WebsocketsClient();
            s_client.onMessage(on_frame);
            s_client.onEvent(on_event);
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
    unsigned int n = cryptoB64Decode(WIFI_SSID, (unsigned char*)s_ssid);
    s_ssid[n] = '\0';
    n = cryptoB64Decode(WIFI_PASSWORD, (unsigned char*)s_password);
    s_password[n] = '\0';
    n = cryptoB64Decode(HOST, (unsigned char*)s_host);
    s_host[n] = '\0';
    // TOKEN et DEVICE_ID restent en base64 : le contrat d'auth des routes WS
    // (`decode_param`) est « paramètre b64 → décodage serveur → lookup » —
    // l'URL porte les macros telles quelles (envoyés en clair, le rejet
    // 4002 arrive avant l'annonce — leçon du 2026-09-02).
    n = cryptoB64Decode(DEVICE_ID, (unsigned char*)s_device_id);
    s_device_id[n] = '\0';

    // Clé des frames WS — sans elle pnex_crypto_ready()=false et
    // cryptoEncryptFrame renvoie le clair (mode mock) : le serveur ne
    // déchiffre rien, l'annonce n'atteint jamais l'admission (leçon
    // 2026-09-02 : 0 instances, boucle PONG timeout, « not provisioned
    // yet »).
    cryptoSetKey(ENCRYPTION_KEY);

    // Shared TLS posture (WS + future OTA client): decode the optional CA
    // pin once — empty macro = setInsecure posture, unchanged behavior.
    pnex_tls_init(PNEX_CA_CERT);

    // URL selon WS_SSL compilé (port implicite : 443/80, comme le custom).
    snprintf(s_conn, sizeof(s_conn), "%s://%s%s?token=%s&device_id=%s",
             pnex_use_tls() ? "wss" : "ws", s_host, init.ws_path, token, device_id);

    if (pnex_use_tls()) {
        pnex_tls_apply(s_client);
    }
    s_client.onMessage(on_frame);
    s_client.onEvent(on_event);
}

// ───────────────────────── Getters ─────────────────────────

const char* pnex_host() { return s_host; }
const char* pnex_device_id() { return s_device_id; }
const char* pnex_conn_string() { return s_conn; }

// Raw base64 credentials for the OTA download URL (see pnex_transport.h).
const char* pnex_token_b64() { return token; }
const char* pnex_device_id_b64() { return device_id; }

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

bool pnex_ws_connect() {
    if (s_client.available()) {
        s_client.close();
        delay(100);
    }
    bool ok = false;
#if defined(ESP8266)
    if (!(pnex_use_tls() && lean_wss_connect(ok))) {
        ok = s_client.connect(s_conn);
    }
#else
    ok = s_client.connect(s_conn);
#endif
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
    s_client.send(cryptoEncryptFrame("PING"));
}

bool pnex_ws_available() {
    return s_client.available();
}

void pnex_ws_send(const char* plain) {
    if (plain == nullptr || plain[0] == '\0') {
        return;
    }
    // Point de passage unique du trafic data (announce/ack/state_report) —
    // les PING passent par pnex_ws_send_ping et ne comptent pas. Simple
    // incrément : jamais sur le chemin de données.
    const String wire = cryptoEncryptFrame(plain);
    if (wire.length() == 0) {
        // cryptoEncryptFrame refuse > MAX_PLAIN (il renvoyait le CLAIR
        // avant — leçon 2026-09-21) : jeter ici, jamais de frame claire.
        Serial.println("[PROTO] frame trop longue — ignorée");
        return;
    }
    pnex_status::bump_tx();
    s_client.send(wire);
}

void pnex_ws_close() {
    s_client.close();
}

// ─────────────────────── Callbacks internes ───────────────────────

static void on_frame(WebsocketsMessage message) {
    // Frame serveur chiffrée base64(nonce‖ct) → plaintext ; sans clé
    // chargée (mock local), la passe-passe est transparente.
    String msg = cryptoDecryptFrame(message.data().c_str());
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
    // Frame vide (illisible/clé absente) passée au main : le wording du
    // log « illisible » divergeait déjà entre firmwares.
    if (s_init.on_message) {
        s_init.on_message(msg);
    }
}

static void on_event(WebsocketsEvent event, String) {
    if (event == WebsocketsEvent::ConnectionOpened) {
        if (s_init.on_connected) {
            s_init.on_connected();
        }
    } else if (event == WebsocketsEvent::ConnectionClosed) {
        if (s_init.on_closed) {
            s_init.on_closed();
        }
    } else if (event == WebsocketsEvent::GotPing) {
        // Le générique répond au ping WS ; soil_sensor ne le fait pas
        // (comportements d'origine préservés — knob reply_ws_ping).
        if (s_init.reply_ws_ping) {
            s_client.ping();
        }
    } else if (event == WebsocketsEvent::GotPong) {
        s_last_pong_ms = millis();
    }
}
