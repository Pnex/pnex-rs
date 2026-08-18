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

/// Wrap du décodage base64 (lib densaugeo). Son header est header-only
/// NON inline : inclus par deux unités de traduction, ses fonctions sont
/// définies en double et le link échoue — on ne l'inclut qu'ici, dans
/// chacha_crypto.cpp, et tout le firmware passe par ce wrap.
/// Ne null-terminate PAS la sortie ; retourne la longueur décodée.
unsigned int cryptoB64Decode(const char* b64, unsigned char* out);

#endif  // CHACHA_CRYPTO_H
