//! Reference math of the control cards — executable specification of the
//! contract that the `regulator` firmware (C++) reproduces identically. These
//! functions never run in production on the server (D13/D17: the
//! control loop lives on the device); they serve as a shared reference
//! + golden vector generator for the firmware phase.
//!
//! Pure, wasm-safe, zero dependency — compiled native and wasm32.

use serde::{Deserialize, Serialize};

/// One on/off control step (reference of the `reg_tt_*` cards).
///
/// - heating (`heat = true`): ON when `measurement < setpoint - deadband`,
///   stays ON while `measurement < setpoint`, OFF when back at the setpoint;
/// - cooling (`heat = false`): ON when `measurement > setpoint + deadband`, stays
///   ON while `measurement > setpoint`, OFF when back at the setpoint.
///
/// Non-finite or ≤ 0 `deadband` → never ON (fail-safe, mirror of the config
/// refusal when the runtime node is built).
pub fn tt_step(
    heat: bool,
    setpoint: f64,
    deadband: f64,
    current_on: bool,
    measurement: f64,
) -> bool {
    if !(deadband.is_finite() && deadband > 0.0) {
        return false;
    }
    if current_on {
        if heat {
            measurement < setpoint
        } else {
            measurement > setpoint
        }
    } else if heat {
        measurement < setpoint - deadband
    } else {
        measurement > setpoint + deadband
    }
}

/// Internal PID state — carried by the device, one instance per control
/// (lost on reboot: re-derived at the next 1st sample, see doc).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PidState {
    integral: f64,
    prev_measurement: Option<f64>,
}

impl PidState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Current integral (diagnostics / device persistence).
    pub fn integral(&self) -> f64 {
        self.integral
    }
}

/// One PID control step — returns the output's **duty 0..=100 %**.
///
/// - derivative on the **measurement** (`-kd · Δmeasurement/dt`): a setpoint
///   change does not cause a derivative kick, and `dt_secs ≤ 0` (1st
///   sample) degrades the derivative to 0;
/// - anti-windup by **bounding the integral**: the integral can never
///   push the output outside [0, 100] beyond what P+D already put there —
///   deterministic and trivial to mirror in C++.
pub fn pid_step(
    setpoint: f64,
    kp: f64,
    ki: f64,
    kd: f64,
    measurement: f64,
    dt_secs: f64,
    state: &mut PidState,
) -> f64 {
    let error = setpoint - measurement;
    let p = kp * error;
    let d = match state.prev_measurement {
        Some(prev) if dt_secs > 0.0 => -kd * (measurement - prev) / dt_secs,
        _ => 0.0,
    };
    state.integral += ki * error * dt_secs;
    // Integral range that keeps P+I+D within [0, 100]: room_min ≤ i ≤
    // room_max, and room_max ≥ room_min always (since 100 > 0) — safe clamp.
    let room_max = 100.0 - (p + d);
    let room_min = -p - d;
    state.integral = state.integral.clamp(room_min, room_max);
    state.prev_measurement = Some(measurement);
    (p + state.integral + d).clamp(0.0, 100.0)
}

