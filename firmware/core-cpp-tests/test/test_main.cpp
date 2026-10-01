#include <cmath>
#include <limits>
#include <stddef.h>

#include <unity.h>

#include "pnex_control.h"
#include "goldens.h"
#include "chacha20_rfc7539.h"
#include "pnex_camera_frame.h"

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

// Vecteurs RFC 7539 §2.3.2 — bloc de keystream : key = 00..1f,
// nonce = 000000090000004a00000000, compteur 1. C'est la référence
// partagée serveur (RustCrypto) ↔ firmware (BearSSL 8266 / header vendu
// ESP32) : toute divergence casse le chiffrement des frames en e2e ET
// cette CI.
void test_chacha20_bloc_keystream_rfc7539(void) {
    const uint8_t key[32] = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                             0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
                             0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                             0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f};
    const uint8_t nonce[12] = {0x00, 0x00, 0x00, 0x09, 0x00, 0x00,
                               0x00, 0x4a, 0x00, 0x00, 0x00, 0x00};
    static const uint8_t EXPECTED[64] = {
        0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20, 0x71, 0xc4,
        0xc7, 0xd1, 0xf4, 0xc7, 0x33, 0xc0, 0x68, 0x03, 0x04, 0x22, 0xaa, 0x9a, 0xc3, 0xd4, 0x6c, 0x4e,
        0xd2, 0x82, 0x64, 0x46, 0x07, 0x9f, 0xaa, 0x09, 0x14, 0xc2, 0xd7, 0x05, 0xd9, 0x8b, 0x02, 0xa2,
        0xb5, 0x12, 0x9c, 0xd1, 0xde, 0x16, 0x4e, 0xb9, 0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e};
    uint8_t blk[64];
    pnex_crypto::chacha20_block(key, 1, nonce, blk);
    TEST_ASSERT_EQUAL_UINT8_ARRAY(EXPECTED, blk, 64);
}

// Vecteur RFC 7539 §2.4.2 — chiffrement complet (compteur 1) : XOR du
// keystream, l'inverse du test bloc.
void test_chacha20_chiffrement_rfc7539(void) {
    const uint8_t key[32] = {0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                             0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
                             0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17,
                             0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f};
    const uint8_t nonce[12] = {0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                               0x00, 0x4a, 0x00, 0x00, 0x00, 0x00};
    const char* PT =
        "Ladies and Gentlemen of the class of '99: If I could offer you "
        "only one tip for the future, sunscreen would be it.";
    static const uint8_t EXPECTED[114] = {
        0x6e, 0x2e, 0x35, 0x9a, 0x25, 0x68, 0xf9, 0x80, 0x41, 0xba, 0x07, 0x28, 0xdd, 0x0d, 0x69, 0x81,
        0xe9, 0x7e, 0x7a, 0xec, 0x1d, 0x43, 0x60, 0xc2, 0x0a, 0x27, 0xaf, 0xcc, 0xfd, 0x9f, 0xae, 0x0b,
        0xf9, 0x1b, 0x65, 0xc5, 0x52, 0x47, 0x33, 0xab, 0x8f, 0x59, 0x3d, 0xab, 0xcd, 0x62, 0xb3, 0x57,
        0x16, 0x39, 0xd6, 0x24, 0xe6, 0x51, 0x52, 0xab, 0x8f, 0x53, 0x0c, 0x35, 0x9f, 0x08, 0x61, 0xd8,
        0x07, 0xca, 0x0d, 0xbf, 0x50, 0x0d, 0x6a, 0x61, 0x56, 0xa3, 0x8e, 0x08, 0x8a, 0x22, 0xb6, 0x5e,
        0x52, 0xbc, 0x51, 0x4d, 0x16, 0xcc, 0xf8, 0x06, 0x81, 0x8c, 0xe9, 0x1a, 0xb7, 0x79, 0x37, 0x36,
        0x5a, 0xf9, 0x0b, 0xbf, 0x74, 0xa3, 0x5b, 0xe6, 0xb4, 0x0b, 0x8e, 0xed, 0xf2, 0x78, 0x5e, 0x42,
        0x87, 0x4d};
    uint8_t buf[114];
    memcpy(buf, PT, sizeof(buf));
    pnex_crypto::chacha20_xor(key, 1, nonce, buf, sizeof(buf));
    TEST_ASSERT_EQUAL_UINT8_ARRAY(EXPECTED, buf, 114);
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

int main() {
    UNITY_BEGIN();
    RUN_TEST(test_tt_golden_vectors);
    RUN_TEST(test_pid_golden_vectors_replay);
    RUN_TEST(test_relay_golden_vectors);
    RUN_TEST(test_tt_deadband_nan_jamais_on);
    RUN_TEST(test_relay_duty_nan_jamais_on);
    RUN_TEST(test_pid_dt_nul_d_zero);
    RUN_TEST(test_chacha20_bloc_keystream_rfc7539);
    RUN_TEST(test_chacha20_chiffrement_rfc7539);
    RUN_TEST(test_camera_header_golden_bytes);
    return UNITY_END();
}
