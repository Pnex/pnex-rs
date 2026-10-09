//
// Config compilée de la lib PneX — includes par -D defines (contrat
// docs/architecture/firmware-build.md §2.1) : WIFI_SSID/WIFI_PASSWORD/
// HOST/TOKEN/DEVICE_ID EN BASE64, WS_SSL en clair, ENCRYPTION_KEY b64.
//
// ⚠ Règle de linkage : ce header définit `host`/`token`/`device_id`…
// comme des pointeurs GLOBAUX NON const (linkage externe). Il est
// PRIVÉ à la lib : pnex_transport.cpp est l'unique unité de traduction
// qui l'inclut — les sketches/mains passent par les getters
// (pnex_host(), pnex_device_id(), …). Deux TU avec ce header =
// symboles dupliqués au link.
//

#ifndef PNEX_CONFIG_H
#define PNEX_CONFIG_H

// WIFI_SSID/WIFI_PASSWORD sont attendus EN BASE64 (comme HOST/TOKEN/
// DEVICE_ID) : un SSID contient souvent des espaces/quotes qui casseraient
// le flag -D de platformio.ini. Le firmware décode au setup().
#ifndef WIFI_SSID
#define WIFI_SSID "ZGVmYXVsdF9zc2lk"  // base64("default_ssid")
#endif

#ifndef WIFI_PASSWORD
#define WIFI_PASSWORD "ZGVmYXVsdF9wYXNzd29yZA=="  // base64("default_password")
#endif

#ifndef HOST
#define HOST "host_base64"
#endif

#ifndef TOKEN
#define TOKEN "token_base64"
#endif

#ifndef DEVICE_ID
#define DEVICE_ID "device_id_base64"
#endif

// Pre-shared key (32 bytes, BASE64) of the Noise link (D156) — the same as
// device_tokens.encryption_key server-side, injected by the builder /
// task fw:flash (env ENCRYPTION_KEY). Empty or invalid → the device never
// connects (no clear-text mode, SEC-19).
#ifndef ENCRYPTION_KEY
#define ENCRYPTION_KEY ""
#endif

// WebSocket TLS : "true" → wss:// (déploiement industriel derrière TLS),
// "false" → ws:// (serveur local / raspberry pi sans TLS).
#ifndef WS_SSL
#define WS_SSL "true"
#endif

// Optional CA pin (base64 PEM) — empty keeps the setInsecure posture
// (pnex-tls). Consumed via pnex_tls_init(PNEX_CA_CERT) at transport setup.
#ifndef PNEX_CA_CERT
#define PNEX_CA_CERT ""
#endif

// Ed25519 public key (64 hex chars) of the server that signs OTA images
// (SEC-18), set by the server build. Empty = every OTA is refused.
#ifndef PNEX_OTA_PUBKEY
#define PNEX_OTA_PUBKEY ""
#endif

// TLS client certificate + key of this device (D153), base64 PEM, issued
// by the org CA at build time. Empty = no client certificate.
#ifndef PNEX_CLIENT_CERT
#define PNEX_CLIENT_CERT ""
#endif
#ifndef PNEX_CLIENT_KEY
#define PNEX_CLIENT_KEY ""
#endif

// ssid/password ne sont plus exposés ici : décodés au setup() de chaque
// firmware (char ssid[]/password[] locaux, remplis via decode_base64).
const char* host = HOST;
const char* token = TOKEN;
const char* device_id = DEVICE_ID;
const char* encryption_key = ENCRYPTION_KEY;
const char* ws_ssl = WS_SSL;

// Vrai si WS_SSL active le TLS ("1", "true", "yes" — insensible à la casse
// sur le premier caractère).
inline bool ws_use_tls() {
    char c = ws_ssl[0];
    return c == '1' || c == 't' || c == 'T' || c == 'y' || c == 'Y';
}

#endif // PNEX_CONFIG_H
