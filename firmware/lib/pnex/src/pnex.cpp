//
// Implémentation PneX — port du main générique (generic_esp8266/
// generic_esp32c3) en lib PIO. Les commentaires « leçon / constat » sont
// conservés tels quels (historique des pièges, à ne pas réapprendre).
//
// Les différences SoC sont absorbées ICI (dernier endroit où elles
// existent) : header WiFi, RNG matériel, watchdog utilisateur (ESP8266
// only), canal ADC (A0 vs GPIO). Les mains des presets n'ont plus AUCUNE
// divergence protocolaire — un seul code path, un seul comportement.
//

#include "Pnex.h"

#include "pnex_io.h"
#include "pnex_screen.h"
#include "pnex_status.h"

#include <ArduinoJson.h>

#if defined(ESP8266)
#include <esp8266_peri.h>  // RANDOM_REG32
#else
#include <esp_system.h>    // esp_random()
// Rollback guard in begin() runs whatever PNEX_OTA_ENABLE says (a USB
// flash also boots PENDING_VERIFY).
#include <esp_ota_ops.h>
#endif

#if PNEX_OTA_ENABLE == 1
#include "pnex_ota.h"
#if defined(ESP32)
#include <Update.h>  // built-in Update library (see env lib_deps)
#include <esp_ota_ops.h>  // esp_ota_mark_app_valid_cancel_rollback
#else
#include <Updater.h>
#endif
#endif

// ───────────────── Pin panel publication (pnex_status hub) ─────────────────

// The pnex_status PinMode mirror must not drift from the real enum — the
// screen samples and formats on these values without seeing Pnex.h.
static_assert((int)PNEX_DIGITAL_IN == (int)pnex_status::PinMode::DigitalIn &&
                  (int)PNEX_DIGITAL_OUT == (int)pnex_status::PinMode::DigitalOut &&
                  (int)PNEX_ADC_IN == (int)pnex_status::PinMode::AdcIn &&
                  (int)PNEX_PWM_OUT == (int)pnex_status::PinMode::PwmOut,
              "pnex_status::PinMode drifted from PnexPinMode");

// Republishes the active pin table for the screen's MAIN pin panel. The
// ProvisionAck table already excludes screen gpios (server-side
// reserved_gpios), so the panel shows exactly the available pins.
static void publish_pins_to_status(const std::vector<PnexPin>& pins) {
    if (!PNEX_HAS_SCREEN) {
        return;
    }
    pnex_status::PinEntry snapshot[pnex_status::PIN_SLOTS];
    size_t n = pins.size();
    if (n > pnex_status::PIN_SLOTS) {
        n = pnex_status::PIN_SLOTS;
    }
    for (size_t i = 0; i < n; i++) {
        pnex_status::PinEntry& e = snapshot[i];
        e.gpio = pins[i].gpio;
        e.mode = (pnex_status::PinMode)pins[i].mode;
        e.duty_pct = pins[i].duty_pct;
        snprintf(e.label, sizeof(e.label), "%.8s", pins[i].label.c_str());
    }
    pnex_status::set_pins(snapshot, n);
}

// ───────────────── Instance unique + trampolines transport ─────────────────

static PnexDevice* s_pnex = nullptr;

void pnex_on_ws_opened() {
    Serial.println("[WS] ouvert");
}

void pnex_on_ws_closed() {
    Serial.println("[WS] fermé — sorties en safe-state");
    if (s_pnex) {
        s_pnex->forceAllOff();
    }
}

void pnex_on_ws_message(const String& plain) {
    pnex_status::bump_rx();  // trafic RX réel (les PONG sont filtrés en amont)
    if (plain.length() == 0) {
        Serial.println("[WS] frame illisible (clé ?)");
        return;
    }
    if (s_pnex) {
        s_pnex->handleServerMessage(plain);
    }
}

// ── Écran : tick pendant les attentes bloquantes du main ──
// pnex.cpp est l'orchestrateur qui connaît les deux bouts ; le transport
// reçoit un pointeur nu, l'écran lit pnex_status — aucun des deux ne
// connaît l'autre.

static void screen_wait_tick() {
    pnex_screen::tick();
}

// delay par tranches qui garde l'écran vivant (backoff de reconnexion) —
// durée totale identique, wdtFeed 8266 conservé.
static void delay_with_screen(unsigned long ms) {
    while (ms > 0) {
        const unsigned long slice = ms > 50 ? 50 : ms;
        delay(slice);
        ms -= slice;
        pnex_screen::tick();
#if defined(ESP8266)
        ESP.wdtFeed();
#endif
    }
}

