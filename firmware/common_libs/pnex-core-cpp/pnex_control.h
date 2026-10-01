//
// pnex-core-cpp — miroir C++ de `pnex_core::control` (edge-model.md §3
// règle 2, D20 §6) : la référence mathématique des cartes de régulation,
// reproduite à l'identique côté firmware. Golden vectors Rust = tests C++
// : `firmware/core-cpp-tests` rejoue les vecteurs générés par
// `crates/pnex-core/tests/golden_vectors.rs` (artefacts générés dans
// goldens.h — ne pas éditer à la main).
//
// Pur, sans dépendance (ni ArduinoJson, ni WS) — ces fonctions ne touchent
// jamais le fil ni le hardware ; elles ne font que calculer. Convention
// testée : comparaison à 1e-9 près (les doubles IEEE 754 des deux côtés,
// mêmes opérations → mêmes résultats ; l'epsilon n'est qu'un filet de
// sécurité de portabilité compilateur).
//
#ifndef PNEX_CONTROL_H
#define PNEX_CONTROL_H

namespace pnex_control {

// État interne du PID — porté par le device, une instance par régulation
// (perdu au reboot : re-dérivé au 1er échantillon suivant). Miroir de
// `pnex_core::control::PidState` (Option<f64> → bool + double).
struct PidState {
    double integral = 0.0;
    bool has_prev = false;
    double prev_measurement = 0.0;
};

// Un pas de régulation tout-ou-rien (référence des cartes reg_tt_*).
//
// - chauffage (heat=true) : ON quand mesure < consigne - deadband, reste ON
//   tant que mesure < consigne, OFF au retour à la consigne ;
// - clim (heat=false) : ON quand mesure > consigne + deadband, reste ON
//   tant que mesure > consigne, OFF au retour à la consigne.
//
// deadband non fini ou <= 0 → jamais ON (fail-safe).
bool tt_step(bool heat, double setpoint, double deadband, bool current_on, double measurement);

// Un pas de régulation PID — retourne le duty 0..=100 % de la sortie.
//
// - dérivée sur la mesure (-kd · Δmesure/dt) : pas de derivative kick au
//   changement de consigne ; dt <= 0 ou premier échantillon → D = 0 ;
// - anti-windup par bornage de l'intégrale (l'intégrale ne peut jamais
//   pousser la sortie hors [0, 100] au-delà de P+D).
double pid_step(double setpoint, double kp, double ki, double kd,
                double measurement, double dt_secs, PidState& state);

// Répartition d'un duty % en relais time-proportional : ON pendant les
// duty_pct % initiaux du cycle, OFF ensuite. elapsed_secs = position dans
// le cycle courant (0..cycle_time) ; hors cycle/invalide → OFF (fail-safe).
bool relay_window(double duty_pct, double cycle_time_secs, double elapsed_secs);

}  // namespace pnex_control

#endif  // PNEX_CONTROL_H
