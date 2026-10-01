#include "pnex_control.h"

#include <cmath>

namespace pnex_control {

// Clamp explicite : même sémantique que Rust f64::clamp (NaN en entree ->
// NaN en sortie ; pas d'UB si lo > hi, garanti impossible ici car 100 > 0).
static inline double pnex_clamp(double v, double lo, double hi) {
    if (v < lo) {
        return lo;
    }
    if (v > hi) {
        return hi;
    }
    return v;
}

bool tt_step(bool heat, double setpoint, double deadband, bool current_on,
             double measurement) {
    if (!std::isfinite(deadband) || deadband <= 0.0) {
        return false;
    }
    if (current_on) {
        return heat ? (measurement < setpoint) : (measurement > setpoint);
    }
    return heat ? (measurement < setpoint - deadband)
                : (measurement > setpoint + deadband);
}

double pid_step(double setpoint, double kp, double ki, double kd,
                double measurement, double dt_secs, PidState& state) {
    const double error = setpoint - measurement;
    const double p = kp * error;
    const double d = (state.has_prev && dt_secs > 0.0)
                         ? (-kd) * (measurement - state.prev_measurement) / dt_secs
                         : 0.0;
    state.integral += ki * error * dt_secs;
    const double room_max = 100.0 - (p + d);
    const double room_min = -p - d;
    state.integral = pnex_clamp(state.integral, room_min, room_max);
    state.has_prev = true;
    state.prev_measurement = measurement;
    return pnex_clamp(p + state.integral + d, 0.0, 100.0);
}

bool relay_window(double duty_pct, double cycle_time_secs, double elapsed_secs) {
    if (!std::isfinite(duty_pct) || !std::isfinite(cycle_time_secs) ||
        cycle_time_secs <= 0.0) {
        return false;
    }
    const double on_secs = pnex_clamp(duty_pct, 0.0, 100.0) / 100.0 * cycle_time_secs;
    return elapsed_secs >= 0.0 && elapsed_secs < on_secs;
}

}  // namespace pnex_control
