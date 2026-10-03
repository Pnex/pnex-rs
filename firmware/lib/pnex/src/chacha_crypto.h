//
// Chiffrement des frames du protocole d'ingest WS — miroir exact du
// serveur (pnex-rust, crates/pnex-backend/src/controllers/ws_ingest.rs) :
// chaque frame, dans les deux sens, est le texte base64(nonce 12 ‖
// ChaCha20-nu ct), nonce frais par message (RFC 7539, sans Poly1305 —
// pas d'AEAD, décision D8).
//
// La clé (32 octets, base64 de 44 car. — device_tokens.encryption_key
// côté serveur) est injectée au build via -D ENCRYPTION_KEY
// (platformio.ini ← env du même nom, posée par le builder ou
// task fw:flash). Sans clé valide, les frames passent EN CLAIR des deux
// côtés : mode mock local (ws-server/) qui ne chiffre pas — le serveur
// réel les rejetterait (ERROR:decryption_failed à tout, device jamais
// « actif »).
//
#ifndef CHACHA_CRYPTO_H
#define CHACHA_CRYPTO_H

#include <Arduino.h>

/// Décode la clé base64 (32 octets) ; faux si absente/invalide — dans ce
/// cas les frames circulent en clair (mock local).
bool cryptoSetKey(const char* b64Key);

/// Vrai si une clé valide est chargée.
bool cryptoReady();

/// plaintext → base64(nonce 12 ‖ ct). Retourne le plaintext tel quel si
/// la clé n'est pas chargée (mock local) ou le payload hors bornes.
String cryptoEncryptFrame(const char* plain);

/// base64(nonce 12 ‖ ct) → plaintext ; sans clé chargée, passe-passe
/// transparente (mock local). "" si la frame est illisible.
String cryptoDecryptFrame(const char* wire);

/// Nonce length prefixed to every encrypted frame (RFC 7539, 96 bits).
constexpr size_t CRYPTO_NONCE_LEN = 12;

/// Binary frame encryption (camera uplink, camera-video.md D73): writes
/// `nonce(12) || ChaCha20(in[0..n])` into `out` and returns n + 12 — same
/// key, same raw RFC 7539 ChaCha20 (counter 0, no Poly1305) as the text
/// frames, minus base64. `out` must hold n + 12 bytes; `in` may alias
/// `out + 12` (in-place, the usual camera path) — any other overlap is
/// unsupported. Without a loaded key the frame passes in clear: `in` is
/// moved to `out` and n is returned. Returns 0 when n == 0.
size_t cryptoEncryptBinary(const uint8_t* in, size_t n, uint8_t* out);

/// Wrap du décodage base64 (lib densaugeo). Son header est header-only
/// NON inline : inclus par deux unités de traduction, ses fonctions sont
/// définies en double et le link échoue — on ne l'inclut qu'ici, dans
/// chacha_crypto.cpp, et tout le firmware passe par ce wrap.
/// Ne null-terminate PAS la sortie ; retourne la longueur décodée.
unsigned int cryptoB64Decode(const char* b64, unsigned char* out);

/// Base64 encode through the same wrapped lib (single-TU rule above).
/// `out` must hold 4 * ceil(n / 3) + 1 bytes; it is null-terminated.
/// Returns the encoded length.
unsigned int cryptoB64Encode(const unsigned char* in, unsigned int n, char* out);

#endif  // CHACHA_CRYPTO_H