#if PNEX_HAS_SCREEN
// RSSI → 4 barres (seuils classiques) — lu au slot du timer PING, jamais
// ailleurs : le poll RSSI n'a aucun intérêt en build sans écran.
static uint8_t rssi_level(int rssi) {
    if (rssi >= -60) return 4;
    if (rssi >= -67) return 3;
    if (rssi >= -75) return 2;
    return 1;
}
#endif

// ───────────────────────── État session ─────────────────────────

static unsigned long last_ping_ms = 0;
static uint32_t reconnect_delay_ms = 1000;

// F2 (D46) : identité de boot + numéro d'ordre des reports — clé de dédup
// `(boot_id, seq)` côté serveur quand le buffering existera (F3).
static char boot_id[9];      // 8 hex + zero de fin, tire du RNG materiel au setup
static uint32_t report_seq = 0;

const unsigned long PING_INTERVAL_MS = 5000;

// ───────────────────────── Pin declarations ─────────────────────────

bool PnexDevice::declare(PnexPin p) {
    if (pins_.size() >= PNEX_MAX_PINS) {
        Serial.println("[PINS] PNEX_MAX_PINS atteint — pin ignoré");
        return false;
    }
    if (p.label.isEmpty()) {
        p.label = (p.gpio == 17) ? "a0" : (String("gpio") + (unsigned)p.gpio);
    }
    p.declared = true;
    declared_.push_back(p);
    pins_.push_back(p);
    apply_pin(pins_.back());  // effet local immédiat, serveur ou pas
    Serial.printf("[PINS] %s=GPIO%u mode=%d pullup=%d safe=%s\n",
                  p.label.c_str(), p.gpio, p.mode, p.pullup,
                  p.safe_high ? "high" : "low");
    return true;
}

bool PnexDevice::addInput(uint8_t gpio, const char* label, bool pullup) {
    PnexPin p{};
    p.gpio = gpio;
    p.mode = PNEX_DIGITAL_IN;
    p.pullup = pullup;
    if (label) p.label = String(label);
    return declare(p);
}

bool PnexDevice::addOutput(uint8_t gpio, const char* label, bool safe_high) {
    PnexPin p{};
    p.gpio = gpio;
    p.mode = PNEX_DIGITAL_OUT;
    p.safe_high = safe_high;
    if (label) p.label = String(label);
    return declare(p);
}

bool PnexDevice::addPwmOutput(uint8_t gpio, const char* label, bool safe_high) {
    PnexPin p{};
    p.gpio = gpio;
    p.mode = PNEX_PWM_OUT;
    p.safe_high = safe_high;
    if (label) p.label = String(label);
    return declare(p);
}

bool PnexDevice::addAnalogInput(uint8_t gpio, const char* label) {
    PnexPin p{};
#if defined(ESP8266)
    (void)gpio;  // l'ESP8266 n'a que l'ADC unique A0
    p.gpio = 17;
#else
    p.gpio = gpio;
#endif
    p.mode = PNEX_ADC_IN;
    if (label) p.label = String(label);
    return declare(p);
}

// ───────────────────── Extension points (modules) ─────────────────────

bool PnexDevice::addCap(const char* id, const char* family) {
    if (id == nullptr || family == nullptr || extra_caps_ >= PNEX_MAX_EXTRA_CAPS) {
        Serial.println("[PROTO] extra cap table full — cap ignored");
        return false;
    }
    extra_cap_ids_[extra_caps_] = id;
    extra_cap_families_[extra_caps_] = family;
    ++extra_caps_;
    return true;
}

void PnexDevice::onServerMessage(PnexServerMsgHook hook) {
    msg_hook_ = hook;
}

// ───────────────── Custom metrics and commands (D87/D88) ─────────────────

bool PnexDevice::addMetric(const char* id, const char* unit) {
    if (id == nullptr || id[0] == '\0') {
        Serial.println("[METRIC] empty id — metric ignored");
        return false;
    }
    for (uint8_t i = 0; i < metrics_; ++i) {
        if (strcmp(metric_ids_[i], id) == 0) {
            metric_units_[i] = unit;  // re-declaration updates the unit
            return true;
        }
    }
    if (metrics_ >= PNEX_MAX_METRICS) {
        Serial.printf("[METRIC] PNEX_MAX_METRICS reached — %s ignored\n", id);
        return false;
    }
    metric_ids_[metrics_] = id;
    metric_units_[metrics_] = unit;
    ++metrics_;
    return true;
}

