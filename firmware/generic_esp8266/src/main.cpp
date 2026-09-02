// Firmware générique ESP8266 (Brick 0) — comportement 100 % piloté par le
// serveur : pin map poussée dans le `ProvisionAck`, commandes
// `SetMode`/`Write`/`Subscribe` avec `cmd_id` et réponse `Ack` (sémantique
// RPC à la ThingsBoard), lectures cadencées remontées en `StateReport`.
//
// Config device (wifi/hôte/token/device_id) **compilée dans le .bin** —
// décision utilisateur du 2026-09-02 : le device « générique » est compilé
// par device comme les firmwares custom (parité soil_sensor), via les
// variables d'environnement du sous-process `pio run` (base64, common_libs/
// config). Le secteur flash PNEXCFG1 (B0.1 d'origine) a été retiré.
//
// Contrat fil : `crates/pnex-core/src/proto.rs` (miroir ArduinoJson) ;
// framing = base64(nonce 12 ‖ ChaCha20-nu) via common_libs/crypto ; PING
// 5 s / PONG 15 s ; toute perte (close, PONG timeout, WiFi) → sorties en
// safe-state (`forceAllOff`) puis backoff de reconnexion 1 s → 60 s.

#include <ESP8266WiFi.h>
#include <ArduinoWebsockets.h>
#include <ArduinoJson.h>
#include <config.h>
#include "chacha_crypto.h"

using namespace websockets;

static WebsocketsClient client;

// ───────────────────────── Table de pins ─────────────────────────

enum PinMode : uint8_t { M_DIGITAL_IN = 0, M_DIGITAL_OUT = 1, M_ADC_IN = 2 };

struct PinEntry {
    uint8_t gpio;
    uint8_t mode;
    bool pullup;
    bool safe_high;
    uint32_t interval_ms;
    unsigned long last_read_ms;
};

static const uint8_t MAX_PINS = 12;
static PinEntry pins[MAX_PINS];
static uint8_t pin_count = 0;

static PinEntry* pin_by_gpio(uint8_t gpio) {
    for (uint8_t i = 0; i < pin_count; ++i) {
        if (pins[i].gpio == gpio) {
            return &pins[i];
        }
    }
    return nullptr;
}

/// Applique pinMode + niveau initial (safe-state) d'un pin.
static void apply_pin(PinEntry& p) {
    if (p.gpio == 17) {
        return;  // A0 : canal ADC, pas de pinMode
    }
    switch (p.mode) {
        case M_DIGITAL_OUT:
            digitalWrite(p.gpio, p.safe_high ? HIGH : LOW);  // état safe AVANT la sortie
            pinMode(p.gpio, OUTPUT);
            break;
        case M_DIGITAL_IN:
            pinMode(p.gpio, p.pullup ? INPUT_PULLUP : INPUT);
            break;
        default:
            break;  // ADC : rien
    }
}

/// Toutes les sorties vers leur safe-state — appelé sur CHAQUE perte
/// (close, PONG timeout, WiFi down) : brick0.md §8.
static void forceAllOff() {
    for (uint8_t i = 0; i < pin_count; ++i) {
        if (pins[i].mode == M_DIGITAL_OUT) {
            digitalWrite(pins[i].gpio, pins[i].safe_high ? HIGH : LOW);
        }
    }
}

// ───────────────────────── État session ─────────────────────────

static char cfg_ssid[101];
static char cfg_password[101];
static char cfg_host[65];
static char cfg_device_id[65];
static bool wifi_ok = false;
static unsigned long last_ping_ms = 0;
static unsigned long last_pong_ms = 0;
static bool announced = false;
static uint32_t reconnect_delay_ms = 1000;

const unsigned long PING_INTERVAL_MS = 5000;
const unsigned long PONG_TIMEOUT_MS = 15000;

static char conn_str[256];

