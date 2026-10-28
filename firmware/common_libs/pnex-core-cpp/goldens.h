// GENERATED FILE — do not edit by hand. Reference: pnex_core::control
// (crates/pnex-core/src/control.rs). Regen:
//   PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test golden_vectors
// Replayed by firmware/core-cpp-tests (Unity, host) — golden vectors
// Rust = C++ tests (D20 §6, edge-model.md §3 rule 2).
#pragma once

#include <cstddef>

namespace pnex_goldens {

struct TtStep { double measurement; bool current_on; bool expected_on; };
struct TtScenario { const char* name; bool heat; double setpoint; double deadband; const TtStep* steps; size_t n; };
struct PidStep { double dt_secs; double measurement; double expected_duty; double expected_integral; };
struct PidScenario { const char* name; double setpoint; double kp; double ki; double kd; const PidStep* steps; size_t n; };
struct RelayStep { double elapsed_secs; bool expected_on; };
struct RelayScenario { const char* name; double duty_pct; double cycle_time_secs; const RelayStep* steps; size_t n; };

static const TtStep TT_HEATING_HYSTERESIS_STEPS[] = {
    {18.4, false, true},
    {18.6, false, false},
    {18.6, true, true},
    {18.99, true, true},
    {19.0, true, false},
    {18.5, false, false},
    {18.4999, false, true},
};
static const TtStep TT_COOLING_INVERTED_STEPS[] = {
    {26.6, false, true},
    {26.4, false, false},
    {26.1, true, true},
    {26.0, true, false},
    {26.5, false, false},
};
static const TtStep TT_ZERO_DEADBAND_NEVER_TURNS_ON_STEPS[] = {
    {10.0, false, false},
    {10.0, true, false},
};
static const TtStep TT_NEGATIVE_DEADBAND_NEVER_TURNS_ON_STEPS[] = {
    {10.0, false, false},
};
static const TtScenario TT_SCENARIOS[] = {
    {"heating_hysteresis", true, 19.0, 0.5, TT_HEATING_HYSTERESIS_STEPS, 7},
    {"cooling_inverted", false, 26.0, 0.5, TT_COOLING_INVERTED_STEPS, 5},
    {"zero_deadband_never_turns_on", true, 19.0, 0.0, TT_ZERO_DEADBAND_NEVER_TURNS_ON_STEPS, 2},
    {"negative_deadband_never_turns_on", true, 19.0, -1.0, TT_NEGATIVE_DEADBAND_NEVER_TURNS_ON_STEPS, 1},
};

static const PidStep PID_CONVERGENCE_OVERSHOOT_REARM_STEPS[] = {
    {5.0, 18.0, 9.00000000000000000e0, 5.00000000000000000e0},
    {5.0, 18.5, 1.17400000000000002e1, 8.75000000000000000e0},
    {5.0, 18.4, 1.59520000000000071e1, 1.27500000000000036e1},
    {5.0, 25.0, 0.00000000000000000e0, 1.01319999999999997e1},
    {5.0, 25.0, 0.00000000000000000e0, 1.00000000000000000e1},
};
static const PidStep PID_ANTI_WINDUP_HIGH_SATURATION_STEPS[] = {
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
    {1.0, 0.0, 1.00000000000000000e2, 8.00000000000000000e1},
};
static const PidStep PID_INTEGRAL_LOWER_BOUND_STEPS[] = {
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
    {1.0, 100.0, 0.00000000000000000e0, 8.00000000000000000e1},
};
static const PidStep PID_SETPOINT_KICK_ABSENT_STEPS[] = {
    {1.0, 19.0, 1.00000000000000000e0, 0.00000000000000000e0},
    {1.0, 18.0, 7.00000000000000000e0, 0.00000000000000000e0},
};
static const PidStep PID_ZERO_DT_NO_DERIVATIVE_STEPS[] = {
    {1.0, 19.0, 1.00000000000000000e0, 0.00000000000000000e0},
    {0.0, 18.0, 2.00000000000000000e0, 0.00000000000000000e0},
};
static const PidScenario PID_SCENARIOS[] = {
    {"convergence_overshoot_rearm", 20.0, 2.0, 0.5, 0.1, PID_CONVERGENCE_OVERSHOOT_REARM_STEPS, 5},
    {"anti_windup_high_saturation", 20.0, 1.0, 10.0, 0.0, PID_ANTI_WINDUP_HIGH_SATURATION_STEPS, 8},
    {"integral_lower_bound", 20.0, 1.0, 10.0, 0.0, PID_INTEGRAL_LOWER_BOUND_STEPS, 4},
    {"setpoint_kick_absent", 20.0, 1.0, 0.0, 5.0, PID_SETPOINT_KICK_ABSENT_STEPS, 2},
    {"zero_dt_no_derivative", 20.0, 1.0, 0.0, 5.0, PID_ZERO_DT_NO_DERIVATIVE_STEPS, 2},
};

static const RelayStep RELAY_DUTY_25_CYCLE_10_STEPS[] = {
    {0.0, true},
    {2.49, true},
    {2.5, false},
    {9.9, false},
};
static const RelayStep RELAY_DUTY_EDGES_STEPS[] = {
    {0.0, false},
};
static const RelayStep RELAY_DUTY_100_ALWAYS_ON_STEPS[] = {
    {0.0, true},
    {5.0, true},
    {9.99, true},
};
static const RelayStep RELAY_ZERO_CYCLE_NEVER_ON_STEPS[] = {
    {0.0, false},
};
static const RelayStep RELAY_NEGATIVE_ELAPSED_NEVER_ON_STEPS[] = {
    {-0.1, false},
};
static const RelayScenario RELAY_SCENARIOS[] = {
    {"duty_25_cycle_10", 25.0, 10.0, RELAY_DUTY_25_CYCLE_10_STEPS, 4},
    {"duty_edges", 0.0, 10.0, RELAY_DUTY_EDGES_STEPS, 1},
    {"duty_100_always_on", 100.0, 10.0, RELAY_DUTY_100_ALWAYS_ON_STEPS, 3},
    {"zero_cycle_never_on", 50.0, 0.0, RELAY_ZERO_CYCLE_NEVER_ON_STEPS, 1},
    {"negative_elapsed_never_on", 50.0, 10.0, RELAY_NEGATIVE_ELAPSED_NEVER_ON_STEPS, 1},
};

static const size_t TT_N = sizeof(TT_SCENARIOS) / sizeof(TT_SCENARIOS[0]);
static const size_t PID_N = sizeof(PID_SCENARIOS) / sizeof(PID_SCENARIOS[0]);
static const size_t RELAY_N = sizeof(RELAY_SCENARIOS) / sizeof(RELAY_SCENARIOS[0]);

}  // namespace pnex_goldens
