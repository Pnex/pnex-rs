#include <cstring>
#include <cmath>
#include <limits>
#include <stddef.h>

#include <unity.h>

#include "pnex_control.h"
#include "goldens.h"
#include "monocypher/monocypher.h"
#include "pnex_camera_frame.h"
#include "pnex_noise.h"
#include "pnex_sha256.h"
#include "noise_goldens.h"
#include "pnex_ota_sig.h"
#include "ota_sig_goldens.h"

void setUp(void) {}
void tearDown(void) {}

static const double EPS = 1e-9;

void test_tt_golden_vectors(void) {
    for (size_t s = 0; s < pnex_goldens::TT_N; ++s) {
        const pnex_goldens::TtScenario& sc = pnex_goldens::TT_SCENARIOS[s];
        for (size_t i = 0; i < sc.n; ++i) {
            const pnex_goldens::TtStep& st = sc.steps[i];
            bool got = pnex_control::tt_step(sc.heat, sc.setpoint, sc.deadband,
                                             st.current_on, st.measurement);
            TEST_ASSERT_EQUAL_UINT8_MESSAGE(st.expected_on, got, sc.name);
        }
    }
}

void test_pid_golden_vectors_replay(void) {
    for (size_t s = 0; s < pnex_goldens::PID_N; ++s) {
        const pnex_goldens::PidScenario& sc = pnex_goldens::PID_SCENARIOS[s];
        pnex_control::PidState st;
        for (size_t i = 0; i < sc.n; ++i) {
            const pnex_goldens::PidStep& step = sc.steps[i];
            double duty = pnex_control::pid_step(sc.setpoint, sc.kp, sc.ki, sc.kd,
                                                 step.measurement, step.dt_secs, st);
            TEST_ASSERT_DOUBLE_WITHIN_MESSAGE(EPS, step.expected_duty, duty, sc.name);
            TEST_ASSERT_DOUBLE_WITHIN_MESSAGE(EPS, step.expected_integral,
                                              st.integral, sc.name);
        }
    }
}

void test_relay_golden_vectors(void) {
    for (size_t s = 0; s < pnex_goldens::RELAY_N; ++s) {
        const pnex_goldens::RelayScenario& sc = pnex_goldens::RELAY_SCENARIOS[s];
        for (size_t i = 0; i < sc.n; ++i) {
            const pnex_goldens::RelayStep& st = sc.steps[i];
            bool got = pnex_control::relay_window(sc.duty_pct, sc.cycle_time_secs,
                                                  st.elapsed_secs);
            TEST_ASSERT_EQUAL_UINT8_MESSAGE(st.expected_on, got, sc.name);
        }
    }
}

void test_tt_deadband_nan_jamais_on(void) {
    const double nan = std::numeric_limits<double>::quiet_NaN();
    TEST_ASSERT_FALSE(pnex_control::tt_step(true, 19.0, nan, false, 10.0));
}

void test_relay_duty_nan_jamais_on(void) {
    const double nan = std::numeric_limits<double>::quiet_NaN();
    TEST_ASSERT_FALSE(pnex_control::relay_window(nan, 10.0, 0.0));
}

void test_pid_dt_nul_d_zero(void) {
    pnex_control::PidState st;
    double duty = pnex_control::pid_step(20.0, 1.0, 0.0, 5.0, 19.0, 0.0, st);
    TEST_ASSERT_DOUBLE_WITHIN(EPS, 1.0, duty);
}

// ChaCha20-Poly1305 AEAD, RFC 8439 §2.8.2, through the vendored Monocypher
// (the primitive under the Noise transport).
void test_aead_chacha20_poly1305_rfc8439(void) {
    uint8_t key[32];
    for (int i = 0; i < 32; ++i) key[i] = (uint8_t)(0x80 + i);
    static const uint8_t NONCE[12] = {0x07, 0x00, 0x00, 0x00, 0x40, 0x41,
                                      0x42, 0x43, 0x44, 0x45, 0x46, 0x47};
    static const uint8_t AAD[12] = {0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1,
                                    0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7};
    static const uint8_t CT_HEAD[16] = {0xd3, 0x1a, 0x8d, 0x34, 0x64, 0x8e, 0x60, 0xdb,
                                        0x7b, 0x86, 0xaf, 0xbc, 0x53, 0xef, 0x7e, 0xc2};
    static const uint8_t TAG[16] = {0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a,
                                    0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60, 0x06, 0x91};
    const char* plain =
        "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for "
        "the future, sunscreen would be it.";
    const size_t len = strlen(plain);
    uint8_t ct[128];
    uint8_t mac[16];
    crypto_aead_ctx ctx;
    crypto_aead_init_ietf(&ctx, key, NONCE);
    crypto_aead_write(&ctx, ct, mac, AAD, sizeof(AAD), (const uint8_t*)plain, len);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(CT_HEAD, ct, 16);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(TAG, mac, 16);
    uint8_t back[128];
    crypto_aead_init_ietf(&ctx, key, NONCE);
    TEST_ASSERT_EQUAL_INT(0, crypto_aead_read(&ctx, back, mac, AAD, sizeof(AAD), ct, len));
    TEST_ASSERT_EQUAL_MEMORY(plain, back, len);
}