// Prototypes
void connectWiFi();
void connectWebSocket();
void sendAnnounce();
void sendAck(const char* cmd_id, bool ok, const char* err);
void sendStateReport(const PinEntry& p);
void handleServerMessage(const String& plain);
void applyProvisionAck(JsonDocument& doc);
void handleSetMode(JsonDocument& doc);
void handleWrite(JsonDocument& doc);
void handleSubscribe(JsonDocument& doc);
void onMessageCallback(WebsocketsMessage message);
void onEventsCallback(WebsocketsEvent event, String data);

void setup() {
    Serial.begin(115200);
    delay(500);

    Serial.println("\n[pnex-generic] boot");
    // Décodage base64 de la config compilée (parité soil_sensor) — les
    // macros WIFI_SSID/HOST/… arrivent en base64 depuis child_env (env.rs).
    unsigned int n = cryptoB64Decode(WIFI_SSID, (unsigned char*)cfg_ssid);
    cfg_ssid[n] = '\0';
    n = cryptoB64Decode(WIFI_PASSWORD, (unsigned char*)cfg_password);
    cfg_password[n] = '\0';
    n = cryptoB64Decode(HOST, (unsigned char*)cfg_host);
    cfg_host[n] = '\0';
    // TOKEN et DEVICE_ID restent en base64 : le contrat d'auth des routes WS
    // (`decode_param`) est « paramètre b64 → décodage serveur → lookup » —
    // soil_sensor passe les macros telles quelles, le générique fait pareil
    // (envoyés en clair, le rejet 4002 arrive avant l'annonce — leçon du
    // 2026-09-02).
    n = cryptoB64Decode(DEVICE_ID, (unsigned char*)cfg_device_id);
    cfg_device_id[n] = '\0';
    // Clé des frames WS — SANS elle cryptoReady()=false et cryptoEncryptFrame
    // renvoie le clair (mode mock) : le serveur ne déchiffre rien, l'annonce
    // n'atteint jamais l'admission (leçon 2026-09-02 : 0 instances, boucle
    // PONG timeout, « not provisioned yet »).
    if (!cryptoSetKey(ENCRYPTION_KEY)) {
        Serial.println("[CRYPTO] clé ENCRYPTION_KEY invalide — frames non chiffrées");
    }
    Serial.printf("[pnex-generic] config ok : device_id=%s host=%s ssl=%d\n",
                  cfg_device_id, cfg_host, ws_use_tls());

    ESP.wdtDisable();
    ESP.wdtEnable(WDTO_4S);

    connectWiFi();

    // URL selon WS_SSL compilé (port implicite : 443/80, comme le custom).
    // token/device_id en base64 (macros compilées) — contrat `decode_param`.
    snprintf(conn_str, sizeof(conn_str), "%s://%s/ws/device?token=" TOKEN "&device_id=" DEVICE_ID,
             ws_use_tls() ? "wss" : "ws", cfg_host);
    Serial.printf("[WS] %s\n", conn_str);

    if (ws_use_tls()) {
        client.setInsecure();
    }
    client.onMessage(onMessageCallback);
    client.onEvent(onEventsCallback);

    connectWebSocket();
    last_ping_ms = millis();
    last_pong_ms = millis();
}

