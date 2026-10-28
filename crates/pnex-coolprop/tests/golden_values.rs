//! Golden value tests: known physics, cross-checked between independent
//! paths (scalar vs multi vs FORTRAN shim vs AbstractState).
//!
//! Adapted from coolprop-rs (HTTP tests) into direct calls of the safe wrapper.

use pnex_coolprop::{
    abstract_state_factory, abstract_state_free, abstract_state_keyed_output,
    abstract_state_update, get_input_pair_index, get_param_index, get_parameter_information_string,
    props_1si, props_si, props_si_multi, propssi_fortran, saturation_ancillary,
};

/// Normal boiling point of water according to the IAPWS-95 EOS used by
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

/// The CoolProp README example, consistent on every path able to
/// compute it: PropsSI (default and HEOS-prefixed), PropsSImulti and the
/// FORTRAN shim.
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

/// A PT AbstractState update must converge with PropsSI for the same state.
/// Input pair and parameter indices are resolved through the API
/// (the C enum order is not a stable API — never hardcode).
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

/// The saturation ancillary approaches the EOS boiling pressure.
#[test]
fn saturation_ancillary_close_to_eos() {
    let ancillary = saturation_ancillary("Water", "P", 0, "T", WATER_NBP_K).expect("ancillary");
    let eos = props_si("P", "T", WATER_NBP_K, "Q", 0.0, "Water").expect("eos");

    assert!(
        (ancillary - eos).abs() / eos < 0.01,
        "ancillary {ancillary} vs EOS {eos}",
    );
}

/// `get_parameter_information_string` describes the known parameters.
///
/// Quirk: the output buffer serves as an input selector — here `"long"`
/// (exact case, see `src/DataStructures.cpp`) for the long description.
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

/// Safeguard: pair/parameter names resolve to valid indices,
/// unknown ones are rejected (wrapper contract, no magic indices).
#[test]
fn input_pair_and_param_indices_resolve() {
    assert!(get_input_pair_index("PT_INPUTS").expect("PT_INPUTS") >= 0);
    assert!(get_param_index("Dmolar").expect("Dmolar") >= 0);
    assert!(get_input_pair_index("NOT_A_PAIR").is_err());
    assert!(get_param_index("NOT_A_PARAM").is_err());
}
