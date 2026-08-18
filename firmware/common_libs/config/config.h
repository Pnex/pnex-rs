//
// Created by shan on 15/05/24.
//

#ifndef CONFIG_H
#define CONFIG_H

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

// Clé ChaCha20 (32 octets) des frames WS, EN BASE64 — la même que
// device_tokens.encryption_key côté serveur, injectée par le builder /
// task fw:flash (env ENCRYPTION_KEY). Vide → frames en clair, réservé au
// mock local ws-server/ : le serveur réel les rejette.
#ifndef ENCRYPTION_KEY
#define ENCRYPTION_KEY ""
#endif

// WebSocket TLS : "true" → wss:// (déploiement industriel derrière TLS),
// "false" → ws:// (serveur local / raspberry pi sans TLS).
#ifndef WS_SSL
#define WS_SSL "true"
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

#endif //CONFIG_H