bool PnexDevice::metricReport(const char* id, JsonDocument& doc) {
    if (id == nullptr) {
        return false;
    }
    for (uint8_t i = 0; i < metrics_; ++i) {
        if (strcmp(metric_ids_[i], id) == 0) {
            // D87: no gpio — the server routes the series by cap_id.
            doc["t"] = "state_report";
            doc["cap_id"] = metric_ids_[i];
            doc["uptime_ms"] = (uint32_t)millis();
            doc["boot_id"] = boot_id;
            doc["seq"] = ++report_seq;
            return true;
        }
    }
    Serial.printf("[METRIC] unknown metric %s — declare it with addMetric()\n", id);
    return false;
}

void PnexDevice::publish(const char* id, double value) {
    if (isnan(value) || isinf(value)) {
        return;  // a gap, never an invented value
    }
    JsonDocument doc;
    if (metricReport(id, doc)) {
        doc["value"] = value;
        sendJson(doc);
    }
}

void PnexDevice::publish(const char* id, float value) {
    publish(id, (double)value);
}

void PnexDevice::publish(const char* id, long value) {
    JsonDocument doc;
    if (metricReport(id, doc)) {
        doc["value"] = value;
        sendJson(doc);
    }
}

void PnexDevice::publish(const char* id, int value) {
    publish(id, (long)value);
}

void PnexDevice::publish(const char* id, unsigned long value) {
    JsonDocument doc;
    if (metricReport(id, doc)) {
        doc["value"] = value;
        sendJson(doc);
    }
}

void PnexDevice::publish(const char* id, bool value) {
    JsonDocument doc;
    if (metricReport(id, doc)) {
        doc["value"] = value;
        sendJson(doc);
    }
}

void PnexDevice::publish(const char* id, const char* value) {
    JsonDocument doc;
    if (metricReport(id, doc)) {
        doc["value"] = value;
        sendJson(doc);  // over-long strings are dropped by sendJson (384 B)
    }
}

bool PnexDevice::onCommand(const char* name, PnexCommandHandler handler) {
    if (name == nullptr || name[0] == '\0' || handler == nullptr) {
        Serial.println("[CMD] empty command name or handler — ignored");
        return false;
    }
    for (uint8_t i = 0; i < commands_; ++i) {
        if (strcmp(command_names_[i], name) == 0) {
            command_handlers_[i] = handler;
            return true;
        }
    }
    if (commands_ >= PNEX_MAX_COMMANDS) {
        Serial.printf("[CMD] PNEX_MAX_COMMANDS reached — %s ignored\n", name);
        return false;
    }
    command_names_[commands_] = name;
    command_handlers_[commands_] = handler;
    ++commands_;
    return true;
}

void PnexDevice::handleCommand(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    const char* name = doc["name"] | "";
    for (uint8_t i = 0; i < commands_; ++i) {
        if (strcmp(command_names_[i], name) == 0) {
            bool ok = command_handlers_[i](doc["args"].as<JsonVariantConst>());
            sendAck(cmd_id, ok, ok ? nullptr : "command_failed");
            return;
        }
    }
    sendAck(cmd_id, false, "unknown_command");
}

// ───────────────────────── begin / loop ─────────────────────────

