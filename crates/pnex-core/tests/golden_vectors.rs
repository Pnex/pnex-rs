//! Golden vectors Rust = C++ (D20 §6, edge-model.md §3 règle 2).
//!
//! `pnex_core::control` est la **référence** (spécification exécutable des
//! cartes de régulation) ; ce test rejoue des scénarios figés et vérifie
//! que les deux artefacts générés sont à jour :
//!
//! - `crates/pnex-core/goldens/golden_vectors.json` — données de référence ;
//! - `firmware/common_libs/pnex-core-cpp/goldens.h` — mêmes données en
//!   tableaux C++, rejouées par `firmware/core-cpp-tests` (Unity, hôte).
//!
//! Regen : `PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test golden_vectors`
//! (les valeurs non finies — NaN/Inf — sont exclues des vecteurs : le
//! fail-safe NaN est testé nativement de chaque côté, le JSON ne les
//! transporte pas).

use std::path::PathBuf;
use std::sync::Mutex;

use pnex_core::{pid_step, relay_window, tt_step};

// ───────────────────────── Scénarios figés ─────────────────────────

struct TtScenario {
    name: &'static str,
    heat: bool,
    setpoint: f64,
    deadband: f64,
    /// (measurement, current_on) — expected calculé par la référence.
    steps: &'static [(f64, bool)],
}

struct PidScenario {
    name: &'static str,
    setpoint: f64,
    kp: f64,
    ki: f64,
    kd: f64,
    /// (dt_secs, measurement)
    steps: &'static [(f64, f64)],
}

struct RelayScenario {
    name: &'static str,
    duty_pct: f64,
    cycle_time_secs: f64,
    /// elapsed_secs
    steps: &'static [f64],
}

const TT: &[TtScenario] = &[
    // Hystérésis asymétrique : ON sous consigne − deadband, OFF au retour à
    // la consigne ; frontières exclues incluses.
    TtScenario {
        name: "chauffage_hysteresis",
        heat: true,
        setpoint: 19.0,
        deadband: 0.5,
        steps: &[
            (18.4, false), // < 18.5 → ON
            (18.6, false), // bande morte → OFF
            (18.6, true),  // < 19 → reste ON
            (18.99, true),
            (19.0, true),     // retour exact à la consigne → OFF
            (18.5, false),    // frontière : 18.5 < 18.5 est faux → OFF
            (18.4999, false), // juste sous la frontière → ON
        ],
    },
    // Clim : sens inversé, frontière 26.5 exclue.
    TtScenario {
        name: "clim_inversee",
        heat: false,
        setpoint: 26.0,
        deadband: 0.5,
        steps: &[
            (26.6, false),
            (26.4, false),
            (26.1, true),
            (26.0, true),
            (26.5, false), // 26.5 > 26.5 est faux → OFF
        ],
    },
    // Fail-safe : deadband ≤ 0 → jamais ON.
    TtScenario {
        name: "deadband_zero_ne_s_allume_jamais",
        heat: true,
        setpoint: 19.0,
        deadband: 0.0,
        steps: &[(10.0, false), (10.0, true)],
    },
    TtScenario {
        name: "deadband_negatif_ne_s_allume_jamais",
        heat: true,
        setpoint: 19.0,
        deadband: -1.0,
        steps: &[(10.0, false)],
    },
];

const PID: &[PidScenario] = &[
    // 1er échantillon (D=0), convergence, overshoot (duty 0, intégrale
    // bornée bas), puis ré-arme par room_min.
    PidScenario {
        name: "convergence_overshoot_re_arme",
        setpoint: 20.0,
        kp: 2.0,
        ki: 0.5,
        kd: 0.1,
        steps: &[
            (5.0, 18.0),
            (5.0, 18.5),
            (5.0, 18.4),
            (5.0, 25.0),
            (5.0, 25.0),
        ],
    },
    // Anti-windup : intégrale plafonnée à room_max = 100 − P.
    PidScenario {
        name: "anti_windup_saturation_haute",
        setpoint: 20.0,
        kp: 1.0,
        ki: 10.0,
        kd: 0.0,
        steps: &[(1.0, 0.0); 8],
    },
    // Intégrale bornée bas : room_min = −P.
    PidScenario {
        name: "borne_basse_integrale",
        setpoint: 20.0,
        kp: 1.0,
        ki: 10.0,
        kd: 0.0,
        steps: &[(1.0, 100.0); 4],
    },
    // Dérivée sur la mesure : changer la consigne ne kick pas la sortie.
    PidScenario {
        name: "kick_de_consigne_absent",
        setpoint: 20.0,
        kp: 1.0,
        ki: 0.0,
        kd: 5.0,
        steps: &[(1.0, 19.0), (1.0, 18.0)],
    },
    // dt ≤ 0 : dérivée dégradée à 0 même avec un prev.
    PidScenario {
        name: "dt_nul_sans_derivee",
        setpoint: 20.0,
        kp: 1.0,
        ki: 0.0,
        kd: 5.0,
        steps: &[(1.0, 19.0), (0.0, 18.0)],
    },
];

