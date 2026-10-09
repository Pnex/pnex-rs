//
// pnex-transport — couche transport commune des firmwares (edge-model.md §3,
// roadmap F1) : WiFi + WebSocket + framing ChaCha + bookkeeping PONG.
//
// La lib contient la MÉCANIQUE partagée ; la POLICY reste dans le main de
// chaque firmware (backoff, safe-states, announce proto, affichage) :
//   - pnex_wifi_connect()  : boucle d'attente WiFi (nombre d'essais en
//     paramètre — 40 pour le générique, 20 pour soil_sensor), wdtFeed dans
//     l'attente ;
//   - pnex_ws_connect()    : une tentative WS (fermeture propre si déjà
//     ouvert, last_pong réarmé) — le backoff/les retries sont du main ;
//   - pnex_ws_poll()       : client.poll() puis horloge RELUE après le poll
//     (leçon 2026-09-03 : millis() pendant le callback avance, une
//     comparaison avec un `now` d'avant-poll wrappe en non-signé et
//     déclenche un « PONG timeout » fantôme) + détection PONG timeout
//     (close ; les safe-states restent du main au tour suivant) ;
//   - bookkeeping PONG interne : text « PONG » (serveur) ET pong WS
//     (GotPong) rearment le compteur ; le timeout est configurable
//     (0 = désactivé — soil_sensor n'en a pas).
//
// Consommateurs : generic_esp8266 (profil pin_slave) et soil_sensor.
// 4_chan_relay (nanopb, déprécié D20) et tft_dev restent hors F1.
//
// ⚠ Règle de linkage : config.h définit `host`/`token`/`device_id`…
// comme des pointeurs GLOBAUX NON const (linkage externe). Ce header est
// le SEUL endroit avec pnex_crypto.cpp où config.h peut être inclus par
// une lib : pnex_transport.cpp est l'unique unité de traduction qui
// l'inclut — les mains passent par les getters (pnex_host(),
// pnex_device_id(), …) et n'incluent plus config.h. Deux TU avec
// config.h = symboles dupliqués au link.
//
#ifndef PNEX_TRANSPORT_H
#define PNEX_TRANSPORT_H

#include <Arduino.h>

// Callbacks optionnels (pointeur nul = ignoré).
typedef void (*PnexOnMessageFn)(const String& plain);  // frame déchiffrée, hors PONG
typedef void (*PnexOnPongFn)();                        // « PONG » texte du serveur
typedef void (*PnexOnConnectedFn)();                   // event ConnectionOpened
typedef void (*PnexOnClosedFn)();                      // event ConnectionClosed

// Tick optionnel pendant l'attente WiFi — pnex.cpp y passe le tick écran
// (le transport reste display-agnostic : pointeur nu, aucune include écran).
typedef void (*PnexWaitTickFn)();

struct PnexTransportInit {
    // Chemin WS du dialecte : « /ws/device » (générique, brick0) ou
    // « /ws/sensor/ingest » (soil_sensor). Le schéma (ws/wss) et les
    // params token/device_id (base64, contrat decode_param) sont internes.
    const char* ws_path;

    // Silence serveur > pong_timeout_ms → close (pnex_ws_poll). 0 = off.
    unsigned long pong_timeout_ms;

    // GotPing (ping WS protocole) → réponse pong automatique. Vrai pour le
    // générique (client.ping() sur GotPing), faux pour soil_sensor.
    bool reply_ws_ping;

    PnexOnMessageFn on_message;
    PnexOnPongFn on_pong;
    PnexOnConnectedFn on_connected;
    PnexOnClosedFn on_closed;
};

/// Décodage de la config compilée (base64 WIFI_SSID/WIFI_PASSWORD/HOST/
/// DEVICE_ID, TOKEN/DEVICE_ID gardés en b64 pour l'URL — contrat
/// `decode_param`), loading of the Noise pre-shared key (ENCRYPTION_KEY), build de
/// l'URL (ws/wss selon WS_SSL), setInsecure si TLS, enregistrement des
/// callbacks. À appeler une fois au setup().
void pnex_transport_setup(const PnexTransportInit& init);

// Config décodée (les mains n'incluent plus config.h — cf. règle supra).
const char* pnex_host();
const char* pnex_device_id();
const char* pnex_conn_string();  // Connection URL for logs, token masked (SEC-W7)
bool pnex_use_tls();             // WS_SSL actif
bool pnex_crypto_ready();        // valid Noise pre-shared key loaded

// Raw base64 credentials (OTA download URL — same wire posture as the WS
// query auth; decoded by the server exactly like /ws/device params).
const char* pnex_token_b64();
const char* pnex_device_id_b64();

// Ed25519 public key (hex) that OTA images must be signed with (SEC-18);
// empty when the firmware was built without one.
const char* pnex_ota_pubkey();

/// Boucle d'attente WiFi bloquante (delay 500 ms/essai, wdtFeed, points de
/// progression) — retourne l'état final. `tick` (optionnel) est appelé à
/// chaque itération AVANT le delay : l'écran reste vivant pendant le
/// connect bloquant sans que cette couche le sache.
bool pnex_wifi_connect(unsigned max_attempts, PnexWaitTickFn tick = nullptr);

/// One connection attempt: clean close if already open (delay 100 ms),
/// socket connect, then the Noise handshake (D156, bounded by 10 s). True
/// = link established, ready to announce; the retries stay in the main.
bool pnex_ws_connect();

/// poll() + horloge relue + PONG timeout (close si dépassé — cf. supra).
/// Le PING est laissé au main (pnex_ws_send_ping + timer du main) : la
/// cadence exacte (>= 5 s vs > 5 s) et les effets d'affichage divergent
/// déjà entre firmwares.
void pnex_ws_poll();

/// « PING » chiffré — schedule et timer dans le main.
void pnex_ws_send_ping();

/// Socket open AND Noise link established.
bool pnex_ws_available();
/// Plain text, sealed on the Noise link when sent (D156); no-op when empty
/// or before the link is established. The JSON size limit stays in the
/// main (sendJson).
void pnex_ws_send(const char* plain);
void pnex_ws_close();

#endif  // PNEX_TRANSPORT_H