void PnexDevice::begin() {
    s_pnex = this;
    Serial.begin(115200);
    delay(500);

    // Écran de debug local (no-op sans driver compilé) — timeline WiFi
    // clignotante + barre dès la première frame, avant le blocage WiFi.
    pnex_screen::begin();
    pnex_status::set_step(pnex_status::Step::Wifi);
    pnex_screen::tick();

    Serial.println("\n[pnex] boot");
#if defined(ESP8266)
    snprintf(boot_id, sizeof(boot_id), "%08x", RANDOM_REG32);
#else
    snprintf(boot_id, sizeof(boot_id), "%08x", (unsigned)esp_random());
#endif
    // Config compilée décodée + clé ChaCha + URL + callbacks par la couche
    // transport (ex pnex-transport) — le wording du log d'erreur de clé est
    // conservé (il divergeait déjà entre firmwares).
    PnexTransportInit ti;
    ti.ws_path = "/ws/device";
    ti.pong_timeout_ms = 15000;  // silence > 15 s → close (§3/§8)
    ti.reply_ws_ping = true;     // GotPing → client.ping()
    ti.on_connected = pnex_on_ws_opened;
    ti.on_message = pnex_on_ws_message;
    ti.on_pong = nullptr;  // compteur PONG interne à la lib
    ti.on_closed = pnex_on_ws_closed;
    pnex_transport_setup(ti);
    if (!pnex_crypto_ready()) {
        Serial.println("[CRYPTO] ENCRYPTION_KEY invalid — the device will not connect");
    }
    Serial.printf("[pnex] config ok : device_id=%s host=%s ssl=%d\n",
                  pnex_device_id(), pnex_host(), pnex_use_tls());
    pnex_screen::set_device_id(pnex_device_id());

#if defined(ESP8266)
    ESP.wdtDisable();
    ESP.wdtEnable(WDTO_4S);
#endif

    // L'attente bloquante tick l'écran (blink WiFi) via le callback nu du
    // transport — pas de dépendance transport→écran.
    const bool wifi_ok = pnex_wifi_connect(40, &screen_wait_tick);
#if PNEX_HAS_SCREEN
    pnex_status::set_wifi(wifi_ok, wifi_ok ? rssi_level(WiFi.RSSI()) : 0);
#endif

    Serial.printf("[WS] %s\n", pnex_conn_string());
    pnex_status::set_step(pnex_status::Step::Link);
    // Announce AUSSI au premier connect : il ne partait que sur le chemin
    // « reconnexion » de loop() — une connexion propre au boot restait
    // muette pour toujours (pas de ProvisionAck, session fantôme — constat
    // e2e 2026-09-14, invisible ce matin car le device tournait en boucles
    // de reconnexion).
    if (pnex_ws_connect()) {
        pnex_status::set_link(true);
        sendAnnounce();
#if defined(ESP32)
        // Rollback guard (CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y in the
        // arduino-esp32 sdkconfigs): a freshly flashed image boots
        // PENDING_VERIFY — confirm it after a healthy connect+announce,
        // otherwise the bootloader silently reverts to the old slot at the
        // next reboot. No-op when the running image is already valid.
        esp_ota_mark_app_valid_cancel_rollback();
#endif
    }
    last_ping_ms = millis();
}

void PnexDevice::loop() {
#if defined(ESP8266)
    ESP.wdtFeed();
#endif

    if (WiFi.status() != WL_CONNECTED) {
        pnex_status::set_wifi(false, 0);
        pnex_status::set_link(false);
        pnex_screen::tick();
        forceAllOff();
        const bool wifi_ok = pnex_wifi_connect(40, &screen_wait_tick);
#if PNEX_HAS_SCREEN
        pnex_status::set_wifi(wifi_ok, wifi_ok ? rssi_level(WiFi.RSSI()) : 0);
#endif
        return;
    }

    if (!pnex_ws_available()) {
        pnex_status::set_link(false);
        pnex_screen::tick();
        forceAllOff();
        Serial.printf("[WS] reconnect dans %lu ms\n", (unsigned long)reconnect_delay_ms);
        // Backoff inchangé, mais l'écran reste vivant pendant l'attente.
        delay_with_screen(reconnect_delay_ms);
        reconnect_delay_ms = reconnect_delay_ms < 60000 ? reconnect_delay_ms * 2 : 60000;
#if defined(ESP8266)
        ESP.wdtFeed();
#endif
        if (pnex_ws_connect()) {
            Serial.println("[WS] connecté");
            reconnect_delay_ms = 1000;  // succès : retour au backoff minimal
            pnex_status::set_link(true);
            sendAnnounce();
        } else {
            Serial.println("[WS] échec de connexion");
        }
        return;
    }

    pnex_ws_poll();
#if PNEX_OTA_ENABLE == 1
    // A pending OTA runs here, with the WS callback stack unwound.
    if (runPendingOta()) {
        return;
    }
#endif
    // PONG timeout → la lib a fermé la connexion : pas de lecture ce tour,
    // safe-states au prochain tour (branche !available ci-dessus).
    if (!pnex_ws_available()) {
        return;
    }

    unsigned long now = millis();

    // PING 5 s ; silence > 15 s → safe-state puis reconnect (§3/§8 — le
    // timeout est dans pnex_ws_poll).
    if (now - last_ping_ms >= PING_INTERVAL_MS) {
        pnex_ws_send_ping();
        last_ping_ms = now;
#if PNEX_HAS_SCREEN
        // Barres Wi-Fi au slot du timer PING — poll RSSI borné, inutile
        // sans écran.
        pnex_status::set_wifi(true, rssi_level(WiFi.RSSI()));
#endif
    }

    // Lectures cadencées des pins input souscrites → StateReport.
    for (auto& p : pins_) {
        if (p.interval_ms == 0) {
            continue;
        }
        if (now - p.last_read_ms >= p.interval_ms) {
            p.last_read_ms = now;
            sendStateReport(p);
        }
    }

    // Écran : lit l'état publié (pnex_status), ne redessine que ce qui
    // change (no-op sans écran compilé) — la boucle WS reste réactive.
    pnex_screen::tick();

    delay(5);
}