void loop() {
    ESP.wdtFeed();
    unsigned long now = millis();

    if (WiFi.status() != WL_CONNECTED) {
        forceAllOff();
        wifi_ok = false;
        connectWiFi();
        return;
    }

    if (!client.available()) {
        forceAllOff();
        announced = false;
        Serial.printf("[WS] reconnect dans %lu ms\n", reconnect_delay_ms);
        delay(reconnect_delay_ms);
        reconnect_delay_ms = reconnect_delay_ms < 60000 ? reconnect_delay_ms * 2 : 60000;
        ESP.wdtFeed();
        connectWebSocket();
        return;
    }

    client.poll();

    // PING 5 s ; silence > 15 s → safe-state + reconnect (§3/§8).
    if (now - last_ping_ms >= PING_INTERVAL_MS) {
        client.send(cryptoEncryptFrame("PING"));
        last_ping_ms = now;
    }
    if (now - last_pong_ms >= PONG_TIMEOUT_MS) {
        Serial.println("[WS] PONG timeout (15 s) — safe-state puis reconnect");
        forceAllOff();
        client.close();
        announced = false;
        last_pong_ms = millis();
        return;
    }

    // Lectures cadencées des pins input souscrits → StateReport.
    for (uint8_t i = 0; i < pin_count; ++i) {
        PinEntry& p = pins[i];
        if (p.interval_ms == 0) {
            continue;
        }
        if (now - p.last_read_ms >= p.interval_ms) {
            p.last_read_ms = now;
            sendStateReport(p);
        }
    }

    delay(5);
}

// ───────────────────────── WiFi / WS ─────────────────────────

void connectWiFi() {
    Serial.printf("[WiFi] connexion à %s ", cfg_ssid);
    WiFi.begin(cfg_ssid, cfg_password);
    int attempts = 0;
    while (WiFi.status() != WL_CONNECTED && attempts < 40) {
        delay(500);
        Serial.print(".");
        ++attempts;
        ESP.wdtFeed();
    }
    wifi_ok = WiFi.status() == WL_CONNECTED;
    Serial.println(wifi_ok ? " OK" : " ECHEC");
}

void connectWebSocket() {
    announced = false;
    if (client.available()) {
        client.close();
        delay(100);
    }
    if (client.connect(conn_str)) {
        Serial.println("[WS] connecté");
        reconnect_delay_ms = 1000;  // succès : retour au backoff minimal
        last_ping_ms = millis();
        last_pong_ms = millis();
        sendAnnounce();
    } else {
        Serial.println("[WS] échec de connexion");
    }
}

// ───────────────────────── Envois ─────────────────────────

static void sendJson(const JsonDocument& doc) {
    char buf[384];
    size_t n = serializeJson(doc, buf, sizeof(buf));
    if (n >= sizeof(buf)) {
        Serial.println("[PROTO] message trop long — ignoré");
        return;
    }
    client.send(cryptoEncryptFrame(buf));
}

void sendAnnounce() {
    JsonDocument doc;
    doc["t"] = "announce";
    doc["chip"] = "esp8266";
    doc["board"] = "nodemcu";
    doc["fw"] = "1.0.0";
    sendJson(doc);
    announced = true;
}

void sendAck(const char* cmd_id, bool ok, const char* err) {
    JsonDocument doc;
    doc["t"] = "ack";
    doc["cmd_id"] = cmd_id;
    doc["ok"] = ok;
    if (err) {
        doc["err"] = err;
    }
    sendJson(doc);
}

void sendStateReport(const PinEntry& p) {
    JsonDocument doc;
    doc["t"] = "state_report";
    doc["gpio"] = p.gpio;
    if (p.mode == M_ADC_IN) {
        doc["value"] = analogRead(A0);
    } else {
        doc["value"] = digitalRead(p.gpio) == HIGH;
    }
    sendJson(doc);
}

// ─────────────────────── Messages serveur ───────────────────────

void handleServerMessage(const String& plain) {
    if (plain == "PONG") {
        last_pong_ms = millis();
        return;
    }
    JsonDocument doc;
    if (deserializeJson(doc, plain) != DeserializationError::Ok) {
        Serial.println("[PROTO] message serveur illisible");
        return;
    }
    const char* type = doc["t"] | "";
    if (strcmp(type, "provision_ack") == 0) {
        applyProvisionAck(doc);
    } else if (strcmp(type, "set_mode") == 0) {
        handleSetMode(doc);
    } else if (strcmp(type, "write") == 0) {
        handleWrite(doc);
    } else if (strcmp(type, "subscribe") == 0) {
        handleSubscribe(doc);
    } else if (strcmp(type, "reject") == 0) {
        Serial.printf("[PROTO] rejet serveur : %s\n", doc["reason"] | "?");
    } else {
        Serial.printf("[PROTO] message inconnu : %s\n", type);
    }
}

