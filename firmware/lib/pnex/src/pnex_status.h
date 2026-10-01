//
// pnex-status — état réseau publié pour l'écran de debug local.
//
// Découplage strict (contrat écran, pnex_screen.h) : le réseau PUBLIE ici
// (setters O(1), jamais bloquants, jamais sur le chemin de données) ;
// pnex_screen CONSOMME au tick. Cette en-tête ne référence aucun driver
// d'affichage — la logique réseau ne sait rien de l'écran.
//
// Boucle Arduino single-thread (les callbacks WS sont tirés dans poll() sur
// le même thread) : statiques simples, pas d'atomique.
//
// PNEX_HAS_SCREEN=0 (aucune gate écran compilée) → tous les setters se
// replient en no-op : zéro mise à jour d'indicateur en build sans écran,
// chemin de données identique.
//

#ifndef PNEX_STATUS_H
#define PNEX_STATUS_H

#include <Arduino.h>

// Gates écran (0 par défaut — mêmes défauts que pnex_screen.cpp, le builder
// les pousse toujours via ${sysenv.*}).
#ifndef PNEX_SCREEN_SSD1306
#define PNEX_SCREEN_SSD1306 0
#endif
#ifndef PNEX_SCREEN_ST7735
#define PNEX_SCREEN_ST7735 0
#endif

#define PNEX_HAS_SCREEN (PNEX_SCREEN_SSD1306 == 1 || PNEX_SCREEN_ST7735 == 1)

namespace pnex_status {

// Étapes de connexion affichées : WiFi → Link (WS) → Registered.
enum class Step : uint8_t {
    Wifi = 0,       // connexion au point d'accès
    Link = 1,       // WiFi OK, connexion au serveur (WS)
    Registered = 2, // provision_ack reçu — enregistré côté serveur
};

// Pin modes — mirror of the lib's PnexPinMode enum (Pnex.h). This header
// must not pull the whole transport stack, so the values are duplicated
// here; pnex.cpp static_asserts the mirror stays in sync.
enum class PinMode : uint8_t {
    DigitalIn = 0,
    DigitalOut = 1,
    AdcIn = 2,
    PwmOut = 3,
};

// One provisioned pin as shown in the MAIN pin panel. Static config only —
// values are NOT stored here: the screen samples the gpios directly at draw
// time (live, subscription-free).
struct PinEntry {
    uint8_t gpio;
    PinMode mode;
    uint8_t duty_pct;  // PWM: last written duty 0..=100
    char label[9];     // overlay label, truncated (8 chars)
};

// Snapshot capacity — mirrors PNEX_MAX_PINS (the lib announce guard).
constexpr size_t PIN_SLOTS = 32;

struct NetState {
    Step step = Step::Wifi;
    bool wifi_ok = false;
    uint8_t wifi_level = 0;  // 0-4 barres (RSSI mappé)
    bool link_ok = false;    // WS ouvert
    uint32_t tx_count = 0;   // frames data émises (hors PING)
    uint32_t rx_count = 0;   // frames data reçues (hors PONG)
    // OTA deployment (a future screen page can render a progress bar).
    char ota_phase[12] = {0};  // "" | "downloading" | "flashing" | "failed"
    uint8_t ota_progress = 0;  // 0..=100
    // Provisioned pin table (ProvisionAck) — the board overlay already
    // excludes screen gpios server-side, so this is exactly the available
    // pins set ("hors pins écran").
    PinEntry pins[PIN_SLOTS];
    uint8_t pin_count = 0;
};

// Stockage (magic static — init thread-safe, ici mono-thread de toute façon).
inline NetState& rw() {
    static NetState s;
    return s;
}

// Setters — appelés du chemin réseau ; la garde est une constante compilée
// (repli no-op en build sans écran).
inline void set_step(Step s) {
    if (PNEX_HAS_SCREEN) {
        rw().step = s;
    }
}

inline void set_wifi(bool ok, uint8_t level) {
    if (PNEX_HAS_SCREEN) {
        rw().wifi_ok = ok;
        rw().wifi_level = ok ? level : 0;
    }
}

inline void set_link(bool ok) {
    if (PNEX_HAS_SCREEN) {
        rw().link_ok = ok;
    }
}

inline void bump_tx() {
    if (PNEX_HAS_SCREEN) {
        ++rw().tx_count;
    }
}

inline void bump_rx() {
    if (PNEX_HAS_SCREEN) {
        ++rw().rx_count;
    }
}

inline void set_ota(const char* phase, uint8_t progress) {
    if (PNEX_HAS_SCREEN) {
        auto& s = rw();
        strncpy(s.ota_phase, phase, sizeof(s.ota_phase) - 1);
        s.ota_phase[sizeof(s.ota_phase) - 1] = '\0';
        s.ota_progress = progress;
    }
}

// Full-table republish (ProvisionAck, SetMode, PWM write — the panel only
// needs static config; duty travels with the next PWM write).
inline void set_pins(const PinEntry* entries, size_t n) {
    if (!PNEX_HAS_SCREEN) {
        return;
    }
    auto& s = rw();
    if (n > PIN_SLOTS) {
        n = PIN_SLOTS;  // = PNEX_MAX_PINS (the lib guard, mirrored by the array)
    }
    memcpy(s.pins, entries, n * sizeof(PinEntry));
    s.pin_count = (uint8_t)n;
}

// Lecture côté écran (au tick).
inline const NetState& state() {
    return rw();
}

}  // namespace pnex_status

#endif  // PNEX_STATUS_H