// PXC1 camera frame header — same bytes as the Rust golden vector
// `header_golden_bytes` (crates/pnex-core/src/camera.rs).
void test_camera_header_golden_bytes(void) {
    static const uint8_t EXPECTED[16] = {'P', 'X', 'C', '1', 1, 0, 0, 0,
                                         2,   0,   0,   0,   0x80, 0x02, 0xE0, 0x01};
    uint8_t out[16];
    pnex_camera_frame::encode_header(out, 1, 2, 640, 480);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(EXPECTED, out, 16);
}

// SHA-256 FIPS 180-4 "abc" and the two-block message.
void test_sha256_fips_vectors(void) {
    static const uint8_t ABC[32] = {
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad};
    static const uint8_t TWO_BLOCKS[32] = {
        0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8, 0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e, 0x60, 0x39,
        0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67, 0xf6, 0xec, 0xed, 0xd4, 0x19, 0xdb, 0x06, 0xc1};
    uint8_t out[32];
    pnex_crypto::sha256((const uint8_t*)"abc", 3, out);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(ABC, out, 32);
    const char* m = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
    pnex_crypto::sha256((const uint8_t*)m, strlen(m), out);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(TWO_BLOCKS, out, 32);
}

// HMAC-SHA256, RFC 4231 test case 2 (key "Jefe").
void test_hmac_sha256_rfc4231(void) {
    static const uint8_t EXPECTED[32] = {
        0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75, 0xc7,
        0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec, 0x38, 0x43};
    const char* msg = "what do ya want for nothing?";
    uint8_t out[32];
    // Split message: the two-part API must equal the one-part digest.
    pnex_crypto::hmac_sha256((const uint8_t*)"Jefe", 4, (const uint8_t*)msg, 10,
                             (const uint8_t*)msg + 10, strlen(msg) - 10, out);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(EXPECTED, out, 32);
}

// Noise link: the C++ initiator reproduces the Rust reference byte for byte.
void test_noise_golden_handshake_and_frames(void) {
    namespace g = pnex_noise_goldens;
    pnex_noise::Link link;
    uint8_t msg1[pnex_noise::MSG1_LEN];
    link.start(g::PSK, g::DEVICE_ID, g::E_DEVICE, msg1);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(g::MSG1, msg1, sizeof(msg1));
    TEST_ASSERT_TRUE(link.finish(g::MSG2, sizeof(g::MSG2)));

    uint8_t buf[64];
    size_t n = link.seal((const uint8_t*)"PING", 4, buf);
    TEST_ASSERT_EQUAL_UINT32(sizeof(g::UP_PING), n);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(g::UP_PING, buf, n);
    const char* announce = "{\"t\":\"announce\"}";
    n = link.seal((const uint8_t*)announce, strlen(announce), buf);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(g::UP_ANNOUNCE, buf, n);

    // Chunked frame, sealed in place (the camera path).
    static uint8_t large[80000];
    for (size_t i = 0; i < g::LARGE_LEN; ++i) large[i] = (uint8_t)(i % 251);
    n = link.seal(large, g::LARGE_LEN, large);
    TEST_ASSERT_EQUAL_UINT32(g::UP_LARGE_LEN, n);
    uint8_t digest[32];
    pnex_crypto::sha256(large, n, digest);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(g::UP_LARGE_SHA256, digest, 32);

    uint8_t plain[16];
    n = link.open(g::DOWN_PONG, sizeof(g::DOWN_PONG), plain);
    TEST_ASSERT_EQUAL_UINT32(4, n);
    TEST_ASSERT_EQUAL_MEMORY("PONG", plain, 4);
    // Replayed: refused.
    TEST_ASSERT_EQUAL_UINT32(SIZE_MAX, link.open(g::DOWN_PONG, sizeof(g::DOWN_PONG), plain));
}