// ───────────────────────── Envois ─────────────────────────

void PnexDevice::sendJson(const JsonDocument& doc) {
    char buf[384];
    size_t n = serializeJson(doc, buf, sizeof(buf));
    if (n >= sizeof(buf)) {
        Serial.println("[PROTO] message trop long — ignoré");
        return;
    }
    pnex_ws_send(buf);
}

void PnexDevice::sendAnnounce() {
    JsonDocument doc;
    doc["t"] = "announce";
    doc["chip"] = PNEX_CHIP;
    doc["board"] = PNEX_BOARD_NAME;
    doc["fw"] = PNEX_FW_VERSION;
    // Manifeste de capacités (D47) — attestation : ce que CE firmware sait
    // faire ; l'overlay board décide toujours de la carte de pins (Tier 1).
    JsonArray caps = doc["caps"].to<JsonArray>();
    JsonObject cap = caps.add<JsonObject>();
    cap["id"] = "digital_state";
    cap["family"] = "state";
    cap = caps.add<JsonObject>();
    cap["id"] = "adc_raw";
    cap["family"] = "measurement";
    cap = caps.add<JsonObject>();
    cap["id"] = "relay_cmd";
    cap["family"] = "actuator";
#if PNEX_SCREEN_SSD1306 == 1 || PNEX_SCREEN_ST7735 == 1
    // (les gates valent 0 si absentes — pas d'écran compilé → pas de cap)
    cap = caps.add<JsonObject>();
    cap["id"] = "screen";
    cap["family"] = "display";
#endif
#if PNEX_OTA_ENABLE == 1
    // OTA admission attestation: this binary consumes
    // ServerMsg::OtaAvailable; the server gates the OTA API on this cap id.
    cap = caps.add<JsonObject>();
    cap["id"] = "ota";
    cap["family"] = "maintenance";
#endif
    // Caps registered by optional modules (e.g. camera/video).
    for (uint8_t i = 0; i < extra_caps_; ++i) {
        cap = caps.add<JsonObject>();
        cap["id"] = extra_cap_ids_[i];
        cap["family"] = extra_cap_families_[i];
    }
    // Custom metrics (D87) and commands (D88) declared by the sketch.
    for (uint8_t i = 0; i < metrics_; ++i) {
        cap = caps.add<JsonObject>();
        cap["id"] = metric_ids_[i];
        cap["family"] = "metric";
        if (metric_units_[i] != nullptr) {
            cap["unit"] = metric_units_[i];
        }
    }
    for (uint8_t i = 0; i < commands_; ++i) {
        cap = caps.add<JsonObject>();
        cap["id"] = command_names_[i];
        cap["family"] = "command";
    }

    // Pins declared by the sketch travel in `pins` — the server validates
    // them (chip-caps) then persists them. Forme fil =
    // PinDecl (proto.rs) : pullup/safe_state absents = défauts (false/low),
    // comme skip_serializing_if côté serde.
    if (!declared_.empty()) {
        JsonArray apins = doc["pins"].to<JsonArray>();
        for (const auto& p : declared_) {
            JsonObject op = apins.add<JsonObject>();
            op["gpio"] = p.gpio;
            op["label"] = p.label;
            switch (p.mode) {
                case PNEX_DIGITAL_OUT:
                    op["mode"] = "digital_out";
                    break;
                case PNEX_PWM_OUT:
                    op["mode"] = "pwm_out";
                    break;
                case PNEX_ADC_IN:
                    op["mode"] = "adc_in";
                    break;
                default:
                    op["mode"] = "digital_in";
                    break;
            }
            if (p.pullup) {
                op["pullup"] = true;
            }
            if (p.safe_high) {
                op["safe_state"] = "high";
            }
        }
    }

    // Static buffer: an announce with declared pins exceeds the 384 B of
    // the other messages (≈ 90 B per pin, up to PNEX_MAX_PINS pins).
    static char ann_buf[3584];
    size_t n = serializeJson(doc, ann_buf, sizeof(ann_buf));
    if (n >= sizeof(ann_buf)) {
        Serial.println("[PROTO] announce trop long — ignoré");
        return;
    }
    pnex_ws_send(ann_buf);
}

