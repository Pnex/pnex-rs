//
// Config compilée de la lib PneX — includes par -D defines (contrat
// docs/architecture/firmware-build.md §2.1) : WIFI_SSID/WIFI_PASSWORD/
// HOST/TOKEN/DEVICE_ID EN BASE64, ENCRYPTION_KEY b64. Always wss/https
// (D154): there is no clear-text build.
//
// Private to the lib: pnex_transport.cpp is the only translation unit
// that includes it — sketches go through the getters (pnex_host(),
// pnex_device_id(), pnex_token()…).
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
// device_tokens.encryption_key server-side, injected by the builder (env
// ENCRYPTION_KEY). Empty or invalid → the device never
// connects (no clear-text mode, SEC-19).
#ifndef ENCRYPTION_KEY
#define ENCRYPTION_KEY ""
#endif

// CA pin (base64 PEM), required: empty = every TLS handshake is refused
// (pnex-tls, no setInsecure). Consumed via pnex_tls_init(PNEX_CA_CERT).
#ifndef PNEX_CA_CERT
#define PNEX_CA_CERT ""
#endif

// Ed25519 public key (64 hex chars) of the server that signs OTA images
// (SEC-18), set by the server build. Empty = every OTA is refused.
#ifndef PNEX_OTA_PUBKEY
#define PNEX_OTA_PUBKEY ""
#endif

// TLS client certificate + key of this device (D153), base64 PEM, issued
// by the org CA at build time. Empty = refused by the server (4014).
#ifndef PNEX_CLIENT_CERT
#define PNEX_CLIENT_CERT ""
#endif
#ifndef PNEX_CLIENT_KEY
#define PNEX_CLIENT_KEY ""
#endif

#endif // PNEX_CONFIG_H
