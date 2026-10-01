//! Math de référence des cartes de régulation — spécification exécutable du
//! contrat que le firmware `regulator` (C++) reproduit à l'identique. Ces
//! fonctions ne tournent jamais en production côté serveur (D13/D17 : la
//! boucle de régulation vit sur le device) ; elles servent de référence
//! partagée + générateur de golden vectors pour la phase firmware.
//!
//! Pur, wasm-safe, zéro dépendance — compilé natif et wasm32.

use serde::{Deserialize, Serialize};

/// Un pas de régulation tout-ou-rien (référence des cartes `reg_tt_*`).
///
/// - chauffage (`heat = true`) : ON quand `mesure < consigne - deadband`,
///   reste ON tant que `mesure < consigne`, OFF au retour à la consigne ;
/// - clim (`heat = false`) : ON quand `mesure > consigne + deadband`, reste
///   ON tant que `mesure > consigne`, OFF au retour à la consigne.
///
/// `deadband` non fini ou ≤ 0 → jamais ON (fail-safe, miroir du refus de
/// config au build du nœud runtime).
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

/// État interne du PID — porté par le device, une instance par régulation
/// (perdu au reboot : re-dérivé au 1er échantillon suivant, cf. doc).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PidState {
    integral: f64,
    prev_measurement: Option<f64>,
}

impl PidState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intégrale courante (diagnostic / persistance device).
    pub fn integral(&self) -> f64 {
        self.integral
    }
}

/// Un pas de régulation PID — retourne le **duty 0..=100 %** de la sortie.
///
/// - dérivée sur la **mesure** (`-kd · Δmesure/dt`) : un changement de
///   consigne ne provoque pas de derivative kick, et `dt_secs ≤ 0` (1er
///   échantillon) dégrade la dérivée à 0 ;
/// - anti-windup par **bornage de l'intégrale** : l'intégrale ne peut jamais
///   pousser la sortie hors [0, 100] au-delà de ce que P+D y mettent déjà —
///   déterministe et trivial à miroiter en C++.
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
    // Plage d'intégrale qui garde P+I+D dans [0, 100] : room_min ≤ i ≤
    // room_max, et room_max ≥ room_min toujours (car 100 > 0) — clamp sûr.
    let room_max = 100.0 - (p + d);
    let room_min = -p - d;
    state.integral = state.integral.clamp(room_min, room_max);
    state.prev_measurement = Some(measurement);
    (p + state.integral + d).clamp(0.0, 100.0)
}