void PnexDevice::sendAck(const char* cmd_id, bool ok, const char* err) {
    if (!ok) {
        // Refus : log systématique — une commande perdue/invalide doit se
        // voir sur le moniteur (retour utilisateur « aucune trace des
        // writes », leçon 2026-09-03).
        Serial.printf("[CMD] refus (cmd %s) : %s\n", cmd_id, err ? err : "?");
    }
    JsonDocument doc;
    doc["t"] = "ack";
    doc["cmd_id"] = cmd_id;
    doc["ok"] = ok;
    if (err) {
        doc["err"] = err;
    }
    sendJson(doc);
}

void PnexDevice::sendStateReport(const PnexPin& p) {
    JsonDocument doc;
    doc["t"] = "state_report";
    doc["gpio"] = p.gpio;
    // Real pad/ADC value (programmed duty for pwm_out), also logged so the
    // serial monitor shows what the server receives.
    const int value = pnex_io_read(p);
    if (p.mode == PNEX_ADC_IN || p.mode == PNEX_PWM_OUT) {
        doc["value"] = value;
    } else {
        doc["value"] = value == 1;
    }
    Serial.printf("[IO] GPIO%u mode=%s value=%d\n", p.gpio, pnex_io_mode_name(p.mode), value);
    // F2 (D46) : horloge device — clé de dédup/reconstruction côté serveur.
    doc["uptime_ms"] = (uint32_t)millis();
    doc["boot_id"] = boot_id;
    doc["seq"] = ++report_seq;
    sendJson(doc);
}

// ─────────────────────── Messages serveur ───────────────────────

// Le « PONG » texte est intercepté en amont par le transport (bookkeeping
// du silence serveur) — il n'atteint pas le dispatch proto.

void PnexDevice::handleServerMessage(const String& plain) {
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
#if PNEX_OTA_ENABLE == 1
    } else if (strcmp(type, "ota_available") == 0) {
        handleOtaAvailable(doc);
#endif
    } else if (strcmp(type, "command") == 0) {
        handleCommand(doc);
    } else if (strcmp(type, "reject") == 0) {
        Serial.printf("[PROTO] rejet serveur : %s\n", doc["reason"] | "?");
    } else if (msg_hook_ != nullptr && msg_hook_(doc)) {
        // Consumed by a module hook (it answers its own Ack).
    } else {
        Serial.printf("[PROTO] message inconnu : %s\n", type);
    }
}

#if PNEX_OTA_ENABLE == 1
// Capture-less trampoline for the OTA progress hook — sends the
// DeviceMsg::OtaState frame through the live session.
static PnexDevice* s_ota_self = nullptr;
static char s_ota_cmd_id[40] = {0};
// Deferred job: ota_available only records it; loop() runs the download
// once the WS callback has returned (nesting the HTTPS handshake inside the
// WS frame handler overflowed the ESP32 loopTask stack — silent reset).
static bool s_ota_pending = false;
static char s_ota_url[160] = {0};
static char s_ota_sha[72] = {0};
static char s_ota_version[24] = {0};
static char s_ota_sig[132] = {0};

static void pnex_ota_progress_trampoline(const char* phase, uint8_t pct, const char* err) {
    if (s_ota_self == nullptr) {
        return;
    }
    s_ota_self->sendOtaState(phase, pct, err);
}

// DeviceMsg::OtaState — OTA progress/result frame.
void PnexDevice::sendOtaState(const char* phase, uint8_t pct, const char* err) {
    JsonDocument st;
    st["t"] = "ota_state";
    st["cmd_id"] = s_ota_cmd_id;
    st["phase"] = phase;
    if (pct > 0 || strcmp(phase, "downloading") == 0) {
        st["progress"] = pct;
    }
    if (err) {
        st["err"] = err;
    }
    sendJson(st);
}