/// Spreads a duty % into a **time-proportional** relay: ON during the
/// first `duty_pct` % of the cycle, OFF afterwards. `elapsed_secs` = position
/// in the current cycle (0..cycle_time); outside the cycle → OFF (fail-safe).
pub fn relay_window(duty_pct: f64, cycle_time_secs: f64, elapsed_secs: f64) -> bool {
    if !(duty_pct.is_finite() && cycle_time_secs.is_finite()) || cycle_time_secs <= 0.0 {
        return false;
    }
    elapsed_secs >= 0.0 && elapsed_secs < duty_pct.clamp(0.0, 100.0) / 100.0 * cycle_time_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tt_chauffage_hysteresis_asymetrique() {
        // Setpoint 19°, deadband 0.5: ON below 18.5, OFF when back at 19.
        let on = tt_step(true, 19.0, 0.5, false, 18.4);
        assert!(on);
        // Between 18.5 and 19: stays OFF (dead band) then stays ON.
        assert!(!tt_step(true, 19.0, 0.5, false, 18.6));
        assert!(tt_step(true, 19.0, 0.5, true, 18.6));
        assert!(tt_step(true, 19.0, 0.5, true, 18.99));
        // On exact return to the setpoint: OFF.
        assert!(!tt_step(true, 19.0, 0.5, true, 19.0));
    }

    #[test]
    fn tt_clim_inverse() {
        // Setpoint 26°, deadband 0.5: ON above 26.5, OFF when back ≤ 26.
        assert!(tt_step(false, 26.0, 0.5, false, 26.6));
        assert!(!tt_step(false, 26.0, 0.5, false, 26.4));
        assert!(tt_step(false, 26.0, 0.5, true, 26.1));
        assert!(!tt_step(false, 26.0, 0.5, true, 26.0));
    }

    #[test]
    fn tt_deadband_invalide_ne_s_allume_jamais() {
        // Fail-safe: zero/negative/non-finite deadband → never ON.
        assert!(!tt_step(true, 19.0, 0.0, false, 10.0));
        assert!(!tt_step(true, 19.0, -1.0, false, 10.0));
        assert!(!tt_step(false, 19.0, f64::NAN, false, 30.0));
    }

    #[test]
    fn pid_premier_echantillon_et_convergence() {
        let mut st = PidState::new();
        // 1st sample: no derivative (prev absent), I starts.
        let duty = pid_step(20.0, 2.0, 0.5, 0.1, 18.0, 5.0, &mut st);
        // P = 2·2 = 4 ; I = 0.5·2·5 = 5 ; D = 0 → duty = 9.
        assert!((duty - 9.0).abs() < 1e-9, "{duty}");
        assert!((st.integral() - 5.0).abs() < 1e-9);

        // 2nd sample: the measurement rises (error decreases), derivative on the
        // measurement = -0.1·(18.5-18)/5 = -0.01.
        let duty2 = pid_step(20.0, 2.0, 0.5, 0.1, 18.5, 5.0, &mut st);
        // P = 3 ; I = 5 + 0.5·1.5·5 = 8.75 ; D = -0.01 → duty ≈ 11.74.
        assert!((duty2 - 11.74).abs() < 1e-9, "{duty2}");
    }

    #[test]
    fn pid_anti_windup_borne_l_integrale() {
        let mut st = PidState::new();
        // Repeated huge error: the integral cannot wind up beyond
        // what keeps the output within [0, 100].
        for _ in 0..1000 {
            let duty = pid_step(20.0, 1.0, 10.0, 0.0, 0.0, 1.0, &mut st);
            assert!((0.0..=100.0).contains(&duty));
        }
        // P = 20, so the integral is capped at 100 - 20 = 80 (D = 0).
        assert!((st.integral() - 80.0).abs() < 1e-9, "{}", st.integral());
        assert!((pid_step(20.0, 1.0, 10.0, 0.0, 0.0, 1.0, &mut st) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn pid_derivee_sur_mesure_sans_kick() {
        // Changing the setpoint does not make the derivative jump: D depends
        // only on the measurement (1st sample: D = 0, prev absent).
        let mut st = PidState::new();
        let d1 = pid_step(20.0, 1.0, 0.0, 5.0, 19.0, 1.0, &mut st);
        assert!((d1 - 1.0).abs() < 1e-9, "{d1}"); // P=1, D=0 (1st sample)
        let d2 = pid_step(30.0, 1.0, 0.0, 5.0, 18.0, 1.0, &mut st);
        // Δmesure = -1 → D = -5·(-1)/1 = +5 ; P = 12 ; I = 0.
        assert!((d2 - 17.0).abs() < 1e-9, "{d2}");
    }

    #[test]
    fn relay_window_repart_le_duty() {
        // Duty 25 % over a 10 s cycle: ON on [0, 2.5), OFF afterwards.
        assert!(relay_window(25.0, 10.0, 0.0));
        assert!(relay_window(25.0, 10.0, 2.49));
        assert!(!relay_window(25.0, 10.0, 2.5));
        assert!(!relay_window(25.0, 10.0, 9.9));
        // Edges: duty 0 → never ON; duty 100 → always ON; invalid
        // cycle → fail-safe OFF.
        assert!(!relay_window(0.0, 10.0, 0.0));
        assert!(relay_window(100.0, 10.0, 5.0));
        assert!(!relay_window(50.0, 0.0, 0.0));
        assert!(!relay_window(f64::NAN, 10.0, 0.0));
        assert!(!relay_window(50.0, 10.0, -0.1));
    }

    #[test]
    fn pid_state_serialise_pour_diag() {
        // The device can report the integral (RegState diag): roundtrip.
        let mut st = PidState::new();
        pid_step(20.0, 1.0, 1.0, 0.0, 18.0, 2.0, &mut st);
        let json = serde_json::to_string(&st).unwrap();
        let back: PidState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
    }
}