/// Répartition d'un duty % en relais **time-proportional** : ON pendant les
/// `duty_pct` % initiaux du cycle, OFF ensuite. `elapsed_secs` = position
/// dans le cycle courant (0..cycle_time) ; hors cycle → OFF (fail-safe).
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
        // Consigne 19°, deadband 0.5 : ON sous 18.5, OFF au retour à 19.
        let on = tt_step(true, 19.0, 0.5, false, 18.4);
        assert!(on);
        // Entre 18.5 et 19 : reste OFF (bande morte) puis reste ON.
        assert!(!tt_step(true, 19.0, 0.5, false, 18.6));
        assert!(tt_step(true, 19.0, 0.5, true, 18.6));
        assert!(tt_step(true, 19.0, 0.5, true, 18.99));
        // Au retour exact à la consigne : OFF.
        assert!(!tt_step(true, 19.0, 0.5, true, 19.0));
    }

    #[test]
    fn tt_clim_inverse() {
        // Consigne 26°, deadband 0.5 : ON au-dessus de 26.5, OFF au retour ≤ 26.
        assert!(tt_step(false, 26.0, 0.5, false, 26.6));
        assert!(!tt_step(false, 26.0, 0.5, false, 26.4));
        assert!(tt_step(false, 26.0, 0.5, true, 26.1));
        assert!(!tt_step(false, 26.0, 0.5, true, 26.0));
    }

    #[test]
    fn tt_deadband_invalide_ne_s_allume_jamais() {
        // Fail-safe : deadband nul/négatif/non fini → jamais ON.
        assert!(!tt_step(true, 19.0, 0.0, false, 10.0));
        assert!(!tt_step(true, 19.0, -1.0, false, 10.0));
        assert!(!tt_step(false, 19.0, f64::NAN, false, 30.0));
    }

    #[test]
    fn pid_premier_echantillon_et_convergence() {
        let mut st = PidState::new();
        // 1er échantillon : pas de dérivée (prev absent), I démarre.
        let duty = pid_step(20.0, 2.0, 0.5, 0.1, 18.0, 5.0, &mut st);
        // P = 2·2 = 4 ; I = 0.5·2·5 = 5 ; D = 0 → duty = 9.
        assert!((duty - 9.0).abs() < 1e-9, "{duty}");
        assert!((st.integral() - 5.0).abs() < 1e-9);

        // 2e échantillon : la mesure monte (erreur diminue), dérivée sur la
        // mesure = -0.1·(18.5-18)/5 = -0.01.
        let duty2 = pid_step(20.0, 2.0, 0.5, 0.1, 18.5, 5.0, &mut st);
        // P = 3 ; I = 5 + 0.5·1.5·5 = 8.75 ; D = -0.01 → duty ≈ 11.74.
        assert!((duty2 - 11.74).abs() < 1e-9, "{duty2}");
    }

    #[test]
    fn pid_anti_windup_borne_l_integrale() {
        let mut st = PidState::new();
        // Erreur énorme répétée : l'intégrale ne peut pas winduper au-delà
        // de ce qui garde la sortie dans [0, 100].
        for _ in 0..1000 {
            let duty = pid_step(20.0, 1.0, 10.0, 0.0, 0.0, 1.0, &mut st);
            assert!((0.0..=100.0).contains(&duty));
        }
        // P = 20, donc l'intégrale est plafonnée à 100 - 20 = 80 (D = 0).
        assert!((st.integral() - 80.0).abs() < 1e-9, "{}", st.integral());
        assert!((pid_step(20.0, 1.0, 10.0, 0.0, 0.0, 1.0, &mut st) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn pid_derivee_sur_mesure_sans_kick() {
        // Changer la consigne ne fait pas sauter la dérivée : D ne dépend
        // que de la mesure (1er échantillon : D = 0, prev absent).
        let mut st = PidState::new();
        let d1 = pid_step(20.0, 1.0, 0.0, 5.0, 19.0, 1.0, &mut st);
        assert!((d1 - 1.0).abs() < 1e-9, "{d1}"); // P=1, D=0 (1er échantillon)
        let d2 = pid_step(30.0, 1.0, 0.0, 5.0, 18.0, 1.0, &mut st);
        // Δmesure = -1 → D = -5·(-1)/1 = +5 ; P = 12 ; I = 0.
        assert!((d2 - 17.0).abs() < 1e-9, "{d2}");
    }

    #[test]
    fn relay_window_repart_le_duty() {
        // Duty 25 % sur un cycle de 10 s : ON sur [0, 2.5), OFF ensuite.
        assert!(relay_window(25.0, 10.0, 0.0));
        assert!(relay_window(25.0, 10.0, 2.49));
        assert!(!relay_window(25.0, 10.0, 2.5));
        assert!(!relay_window(25.0, 10.0, 9.9));
        // Bords : duty 0 → jamais ON ; duty 100 → toujours ON ; cycle
        // invalide → fail-safe OFF.
        assert!(!relay_window(0.0, 10.0, 0.0));
        assert!(relay_window(100.0, 10.0, 5.0));
        assert!(!relay_window(50.0, 0.0, 0.0));
        assert!(!relay_window(f64::NAN, 10.0, 0.0));
        assert!(!relay_window(50.0, 10.0, -0.1));
    }

    #[test]
    fn pid_state_serialise_pour_diag() {
        // Le device peut rapporter l'intégrale (RegState diag) : roundtrip.
        let mut st = PidState::new();
        pid_step(20.0, 1.0, 1.0, 0.0, 18.0, 2.0, &mut st);
        let json = serde_json::to_string(&st).unwrap();
        let back: PidState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
    }
}
