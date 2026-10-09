//
// Noise NNpsk0 initiator (D156) — see pnex_noise.h for the contract.
// Noise spec references: §5.1 CipherState, §5.2 SymmetricState,
// §5.3 HandshakeState, §9 pre-shared keys.
//
#include "pnex_noise.h"

#include <cstring>

#include "monocypher/monocypher.h"
#include "pnex_sha256.h"

namespace pnex_noise {

namespace {

const char PROTOCOL_NAME[] = "Noise_NNpsk0_25519_ChaChaPoly_SHA256";
const char PROLOGUE_PREFIX[] = "PNEX-NOISE-1|";

// Noise ChaChaPoly nonce: 32 bits of zeros then the 64-bit counter,
// little-endian (spec §12.3).
void nonce_of(uint64_t n, uint8_t out[12]) {
    memset(out, 0, 4);
    for (int i = 0; i < 8; ++i) out[4 + i] = (uint8_t)(n >> (8 * i));
}

}  // namespace

void aead_seal(const uint8_t k[KEY_LEN], uint64_t n, const uint8_t* ad, size_t ad_len,
               const uint8_t* in, size_t len, uint8_t* out) {
    uint8_t nonce[12];
    nonce_of(n, nonce);
    crypto_aead_ctx ctx;
    // One write per fresh context = plain RFC 8439 AEAD (Monocypher only
    // rekeys between successive writes of the same context).
    crypto_aead_init_ietf(&ctx, k, nonce);
    crypto_aead_write(&ctx, out, out + len, ad, ad_len, in, len);
    crypto_wipe(&ctx, sizeof(ctx));
}

bool aead_open(const uint8_t k[KEY_LEN], uint64_t n, const uint8_t* ad, size_t ad_len,
               const uint8_t* in, size_t len, uint8_t* out) {
    if (len < TAG_LEN) return false;
    uint8_t nonce[12];
    nonce_of(n, nonce);
    uint8_t mac[TAG_LEN];
    memcpy(mac, in + len - TAG_LEN, TAG_LEN);
    crypto_aead_ctx ctx;
    crypto_aead_init_ietf(&ctx, k, nonce);
    const int mismatch = crypto_aead_read(&ctx, out, mac, ad, ad_len, in, len - TAG_LEN);
    crypto_wipe(&ctx, sizeof(ctx));
    return mismatch == 0;
}

void Link::mix_hash(const uint8_t* data, size_t len) {
    pnex_crypto::Sha256 s;
    s.init();
    s.update(h_, sizeof(h_));
    s.update(data, len);
    s.finish(h_);
}

void Link::mix_key(const uint8_t* ikm, size_t len) {
    uint8_t ck[32];
    pnex_crypto::noise_hkdf(ck_, ikm, len, ck, k_, nullptr);
    memcpy(ck_, ck, sizeof(ck));
    crypto_wipe(ck, sizeof(ck));
    n_ = 0;
}

void Link::reset() {
    crypto_wipe(this, sizeof(*this));
}

void Link::start(const uint8_t psk[KEY_LEN], const char* device_id,
                 const uint8_t e_priv[KEY_LEN], uint8_t msg1[MSG1_LEN]) {
    reset();
    // InitializeSymmetric: the name is longer than 32 bytes → h = HASH(name).
    pnex_crypto::sha256((const uint8_t*)PROTOCOL_NAME, sizeof(PROTOCOL_NAME) - 1, h_);
    memcpy(ck_, h_, sizeof(ck_));
    // Prologue.
    pnex_crypto::Sha256 s;
    s.init();
    s.update(h_, sizeof(h_));
    s.update((const uint8_t*)PROLOGUE_PREFIX, sizeof(PROLOGUE_PREFIX) - 1);
    s.update((const uint8_t*)device_id, strlen(device_id));
    s.finish(h_);

    // -> psk: MixKeyAndHash(psk).
    uint8_t ck[32], temp_h[32];
    pnex_crypto::noise_hkdf(ck_, psk, KEY_LEN, ck, temp_h, k_);
    memcpy(ck_, ck, sizeof(ck));
    mix_hash(temp_h, sizeof(temp_h));
    n_ = 0;
    crypto_wipe(ck, sizeof(ck));
    crypto_wipe(temp_h, sizeof(temp_h));

    // -> e: public key in clear, MixHash, and MixKey (psk mode, §9.2).
    memcpy(e_priv_, e_priv, KEY_LEN);
    crypto_x25519_public_key(msg1, e_priv_);
    mix_hash(msg1, 32);
    mix_key(msg1, 32);

    // EncryptAndHash(empty payload): just the tag, h as associated data.
    aead_seal(k_, n_, h_, sizeof(h_), nullptr, 0, msg1 + 32);
    ++n_;
    mix_hash(msg1 + 32, TAG_LEN);
}

bool Link::finish(const uint8_t* msg2, size_t len) {
    if (ready_ || len != MSG2_LEN) return false;
    // <- e: the server's ephemeral key, MixHash + MixKey (psk mode).
    mix_hash(msg2, 32);
    mix_key(msg2, 32);
    // <- ee: DH of both ephemerals.
    uint8_t dh[32];
    crypto_x25519(dh, e_priv_, msg2);
    mix_key(dh, sizeof(dh));
    crypto_wipe(dh, sizeof(dh));
    // DecryptAndHash(empty payload).
    uint8_t none[1];
    if (!aead_open(k_, n_, h_, sizeof(h_), msg2 + 32, TAG_LEN, none)) {
        reset();
        return false;
    }
    mix_hash(msg2 + 32, TAG_LEN);
    // Split: the initiator sends with the first key, receives with the second.
    uint8_t k1[32], k2[32];
    pnex_crypto::noise_hkdf(ck_, nullptr, 0, k1, k2, nullptr);
    memcpy(tx_.k, k1, KEY_LEN);
    memcpy(rx_.k, k2, KEY_LEN);
    tx_.n = 0;
    rx_.n = 0;
    crypto_wipe(k1, sizeof(k1));
    crypto_wipe(k2, sizeof(k2));
    crypto_wipe(e_priv_, sizeof(e_priv_));
    crypto_wipe(k_, sizeof(k_));
    crypto_wipe(ck_, sizeof(ck_));
    ready_ = true;
    return true;
}

size_t Link::sealed_len(size_t len) {
    size_t chunks = len == 0 ? 1 : (len + MAX_CHUNK - 1) / MAX_CHUNK;
    return len + chunks * TAG_LEN;
}

size_t Link::seal(const uint8_t* in, size_t len, uint8_t* out) {
    if (!ready_) return 0;
    const size_t chunks = len == 0 ? 1 : (len + MAX_CHUNK - 1) / MAX_CHUNK;
    const uint64_t base = tx_.n;
    // Last chunk first: with out == in, each chunk moves to a higher (or
    // equal) address, so the input of earlier chunks is still intact.
    for (size_t i = chunks; i-- > 0;) {
        const size_t off_in = i * MAX_CHUNK;
        const size_t piece = (i + 1 == chunks) ? len - off_in : MAX_CHUNK;
        uint8_t* dst = out + i * MAX_MSG;
        if (dst != in + off_in) memmove(dst, in + off_in, piece);
        aead_seal(tx_.k, base + i, nullptr, 0, dst, piece, dst);
    }
    tx_.n = base + chunks;
    return sealed_len(len);
}

size_t Link::open(const uint8_t* in, size_t len, uint8_t* out) {
    if (!ready_ || len < TAG_LEN || len > MAX_MSG) return SIZE_MAX;
    if (!aead_open(rx_.k, rx_.n, nullptr, 0, in, len, out)) return SIZE_MAX;
    ++rx_.n;
    return len - TAG_LEN;
}

}  // namespace pnex_noise