// ServerMsg::OtaAvailable — acknowledge, then run the blocking download +
// flash (pnex_ota_run) in the WS callback context (same thread as every
// other handler; progress frames are sent from the download loop, which
// also feeds the server's 45 s watchdog). ESP8266: the WS is closed FIRST
// (single TLS context fits the ~40 KB heap) so no progress is reported —
// the server watchdog + post-reboot announce resolve the assignment.
void PnexDevice::handleOtaAvailable(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    const char* version = doc["version"] | "";
    const char* url_path = doc["url"] | "";
    const char* sha_hex = doc["sha256"] | "";
    const char* sig_hex = doc["sig"] | "";

    // Downgrade guard on every chip (SEC-18): refuse a strictly older
    // numeric version (equal stays allowed: force-redeploy). The version is
    // covered by the image signature, so it cannot be relabelled.
    {
        const unsigned long own = strtoul(PNEX_FW_VERSION, nullptr, 10);
        const unsigned long target = strtoul(version, nullptr, 10);
        if (target > 0 && own > 0 && target < own) {
            Serial.printf("[OTA] refus downgrade %s → %s\n", PNEX_FW_VERSION, version);
            sendAck(cmd_id, false, "downgrade refused");
            return;
        }
    }

    if (s_ota_pending || Update.isRunning()) {
        sendAck(cmd_id, false, "update already running");
        return;
    }

    Serial.printf("[OTA] version %s disponible — démarrage\n", version);
    // Ack BEFORE the long download (and before closing the WS on 8266).
    sendAck(cmd_id, true, nullptr);

    // Context in file statics — single-threaded Arduino loop, one OTA at a
    // time. The download itself runs from loop() (runPendingOta).
    s_ota_self = this;
    strlcpy(s_ota_cmd_id, cmd_id, sizeof(s_ota_cmd_id));
    strlcpy(s_ota_url, url_path, sizeof(s_ota_url));
    strlcpy(s_ota_sha, sha_hex, sizeof(s_ota_sha));
    strlcpy(s_ota_version, version, sizeof(s_ota_version));
    strlcpy(s_ota_sig, sig_hex, sizeof(s_ota_sig));
    s_ota_pending = true;
}

bool PnexDevice::runPendingOta() {
    if (!s_ota_pending) {
        return false;
    }
    s_ota_pending = false;
    PnexOtaHooks hooks;
    hooks.tick = &screen_wait_tick;
    hooks.progress = &pnex_ota_progress_trampoline;
    // ESP8266: free the WS/TLS context before the HTTPS download.
#if defined(ESP8266)
    pnex_ws_close();
#endif

    char err[96];
    if (pnex_ota_run(s_ota_url, s_ota_sha, s_ota_version, s_ota_sig, hooks, err, sizeof(err))) {
        delay(200);  // let the last frame flush
        ESP.restart();
    }
    // Failure: stay on the current firmware (the server watchdog or the
    // device-side report marks the assignment failed).
    Serial.printf("[OTA] failed: %s\n", err);
    return true;
}
#endif  // PNEX_OTA_ENABLE

void PnexDevice::applyProvisionAck(JsonDocument& doc) {
    // Enregistré côté serveur : l'écran tient la timeline pleine ~1 s puis
    // bascule sur MAIN (hold géré côté écran, au tick).
    pnex_status::set_step(pnex_status::Step::Registered);
    // Free the PWM channels of the previous table (a re-provision would
    // otherwise leak them on pins that leave it).
    for (const auto& old : pins_) {
        if (old.mode == PNEX_PWM_OUT) {
            pnex_io_release(old.gpio);
        }
    }
    pins_.clear();  // ordre conservé du main historique (reset AVANT tout)
    JsonArrayConst caps = doc["caps"];
    for (JsonObjectConst cap : caps) {
        if (pins_.size() >= PNEX_MAX_PINS) {
            break;
        }
        PnexPin p{};
        p.gpio = cap["gpio"] | 255;
        p.mode = PNEX_DIGITAL_IN;
        const char* mode = cap["mode"] | "digital_in";
        if (strcmp(mode, "digital_out") == 0) {
            p.mode = PNEX_DIGITAL_OUT;
        } else if (strcmp(mode, "pwm_out") == 0) {
            p.mode = PNEX_PWM_OUT;
        } else if (strcmp(mode, "adc_in") == 0 || strcmp(mode, "analog_in") == 0) {
            // adc_in = sérialisation serde du variant `AdcIn` du proto (le
            // fil n'a JAMAIS porté « analog_in » — convention base only) ;
            // tolérance conservée. Sans lui, A0 tombait en digital_in et un
            // subscribe remontait du digitalRead(17) au lieu d'analogRead
            // (leçon 2026-09-03).
            p.mode = PNEX_ADC_IN;
        }
        p.pullup = cap["opts"]["pullup"] | false;
        p.safe_high = strcmp(cap["safe_state"] | "low", "high") == 0;
        p.label = String(cap["label"] | "?");
        pins_.push_back(p);
        apply_pin(pins_.back());
        Serial.printf("[PINS] %s=GPIO%u mode=%s safe=%s\n",
                      cap["label"] | "?", p.gpio, mode, p.safe_high ? "high" : "low");
    }
    publish_pins_to_status(pins_);
}

// Les SetMode/Write/Subscribe ont été validés par caps::validate côté
// serveur AVANT push (brick0.md §8) — le device fait confiance, mais reste
// tolérant aux fautes (Ack err si pin inconnu).