void applyProvisionAck(JsonDocument& doc) {
    pin_count = 0;
    forceAllOff();
    JsonArrayConst caps = doc["caps"];
    for (JsonObjectConst cap : caps) {
        if (pin_count >= MAX_PINS) {
            break;
        }
        PinEntry& p = pins[pin_count++];
        p.gpio = cap["gpio"] | 255;
        p.mode = M_DIGITAL_IN;
        const char* mode = cap["mode"] | "digital_in";
        if (strcmp(mode, "digital_out") == 0) {
            p.mode = M_DIGITAL_OUT;
        } else if (strcmp(mode, "analog_in") == 0) {
            p.mode = M_ADC_IN;
        }
        p.pullup = cap["opts"]["pullup"] | false;
        p.safe_high = strcmp(cap["safe_state"] | "low", "high") == 0;
        p.interval_ms = 0;
        p.last_read_ms = 0;
        apply_pin(p);
        Serial.printf("[PINS] %s=GPIO%u mode=%s safe=%s\n",
                      cap["label"] | "?", p.gpio, mode, p.safe_high ? "high" : "low");
    }
}

// Les SetMode/Write/Subscribe ont été validés par caps::validate côté
// serveur AVANT push (brick0.md §8) — le device fait confiance, mais reste
// tolérant aux fautes (Ack err si pin inconnu).

void handleSetMode(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PinEntry* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    const char* mode = doc["mode"] | "digital_in";
    if (strcmp(mode, "digital_out") == 0) {
        p->mode = M_DIGITAL_OUT;
    } else if (strcmp(mode, "analog_in") == 0) {
        p->mode = M_ADC_IN;
    } else {
        p->mode = M_DIGITAL_IN;
    }
    p->pullup = doc["opts"]["pullup"] | false;
    p->safe_high = strcmp(doc["opts"]["safe_state"] | "low", "high") == 0;
    apply_pin(*p);
    sendAck(cmd_id, true, nullptr);
}

void handleWrite(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PinEntry* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    if (p->mode != M_DIGITAL_OUT) {
        sendAck(cmd_id, false, "pin pas en digital_out");
        return;
    }
    bool high;
    JsonVariant v = doc["value"];
    if (v.is<bool>()) {
        high = v.as<bool>();
    } else {
        high = v.as<int>() != 0;
    }
    digitalWrite(gpio, high ? HIGH : LOW);
    // Confirmation immédiate : la boucle UI (polling /pins) voit l'état.
    sendStateReport(*p);
    sendAck(cmd_id, true, nullptr);
}

void handleSubscribe(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PinEntry* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    p->interval_ms = doc["interval_ms"] | 0;
    p->last_read_ms = 0;
    sendAck(cmd_id, true, nullptr);
}

// ─────────────────────── Callbacks WS ───────────────────────

void onMessageCallback(WebsocketsMessage message) {
    String msg = cryptoDecryptFrame(message.data().c_str());
    msg.trim();
    if (msg.length() == 0) {
        Serial.println("[WS] frame illisible (clé ?)");
        return;
    }
    handleServerMessage(msg);
}

void onEventsCallback(WebsocketsEvent event, String data) {
    if (event == WebsocketsEvent::ConnectionOpened) {
        Serial.println("[WS] ouvert");
    } else if (event == WebsocketsEvent::ConnectionClosed) {
        Serial.println("[WS] fermé — sorties en safe-state");
        forceAllOff();
        announced = false;
    } else if (event == WebsocketsEvent::GotPing) {
        client.ping();
    } else if (event == WebsocketsEvent::GotPong) {
        last_pong_ms = millis();
    }
}
