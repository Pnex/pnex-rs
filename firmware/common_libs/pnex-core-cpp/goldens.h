// FICHIER GÉNÉRÉ — ne pas éditer à la main. Référence : pnex_core::control
// (crates/pnex-core/src/control.rs). Regen :
//   PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test golden_vectors
// Rejoué par firmware/core-cpp-tests (Unity, hôte) — golden vectors
// Rust = tests C++ (D20 §6, edge-model.md §3 règle 2).
#pragma once

#include <cstddef>

namespace pnex_goldens {

struct TtStep { double measurement; bool current_on; bool expected_on; };
struct TtScenario { const char* name; bool heat; double setpoint; double deadband; const TtStep* steps; size_t n; };
struct PidStep { double dt_secs; double measurement; double expected_duty; double expected_integral; };
struct PidScenario { const char* name; double setpoint; double kp; double ki; double kd; const PidStep* steps; size_t n; };
struct RelayStep { double elapsed_secs; bool expected_on; };
struct RelayScenario { const char* name; double duty_pct; double cycle_time_secs; const RelayStep* steps; size_t n; };

static const TtStep TT_CHAUFFAGE_HYSTERESIS_STEPS[] = {
    {18.4, false, true},
    {18.6, false, false},
    {18.6, true, true},
    {18.99, true, true},
    {19.0, true, false},
    {18.5, false, false},
    {18.4999, false, true},
};
static const TtStep TT_CLIM_INVERSEE_STEPS[] = {
    {26.6, false, true},
    {26.4, false, false},
    {26.1, true, true},
    {26.0, true, false},
    {26.5, false, false},
};
static const TtStep TT_DEADBAND_ZERO_NE_S_ALLUME_JAMAIS_STEPS[] = {
    {10.0, false, false},
    {10.0, true, false},
};
static const TtStep TT_DEADBAND_NEGATIF_NE_S_ALLUME_JAMAIS_STEPS[] = {
    {10.0, false, false},
};
static const TtScenario TT_SCENARIOS[] = {
    {"chauffage_hysteresis", true, 19.0, 0.5, TT_CHAUFFAGE_HYSTERESIS_STEPS, 7},
    {"clim_inversee", false, 26.0, 0.5, TT_CLIM_INVERSEE_STEPS, 5},
    {"deadband_zero_ne_s_allume_jamais", true, 19.0, 0.0, TT_DEADBAND_ZERO_NE_S_ALLUME_JAMAIS_STEPS, 2},
    {"deadband_negatif_ne_s_allume_jamais", true, 19.0, -1.0, TT_DEADBAND_NEGATIF_NE_S_ALLUME_JAMAIS_STEPS, 1},
};

static const PidStep PID_CONVERGENCE_OVERSHOOT_RE_ARME_STEPS[] = {
    {5.0, 18.0, 9.00000000000000000e0, 5.00000000000000000e0},
    {5.0, 18.5, 1.17400000000000002e1, 8.75000000000000000e0},
    {5.0, 18.4, 1.59520000000000071e1, 1.27500000000000036e1},
    {5.0, 25.0, 0.00000000000000000e0, 1.01319999999999997e1},
    {5.0, 25.0, 0.00000000000000000e0, 1.00000000000000000e1},
};
static const PidStep PID_ANTI_WINDUP_SATURATION_HAUTE_STEPS[] = {
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
};
static const PidStep PID_BORNE_BASSE_INTEGRALE_STEPS[] = {
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
};
static const PidStep PID_KICK_DE_CONSIGNE_ABSENT_STEPS[] = {
    {1.0, 19.0, 1.00000000000000000e0, 0.00000000000000000e0},
    {1.0, 18.0, 7.00000000000000000e0, 0.00000000000000000e0},
};
static const PidStep PID_DT_NUL_SANS_DERIVEE_STEPS[] = {
    {1.0, 19.0, 1.00000000000000000e0, 0.00000000000000000e0},
    {0.0, 18.0, 2.00000000000000000e0, 0.00000000000000000e0},
};
static const PidScenario PID_SCENARIOS[] = {
    {"convergence_overshoot_re_arme", 20.0, 2.0, 0.5, 0.1, PID_CONVERGENCE_OVERSHOOT_RE_ARME_STEPS, 5},
    {"anti_windup_saturation_haute", 20.0, 1.0, 10.0, 0.0, PID_ANTI_WINDUP_SATURATION_HAUTE_STEPS, 8},
    {"borne_basse_integrale", 20.0, 1.0, 10.0, 0.0, PID_BORNE_BASSE_INTEGRALE_STEPS, 4},
    {"kick_de_consigne_absent", 20.0, 1.0, 0.0, 5.0, PID_KICK_DE_CONSIGNE_ABSENT_STEPS, 2},
    {"dt_nul_sans_derivee", 20.0, 1.0, 0.0, 5.0, PID_DT_NUL_SANS_DERIVEE_STEPS, 2},
};

static const RelayStep RELAY_DUTY_25_CYCLE_10_STEPS[] = {
    {0.0, true},
    {2.49, true},
    {2.5, false},
    {9.9, false},
};
static const RelayStep RELAY_BORDS_DUTY_STEPS[] = {
    {0.0, false},
};
static const RelayStep RELAY_DUTY_100_TOUJOURS_ON_STEPS[] = {
    {0.0, true},
    {5.0, true},
    {9.99, true},
};
static const RelayStep RELAY_CYCLE_NUL_JAMAIS_ON_STEPS[] = {
    {0.0, false},
};
static const RelayStep RELAY_ELAPSED_NEGATIF_JAMAIS_ON_STEPS[] = {
    {-0.1, false},
};
static const RelayScenario RELAY_SCENARIOS[] = {
    {"duty_25_cycle_10", 25.0, 10.0, RELAY_DUTY_25_CYCLE_10_STEPS, 4},
    {"bords_duty", 0.0, 10.0, RELAY_BORDS_DUTY_STEPS, 1},
    {"duty_100_toujours_on", 100.0, 10.0, RELAY_DUTY_100_TOUJOURS_ON_STEPS, 3},
    {"cycle_nul_jamais_on", 50.0, 0.0, RELAY_CYCLE_NUL_JAMAIS_ON_STEPS, 1},
    {"elapsed_negatif_jamais_on", 50.0, 10.0, RELAY_ELAPSED_NEGATIF_JAMAIS_ON_STEPS, 1},
};

static const size_t TT_N = sizeof(TT_SCENARIOS) / sizeof(TT_SCENARIOS[0]);
static const size_t PID_N = sizeof(PID_SCENARIOS) / sizeof(PID_SCENARIOS[0]);
static const size_t RELAY_N = sizeof(RELAY_SCENARIOS) / sizeof(RELAY_SCENARIOS[0]);

}  // namespace pnex_goldens
