//! Tests de valeurs golden : physique connue, contre-vérifiée entre chemins
//! indépendants (scalaire vs multi vs shim FORTRAN vs AbstractState).
//!
//! Adapté de coolprop-rs (tests HTTP) en appels directs du wrapper safe.

use pnex_coolprop::{
    abstract_state_factory, abstract_state_free, abstract_state_keyed_output,
    abstract_state_update, get_input_pair_index, get_param_index, get_parameter_information_string,
    props_1si, props_si, props_si_multi, propssi_fortran, saturation_ancillary,
};

/// Point d'ébullition normal de l'eau d'après l'EOS IAPWS-95 utilisée par
/// CoolProp.
const WATER_NBP_K: f64 = 373.12429584768442;

#[test]
fn water_normal_boiling_point() {
    let t = props_si("T", "P", 101325.0, "Q", 0.0, "Water").expect("body");
    assert!((t - WATER_NBP_K).abs() < 1e-6, "NBP = {t}");
}

#[test]
fn water_molar_mass() {
    let m = props_1si("Water", "molar_mass").expect("body");
    assert!((m - 0.01801528).abs() < 1e-7, "molar mass = {m}");
}

/// L'exemple du README CoolProp, cohérent sur chaque chemin capable de le
/// calculer : PropsSI (défaut et préfixé HEOS), PropsSImulti et le shim
/// FORTRAN.
#[test]
fn mixture_example_consistent_across_paths() {
    let scalar =
        props_si("Dmolar", "T", 298.0, "P", 1e5, "Propane[0.5]&Ethane[0.5]").expect("props_si");
    assert!(scalar.is_finite() && scalar > 0.0);

    let heos = props_si(
        "Dmolar",
        "T",
        298.0,
        "P",
        1e5,
        "HEOS::Propane[0.5]&Ethane[0.5]",
    )
    .expect("props_si HEOS::");
    assert!((heos - scalar).abs() < 1e-9);

    let fortran = propssi_fortran("Dmolar", "T", 298.0, "P", 1e5, "Propane[0.5]&Ethane[0.5]")
        .expect("propssi_");
    assert!((fortran - scalar).abs() < 1e-9);

    let multi = props_si_multi(
        &["Dmolar".to_string()],
        "T",
        &[298.0],
        "P",
        &[1e5],
        "",
        &["Propane".to_string(), "Ethane".to_string()],
        &[0.5, 0.5],
    )
    .expect("PropsSImulti");
    assert!((multi[0][0] - scalar).abs() < 1e-6 * scalar.abs());
}

/// Un update AbstractState PT doit converger avec PropsSI pour le même état.
/// Les indices de paire d'entrée et de paramètre sont résolus par l'API
/// (l'ordre des enums C n'est pas une API stable — ne jamais hardcoder).
#[test]
fn abstract_state_agrees_with_props_si() {
    let handle = abstract_state_factory("HEOS", "Water").expect("factory");
    let pt = get_input_pair_index("PT_INPUTS").expect("PT_INPUTS");
    let dmolar = get_param_index("Dmolar").expect("Dmolar");

    abstract_state_update(handle, pt, 101325.0, 400.0).expect("update");
    let as_value = abstract_state_keyed_output(handle, dmolar).expect("keyed_output");

    let props_value = props_si("Dmolar", "T", 400.0, "P", 101325.0, "Water").expect("props_si");

    assert!(
        (as_value - props_value).abs() < 1e-6 * props_value.abs(),
        "AbstractState {as_value} vs PropsSI {props_value}",
    );

    abstract_state_free(handle).expect("free");
}

/// L'ancillaire de saturation approche la pression d'ébullition de l'EOS.
#[test]
fn saturation_ancillary_close_to_eos() {
    let ancillary = saturation_ancillary("Water", "P", 0, "T", WATER_NBP_K).expect("ancillary");
    let eos = props_si("P", "T", WATER_NBP_K, "Q", 0.0, "Water").expect("eos");

    assert!(
        (ancillary - eos).abs() / eos < 0.01,
        "ancillary {ancillary} vs EOS {eos}",
    );
}

/// `get_parameter_information_string` décrit les paramètres connus.
///
/// Quirk : le buffer de sortie sert de sélecteur d'entrée — ici `"long"`
/// (casse exacte, cf. `src/DataStructures.cpp`) pour la description longue.
#[test]
fn parameter_info_descriptions() {
    for (param, fragment) in [
        ("T", "Temperature"),
        ("P", "Pressure"),
        ("Dmolar", "density"),
    ] {
        let desc = get_parameter_information_string(param, "long")
            .unwrap_or_else(|e| panic!("{param}: {e:?}"))
            .to_lowercase();
        assert!(desc.contains(&fragment.to_lowercase()), "{param}: {desc}");
    }
}

/// Garde-fou : les noms de paires/paramètres se résolvent en indices valides,
/// les inconnus sont rejetés (contrat du wrapper, sans indices magiques).
#[test]
fn input_pair_and_param_indices_resolve() {
    assert!(get_input_pair_index("PT_INPUTS").expect("PT_INPUTS") >= 0);
    assert!(get_param_index("Dmolar").expect("Dmolar") >= 0);
    assert!(get_input_pair_index("NOT_A_PAIR").is_err());
    assert!(get_param_index("NOT_A_PARAM").is_err());
}