// A wrong key or device id fails the handshake; a tampered frame is refused
// and leaves the link usable.
void test_noise_refusals(void) {
    namespace g = pnex_noise_goldens;
    pnex_noise::Link link;
    uint8_t msg1[pnex_noise::MSG1_LEN];
    uint8_t other[32];
    memcpy(other, g::PSK, 32);
    other[0] ^= 1;
    link.start(other, g::DEVICE_ID, g::E_DEVICE, msg1);
    TEST_ASSERT_FALSE(link.finish(g::MSG2, sizeof(g::MSG2)));
    link.start(g::PSK, "other-dev", g::E_DEVICE, msg1);
    TEST_ASSERT_FALSE(link.finish(g::MSG2, sizeof(g::MSG2)));

    link.start(g::PSK, g::DEVICE_ID, g::E_DEVICE, msg1);
    TEST_ASSERT_TRUE(link.finish(g::MSG2, sizeof(g::MSG2)));
    uint8_t forged[sizeof(g::DOWN_PONG)];
    memcpy(forged, g::DOWN_PONG, sizeof(forged));
    forged[1] ^= 0x01;
    uint8_t plain[16];
    TEST_ASSERT_EQUAL_UINT32(SIZE_MAX, link.open(forged, sizeof(forged), plain));
    TEST_ASSERT_EQUAL_UINT32(4, link.open(g::DOWN_PONG, sizeof(g::DOWN_PONG), plain));
}

// SEC-18: the C++ checker accepts the signature made by the Rust signer
// (ed25519-dalek) and refuses any change of key, device, version, digest or
// signature.
void test_ota_signature_golden(void) {
    namespace g = pnex_ota_sig_goldens;
    uint8_t digest[32];
    TEST_ASSERT_TRUE(pnex_hex_decode(g::SHA256_HEX, digest, sizeof(digest)));
    TEST_ASSERT_TRUE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, g::VERSION, digest, g::SIG_HEX));

    // Another device or another version: refused.
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, "golden-deV", g::VERSION, digest, g::SIG_HEX));
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, "1235", digest, g::SIG_HEX));
    // Length prefixes: moving a byte between the fields is not a match.
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, "golden-dev1", "234", digest, g::SIG_HEX));

    uint8_t other[32];
    memcpy(other, digest, sizeof(other));
    other[31] ^= 0x01;
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, g::VERSION, other, g::SIG_HEX));

    char sig[129];
    strcpy(sig, g::SIG_HEX);
    sig[0] = sig[0] == '0' ? '1' : '0';
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, g::VERSION, digest, sig));

    char key[65];
    strcpy(key, g::PUBKEY_HEX);
    key[63] = key[63] == '0' ? '1' : '0';
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(key, g::DEVICE_ID, g::VERSION, digest, g::SIG_HEX));

    // Malformed inputs fail closed: no key compiled, truncated signature.
    TEST_ASSERT_FALSE(pnex_ota_sig_verify("", g::DEVICE_ID, g::VERSION, digest, g::SIG_HEX));
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, g::VERSION, digest, ""));
    TEST_ASSERT_FALSE(pnex_ota_sig_verify(g::PUBKEY_HEX, g::DEVICE_ID, g::VERSION, digest, "zz"));
}

int main() {
    UNITY_BEGIN();
    RUN_TEST(test_tt_golden_vectors);
    RUN_TEST(test_pid_golden_vectors_replay);
    RUN_TEST(test_relay_golden_vectors);
    RUN_TEST(test_tt_deadband_nan_jamais_on);
    RUN_TEST(test_relay_duty_nan_jamais_on);
    RUN_TEST(test_pid_dt_nul_d_zero);
    RUN_TEST(test_aead_chacha20_poly1305_rfc8439);
    RUN_TEST(test_camera_header_golden_bytes);
    RUN_TEST(test_sha256_fips_vectors);
    RUN_TEST(test_hmac_sha256_rfc4231);
    RUN_TEST(test_noise_golden_handshake_and_frames);
    RUN_TEST(test_noise_refusals);
    RUN_TEST(test_ota_signature_golden);
    return UNITY_END();
}
