#include "chacha_crypto.h"

#include <base64.hpp>
#include <bearssl/bearssl_block.h>

namespace {

constexpr size_t KEY_LEN = 32;    // ChaCha20 : 256 bits
constexpr size_t NONCE_LEN = 12;  // RFC 7539 : 96 bits, frais par message

// Frames les plus longues : erreurs serveur `error:invalid_capability:
// measurement '<nom ≤ 100 car.>' not in device capabilities` (~150 o).
constexpr size_t MAX_PLAIN = 160;
constexpr size_t MAX_WIRE = 232;  // b64 de (NONCE_LEN + MAX_PLAIN), paddée

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
        uint32_t r = ESP.random();
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
        return String(plain);
    }
    fillNonce(s_tx);
    memcpy(s_tx + NONCE_LEN, plain, len);
    // cc=0 : le serveur (RustCrypto chacha20, et pycryptodome Django
    // avant lui) chiffre le premier bloc au compteur 0.
    br_chacha20_ct_run(s_key, s_tx, 0, s_tx + NONCE_LEN, len);
    encode_base64(s_tx, NONCE_LEN + len, s_txWire);  // null-terminée par la lib
    return String((char*)s_txWire);
}

String cryptoDecryptFrame(const char* wire) {
    if (!s_ready) {
        return String(wire);  // mock local : passe-passe transparente
    }
    if (!wire) {
        return String();
    }
    unsigned int len = decode_base64((const unsigned char*)wire, s_rx);
    if (len <= NONCE_LEN || len > MAX_WIRE) {
        return String();
    }
    br_chacha20_ct_run(s_key, s_rx, 0, s_rx + NONCE_LEN, len - NONCE_LEN);
    s_rx[len] = '\0';  // decode_base64 ne null-terminate pas sa sortie
    return String((char*)(s_rx + NONCE_LEN));
}

unsigned int cryptoB64Decode(const char* b64, unsigned char* out) {
    return decode_base64((const unsigned char*)b64, out);
}