void PnexDevice::handleSetMode(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PnexPin* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    const char* mode = doc["mode"] | "digital_in";
    if (strcmp(mode, "digital_out") == 0) {
        p->mode = PNEX_DIGITAL_OUT;
    } else if (strcmp(mode, "pwm_out") == 0) {
        p->mode = PNEX_PWM_OUT;
    } else if (strcmp(mode, "adc_in") == 0 || strcmp(mode, "analog_in") == 0) {
        p->mode = PNEX_ADC_IN;
    } else {
        p->mode = PNEX_DIGITAL_IN;
    }
    p->pullup = doc["opts"]["pullup"] | false;
    p->safe_high = strcmp(doc["opts"]["safe_state"] | "low", "high") == 0;
    if (!apply_pin(*p)) {
        // No PWM channel left: the pin is NOT driven — never ack a mode
        // the hardware does not apply.
        p->mode = PNEX_DIGITAL_IN;
        p->pullup = false;
        apply_pin(*p);
        publish_pins_to_status(pins_);
        sendAck(cmd_id, false, "pwm_channels_exhausted");
        return;
    }
    Serial.printf("[CMD] set_mode GPIO%u -> %s (safe=%s)\n",
                  gpio, mode, p->safe_high ? "high" : "low");
    publish_pins_to_status(pins_);
    sendAck(cmd_id, true, nullptr);
}

void PnexDevice::handleWrite(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PnexPin* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    // digital_out : booléen 0/1 ; pwm_out : duty % 0..=100 (mise à
    // l'échelle 8-bit locale — abstraction matérielle côté serveur).
    JsonVariant v = doc["value"];
    if (p->mode == PNEX_PWM_OUT) {
        pnex_io_write_pwm(*p, v.as<int>());
        Serial.printf("[CMD] write GPIO%u -> duty %u%%\n", gpio, (unsigned)p->duty_pct);
    } else if (p->mode == PNEX_DIGITAL_OUT) {
        bool high = v.is<bool>() ? v.as<bool>() : (v.as<int>() != 0);
        const int readback = pnex_io_write_digital(gpio, high);
        Serial.printf("[CMD] write GPIO%u -> %s (readback=%s)\n", gpio, high ? "HIGH" : "LOW",
                      readback ? "HIGH" : "LOW");
        if (readback != (high ? 1 : 0)) {
            // Shorted/loaded output or a pad that cannot drive: the
            // StateReport below carries the real level to the server.
            Serial.printf("[IO] MISMATCH GPIO%u commanded=%d pad=%d\n", gpio, high ? 1 : 0,
                          readback);
        }
    } else {
        sendAck(cmd_id, false, "pin pas en sortie (digital_out/pwm_out)");
        return;
    }
    // Confirmation immédiate : la boucle UI (polling /pins) voit l'état.
    sendStateReport(*p);
    publish_pins_to_status(pins_);
    sendAck(cmd_id, true, nullptr);
}

void PnexDevice::handleSubscribe(JsonDocument& doc) {
    const char* cmd_id = doc["cmd_id"] | "";
    uint8_t gpio = doc["gpio"] | 255;
    PnexPin* p = pin_by_gpio(gpio);
    if (!p) {
        sendAck(cmd_id, false, "pin inconnu");
        return;
    }
    p->interval_ms = doc["interval_ms"] | 0;
    p->last_read_ms = 0;
    Serial.printf("[CMD] subscribe GPIO%u -> %u ms\n", gpio, (unsigned)p->interval_ms);
    sendAck(cmd_id, true, nullptr);
}

// ───────────────────────── Lookups / safe-states ─────────────────────────

PnexPin* PnexDevice::pin_by_gpio(uint8_t gpio) {
    for (auto& p : pins_) {
        if (p.gpio == gpio) {
            return &p;
        }
    }
    return nullptr;
}

/// Applies pinMode + initial level (safe state) of a pin; false when a PWM
/// pin gets no hardware channel.
bool PnexDevice::apply_pin(PnexPin& p) {
    return pnex_io_apply(p);
}

/// Toutes les sorties vers leur safe-state — appelé sur CHAQUE perte
/// (close, PONG timeout, WiFi down) : brick0.md §8.
void PnexDevice::forceAllOff() {
    for (auto& p : pins_) {
        if (p.mode == PNEX_PWM_OUT) {
            pnex_io_write_pwm(p, 0);
        } else if (p.mode == PNEX_DIGITAL_OUT) {
            digitalWrite(p.gpio, p.safe_high ? HIGH : LOW);
        }
    }
}