const RELAY: &[RelayScenario] = &[
    RelayScenario {
        name: "duty_25_cycle_10",
        duty_pct: 25.0,
        cycle_time_secs: 10.0,
        steps: &[0.0, 2.49, 2.5, 9.9],
    },
    RelayScenario {
        name: "bords_duty",
        duty_pct: 0.0,
        cycle_time_secs: 10.0,
        steps: &[0.0],
    },
    RelayScenario {
        name: "duty_100_toujours_on",
        duty_pct: 100.0,
        cycle_time_secs: 10.0,
        steps: &[0.0, 5.0, 9.99],
    },
    RelayScenario {
        name: "cycle_nul_jamais_on",
        duty_pct: 50.0,
        cycle_time_secs: 0.0,
        steps: &[0.0],
    },
    RelayScenario {
        name: "elapsed_negatif_jamais_on",
        duty_pct: 50.0,
        cycle_time_secs: 10.0,
        steps: &[-0.1],
    },
];

// ─────────────────── Exécution par la référence ───────────────────

struct TtOut {
    measurement: f64,
    current_on: bool,
    expected_on: bool,
}

struct PidOut {
    dt_secs: f64,
    measurement: f64,
    expected_duty: f64,
    expected_integral: f64,
}

struct RelayOut {
    elapsed_secs: f64,
    expected_on: bool,
}

fn run_tt() -> Vec<(&'static str, bool, f64, f64, Vec<TtOut>)> {
    TT.iter()
        .map(|s| {
            let outs = s
                .steps
                .iter()
                .map(|&(m, on)| {
                    let expected = tt_step(s.heat, s.setpoint, s.deadband, on, m);
                    TtOut {
                        measurement: m,
                        current_on: on,
                        expected_on: expected,
                    }
                })
                .collect();
            (s.name, s.heat, s.setpoint, s.deadband, outs)
        })
        .collect()
}

