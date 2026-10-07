#include "chacha_crypto.h"

#include <base64.hpp>
#if defined(ESP8266)
#include <bearssl/bearssl_block.h>
#else
#include <esp_system.h>
#include "chacha20_rfc7539.h"
#endif

namespace {

constexpr size_t KEY_LEN = 32;    // ChaCha20 : 256 bits
constexpr size_t NONCE_LEN = 12;  // RFC 7539 : 96 bits, frais par message

// Frames les plus longues : le `ProvisionAck` du device générique. Pire cas
// = MAX_PINS (32) specs à label customs longs ≈ 3,5 Ko en clair → 4096 sur
// la famille ESP32 (RAM large). ESP8266 : heap compté (~40 Ko) et overlays
// ≤ 11 pins (~900 o) → 1024 conservé, un ack plus gros y est jeté
// proprement (jamais d'overflow, cf. garde strlen ci-dessous).
// Leçon 2026-09-21 : l'ancien code (MAX_PLAIN 1024 GLOBAL + check de taille
// APRÈS decode_base64) jetait le ProvisionAck d'un devkit 38p (21 caps
// ≈ 1,9 Ko) — « frame illisible (clé ?) », device jamais provisionné,
// timeline coincée sur « WS » — ET écrivait hors de s_rx au passage.
constexpr size_t MAX_PLAIN =
#if defined(ESP8266)
    1024;
#else
    4096;
#endif
constexpr size_t MAX_WIRE = 4 * ((NONCE_LEN + MAX_PLAIN + 2) / 3);  // b64 paddée

uint8_t s_key[KEY_LEN];
bool s_ready = false;

// Buffers statiques tx/rx distincts — stack ESP8266 comptée (~4 Ko), et
// les deux sens ne se chevauchent pas (envois depuis loop, réceptions
// depuis le poll de la même boucle).
uint8_t s_tx[NONCE_LEN + MAX_PLAIN];
uint8_t s_txWire[MAX_WIRE];
uint8_t s_rx[MAX_WIRE + 1];

/// 12 octets d'aléa — RNG matériel via ESP.random() (valide WiFi actif),
/// équivalent de l'os.urandom(12) par message du protocole serveur.
void fillNonce(uint8_t* nonce) {
    for (size_t i = 0; i < NONCE_LEN; i += 4) {
#if defined(ESP8266)
        uint32_t r = ESP.random();
#else
        uint32_t r = esp_random();
#endif
        memcpy(nonce + i, &r, 4);
    }
}

}  // namespace

bool cryptoSetKey(const char* b64Key) {
    s_ready = false;
    if (!b64Key || !*b64Key) {
        return false;
    }
    if (decode_base64((const unsigned char*)b64Key, s_key) != KEY_LEN) {
        return false;
    }
    s_ready = true;
    return true;
}

bool cryptoReady() {
    return s_ready;
}

String cryptoEncryptFrame(const char* plain) {
    if (!s_ready) {
        return String(plain);  // mock local : pas de clé, pas de chiffre
    }
    size_t len = strlen(plain);
    if (len == 0 || len > MAX_PLAIN) {
        // Hors limite → RIEN (l'appelant pnex_ws_send logge et saute).
        // L'ancien fallback renvoyait le clair : une frame trop longue
        // partait EN CLAIR sur le fil, le serveur la jetait.
        return String();
    }
    fillNonce(s_tx);
    memcpy(s_tx + NONCE_LEN, plain, len);
    // cc=0: the server (RustCrypto chacha20, and the legacy stack before
    // it) encrypts the first block at counter 0.
#if defined(ESP8266)
    br_chacha20_ct_run(s_key, s_tx, 0, s_tx + NONCE_LEN, len);
#else
    pnex_crypto::chacha20_xor(s_key, 0, s_tx, s_tx + NONCE_LEN, len);
#endif
    encode_base64(s_tx, NONCE_LEN + len, s_txWire);  // null-terminée par la lib
    return String((char*)s_txWire);
}

size_t cryptoEncryptBinary(const uint8_t* in, size_t n, uint8_t* out) {
    if (in == nullptr || out == nullptr || n == 0) {
        return 0;
    }
    if (!s_ready) {
        // Mock local: no key, clear frame (memmove: in may alias out + 12).
        memmove(out, in, n);
        return n;
    }
    uint8_t* ct = out + NONCE_LEN;
    if (in != ct) {
        memmove(ct, in, n);
    }
    // Nonce written AFTER the move: with in == out + 12 the head of `out`
    // is free, with a distinct `in` it never overlaps.
    fillNonce(out);
    // Same keystream as cryptoEncryptFrame: counter 0 (RustCrypto chacha20
    // server-side, ws_ingest.rs decrypt_frame).
#if defined(ESP8266)
    br_chacha20_ct_run(s_key, out, 0, ct, n);
#else
    pnex_crypto::chacha20_xor(s_key, 0, out, ct, n);
#endif
    return n + NONCE_LEN;
}

String cryptoDecryptFrame(const char* wire) {
    if (!s_ready) {
        return String(wire);  // mock local : passe-passe transparente
    }
    if (!wire) {
        return String();
    }
    // Borne AVANT tout décodage : un b64 plus long que la capacité fil
    // (ex. ack > MAX_PLAIN) est jeté ICI — l'ancien code décodait d'abord
    // dans s_rx puis jetait : overflow du buffer statique au passage.
    if (strlen(wire) > MAX_WIRE) {
        return String();
    }
    unsigned int len = decode_base64((const unsigned char*)wire, s_rx);
    if (len <= NONCE_LEN || len > MAX_WIRE) {
        return String();
    }
#if defined(ESP8266)
    br_chacha20_ct_run(s_key, s_rx, 0, s_rx + NONCE_LEN, len - NONCE_LEN);
#else
    pnex_crypto::chacha20_xor(s_key, 0, s_rx, s_rx + NONCE_LEN, len - NONCE_LEN);
#endif
    s_rx[len] = '\0';  // decode_base64 ne null-terminate pas sa sortie
    return String((char*)(s_rx + NONCE_LEN));
}

unsigned int cryptoB64Decode(const char* b64, unsigned char* out) {
    return decode_base64((const unsigned char*)b64, out);
}

unsigned int cryptoB64DecodeBounded(const char* b64, char* out, unsigned int cap) {
    if (cap == 0) return PNEX_B64_TOO_LONG;
    out[0] = '\0';
    if (!b64 || !*b64) return 0;
    // Size check BEFORE writing: the decoder itself never bounds its output.
    unsigned int need = decode_base64_length((const unsigned char*)b64);
    if (need >= cap) return PNEX_B64_TOO_LONG;
    unsigned int n = decode_base64((const unsigned char*)b64, (unsigned char*)out);
    out[n] = '\0';
    return n;
}

unsigned int cryptoB64Encode(const unsigned char* in, unsigned int n, char* out) {
    return encode_base64(in, n, (unsigned char*)out);
}