type PidSamples = Vec<(&'static str, f64, f64, f64, f64, Vec<PidOut>)>;

fn run_pid() -> PidSamples {
    PID.iter()
        .map(|s| {
            let mut st = pnex_core::PidState::new();
            let outs = s
                .steps
                .iter()
                .map(|&(dt, m)| {
                    let duty = pid_step(s.setpoint, s.kp, s.ki, s.kd, m, dt, &mut st);
                    PidOut {
                        dt_secs: dt,
                        measurement: m,
                        expected_duty: duty,
                        expected_integral: st.integral(),
                    }
                })
                .collect();
            (s.name, s.setpoint, s.kp, s.ki, s.kd, outs)
        })
        .collect()
}

fn run_relay() -> Vec<(&'static str, f64, f64, Vec<RelayOut>)> {
    RELAY
        .iter()
        .map(|s| {
            let outs = s
                .steps
                .iter()
                .map(|&e| RelayOut {
                    elapsed_secs: e,
                    expected_on: relay_window(s.duty_pct, s.cycle_time_secs, e),
                })
                .collect();
            (s.name, s.duty_pct, s.cycle_time_secs, outs)
        })
        .collect()
}

// ─────────────────────── Rendu des artefacts ───────────────────────

/// Format C++ d'un double : représentation shortest-roundtrip de Rust
/// ({:?}) — littéral double C++ valide ; NaN/Inf exclus des vecteurs.
fn cpp_f64(v: f64) -> String {
    assert!(v.is_finite(), "les vecteurs ne transportent pas NaN/Inf");
    format!("{v:?}")
}

fn render_json() -> String {
    let mut s = String::from("{\n");
    s.push_str("  \"reference\": \"pnex_core::control\",\n");
    s.push_str("  \"tt\": [\n");
    for (i, (name, heat, sp, db, outs)) in run_tt().iter().enumerate() {
        s.push_str(&format!(
            "    {{\"name\": \"{}\", \"heat\": {}, \"setpoint\": {}, \"deadband\": {}, \"steps\": [\n",
            name, heat, sp, db
        ));
        for (j, o) in outs.iter().enumerate() {
            s.push_str(&format!(
                "      {{\"measurement\": {}, \"current_on\": {}, \"expected_on\": {}}}{}\n",
                o.measurement,
                o.current_on,
                o.expected_on,
                if j + 1 < outs.len() { "," } else { "" }
            ));
        }
        s.push_str(&format!(
            "    ]}}{}\n",
            if i + 1 < run_tt().len() { "," } else { "" }
        ));
    }
    s.push_str("  ],\n");
    s.push_str("  \"pid\": [\n");
    for (i, (name, sp, kp, ki, kd, outs)) in run_pid().iter().enumerate() {
        s.push_str(&format!(
            "    {{\"name\": \"{}\", \"setpoint\": {}, \"kp\": {}, \"ki\": {}, \"kd\": {}, \"steps\": [\n",
            name, sp, kp, ki, kd
        ));
        for (j, o) in outs.iter().enumerate() {
            s.push_str(&format!(
                "      {{\"dt_secs\": {}, \"measurement\": {}, \"expected_duty\": {:.17e}, \"expected_integral\": {:.17e}}}{}\n",
                o.dt_secs,
                o.measurement,
                o.expected_duty,
                o.expected_integral,
                if j + 1 < outs.len() { "," } else { "" }
            ));
        }
        s.push_str(&format!(
            "    ]}}{}\n",
            if i + 1 < run_pid().len() { "," } else { "" }
        ));
    }
    s.push_str("  ],\n");
    s.push_str("  \"relay\": [\n");
    for (i, (name, duty, cycle, outs)) in run_relay().iter().enumerate() {
        s.push_str(&format!(
            "    {{\"name\": \"{}\", \"duty_pct\": {}, \"cycle_time_secs\": {}, \"steps\": [\n",
            name, duty, cycle
        ));
        for (j, o) in outs.iter().enumerate() {
            s.push_str(&format!(
                "      {{\"elapsed_secs\": {}, \"expected_on\": {}}}{}\n",
                o.elapsed_secs,
                o.expected_on,
                if j + 1 < outs.len() { "," } else { "" }
            ));
        }
        s.push_str(&format!(
            "    ]}}{}\n",
            if i + 1 < run_relay().len() { "," } else { "" }
        ));
    }
    s.push_str("  ]\n}\n");
    s
}

fn render_cpp_header() -> String {
    let mut s = String::from(
        "// FICHIER GÉNÉRÉ — ne pas éditer à la main. Référence : pnex_core::control\n\
         // (crates/pnex-core/src/control.rs). Regen :\n\
         //   PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test golden_vectors\n\
         // Rejoué par firmware/core-cpp-tests (Unity, hôte) — golden vectors\n\
         // Rust = tests C++ (D20 §6, edge-model.md §3 règle 2).\n\
         #pragma once\n\n\
         #include <cstddef>\n\n\
         namespace pnex_goldens {\n\n",
    );
    s.push_str("struct TtStep { double measurement; bool current_on; bool expected_on; };\n");
    s.push_str("struct TtScenario { const char* name; bool heat; double setpoint; double deadband; const TtStep* steps; size_t n; };\n");
    s.push_str("struct PidStep { double dt_secs; double measurement; double expected_duty; double expected_integral; };\n");
    s.push_str("struct PidScenario { const char* name; double setpoint; double kp; double ki; double kd; const PidStep* steps; size_t n; };\n");
    s.push_str("struct RelayStep { double elapsed_secs; bool expected_on; };\n");
    s.push_str("struct RelayScenario { const char* name; double duty_pct; double cycle_time_secs; const RelayStep* steps; size_t n; };\n\n");

    let tt = run_tt();
    for (name, _heat, _sp, _db, outs) in &tt {
        s.push_str(&format!(
            "static const TtStep TT_{}_STEPS[] = {{\n",
            name.to_uppercase()
        ));
        for o in outs {
            s.push_str(&format!(
                "    {{{}, {}, {}}},\n",
                cpp_f64(o.measurement),
                o.current_on,
                o.expected_on
            ));
        }
        s.push_str("};\n");
    }
    s.push_str("static const TtScenario TT_SCENARIOS[] = {\n");
    for (name, heat, sp, db, outs) in &tt {
        s.push_str(&format!(
            "    {{\"{name}\", {heat}, {}, {}, TT_{}_STEPS, {}}},\n",
            cpp_f64(*sp),
            cpp_f64(*db),
            name.to_uppercase(),
            outs.len()
        ));
    }
    s.push_str("};\n\n");

    let pid = run_pid();
    for (name, _sp, _kp, _ki, _kd, outs) in &pid {
        s.push_str(&format!(
            "static const PidStep PID_{}_STEPS[] = {{\n",
            name.to_uppercase()
        ));
        for o in outs {
            s.push_str(&format!(
                "    {{{}, {}, {:.17e}, {:.17e}}},\n",
                cpp_f64(o.dt_secs),
                cpp_f64(o.measurement),
                o.expected_duty,
                o.expected_integral
            ));
        }
        s.push_str("};\n");
    }
    s.push_str("static const PidScenario PID_SCENARIOS[] = {\n");
    for (name, sp, kp, ki, kd, outs) in &pid {
        s.push_str(&format!(
            "    {{\"{name}\", {}, {}, {}, {}, PID_{}_STEPS, {}}},\n",
            cpp_f64(*sp),
            cpp_f64(*kp),
            cpp_f64(*ki),
            cpp_f64(*kd),
            name.to_uppercase(),
            outs.len()
        ));
    }
    s.push_str("};\n\n");

    let relay = run_relay();
    for (name, _duty, _cycle, outs) in &relay {
        s.push_str(&format!(
            "static const RelayStep RELAY_{}_STEPS[] = {{\n",
            name.to_uppercase()
        ));
        for o in outs {
            s.push_str(&format!(
                "    {{{}, {}}},\n",
                cpp_f64(o.elapsed_secs),
                o.expected_on
            ));
        }
        s.push_str("};\n");
    }
    s.push_str("static const RelayScenario RELAY_SCENARIOS[] = {\n");
    for (name, duty, cycle, outs) in &relay {
        s.push_str(&format!(
            "    {{\"{name}\", {}, {}, RELAY_{}_STEPS, {}}},\n",
            cpp_f64(*duty),
            cpp_f64(*cycle),
            name.to_uppercase(),
            outs.len()
        ));
    }
    s.push_str("};\n\n");

    s.push_str("static const size_t TT_N = sizeof(TT_SCENARIOS) / sizeof(TT_SCENARIOS[0]);\n");
    s.push_str("static const size_t PID_N = sizeof(PID_SCENARIOS) / sizeof(PID_SCENARIOS[0]);\n");
    s.push_str(
        "static const size_t RELAY_N = sizeof(RELAY_SCENARIOS) / sizeof(RELAY_SCENARIOS[0]);\n",
    );
    s.push_str("\n}  // namespace pnex_goldens\n");
    s
}

// ─────────────────────────── Test gardien ───────────────────────────

static REGEN_LOCK: Mutex<()> = Mutex::new(());

fn golden_paths() -> (PathBuf, PathBuf) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    (
        manifest.join("goldens/golden_vectors.json"),
        // Le header C++ vit près du miroir ; monter d'un cran = racine repo.
        manifest
            .join("../../firmware/common_libs/pnex-core-cpp/goldens.h")
            .canonicalize()
            .unwrap_or_else(|_| {
                manifest.join("../../firmware/common_libs/pnex-core-cpp/goldens.h")
            }),
    )
}

/// Garde-fou : les artefacts générés correspondent exactement à la
/// référence courante. `PNEX_REGEN_GOLDENS=1` réécrit les deux fichiers.
#[test]
fn golden_vectors_sont_a_jour() {
    let _guard = REGEN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (json_path, cpp_path) = golden_paths();
    if std::env::var("PNEX_REGEN_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(json_path.parent().unwrap()).expect("mkdir goldens");
        std::fs::write(&json_path, render_json()).expect("write json");
        std::fs::write(&cpp_path, render_cpp_header()).expect("write header");
        println!("goldens régénérés : {}", json_path.display());
        return;
    }
    let committed_json =
        std::fs::read_to_string(&json_path).expect("golden_vectors.json absent — lance la regen");
    let committed_cpp =
        std::fs::read_to_string(&cpp_path).expect("goldens.h absent — lance la regen");
    assert_eq!(
        committed_json,
        render_json(),
        "golden_vectors.json périmé — regen"
    );
    assert_eq!(
        committed_cpp,
        render_cpp_header(),
        "goldens.h périmé — regen"
    );
}
