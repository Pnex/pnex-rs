//! Tests des cartes de régulation : registre `inventory` (les 3 types) et
//! rejet fail-loud au build (`BadFlowsJson`) d'une carte invalide — la
//! dégradation silencieuse en nœud `unknown` du vendor est exactement ce
//! que ces tests rendent impossible.

use edgelink_core::runtime::registry::RegistryBuilder;

#[test]
fn registre_contient_les_trois_cartes() {
    // Ancre anti-élagage : sans référence au crate, le linker peut jeter les
    // soumissions `inventory` (même garde que le binaire runtime).
    pnex_node_control::registered();
    let reg = RegistryBuilder::default().build().expect("registre");
    for type_name in ["pnex-reg-tt-heat", "pnex-reg-tt-cool", "pnex-reg-pid"] {
        let meta = reg
            .get(type_name)
            .unwrap_or_else(|| panic!("carte {type_name} absente du registre"));
        assert_eq!(meta.type_, type_name);
    }
}

/// Artefact RAW d'une carte valide (estampilles de la projection présentes).
fn reg_flows(type_name: &str, extra: serde_json::Value) -> String {
    let mut node = serde_json::json!({
        "id": "r1", "type": type_name, "z": "t",
        "device_id": "serre-1", "sensor_pin": "A0", "actuator_pin": "D1",
        "setpoint": 19.0,
        "pnex_node_id": "r1", "pnex_flow_id": 1,
        "x": 200, "y": 100, "wires": []
    });
    for (k, v) in extra.as_object().expect("objet") {
        node[k] = v.clone();
    }
    serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        node
    ])
    .to_string()
}

fn build_flows(json: String) -> edgelink_core::Result<()> {
    let reg = RegistryBuilder::default().build().expect("registre");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        // Le build complet du moteur garantit le fail-loud : sans le crate
        // lié, le type tomberait dans le fallback `unknown` (warn seul).
        edgelink_core::runtime::engine::Engine::with_json_string(&reg, json, None)?;
        Ok(())
    })
}

#[test]
fn build_accepte_les_cartes_valides() {
    for (type_name, extra) in [
        ("pnex-reg-tt-heat", serde_json::json!({"deadband": 0.5})),
        (
            "pnex-reg-tt-cool",
            serde_json::json!({"deadband": 0.5, "actuator_pin": "D2"}),
        ),
        (
            "pnex-reg-pid",
            serde_json::json!({"kp": 2.0, "ki": 0.1, "kd": 0.5, "actuator_pin": "D3"}),
        ),
    ] {
        build_flows(reg_flows(type_name, extra))
            .unwrap_or_else(|e| panic!("{type_name} valide doit builder : {e}"));
    }
}

#[test]
fn build_refuse_sans_deadband_tt() {
    // TT sans deadband → violation reg_deadband_range au build.
    let json = reg_flows("pnex-reg-tt-heat", serde_json::json!({}));
    let err = build_flows(json).expect_err("TT sans deadband doit être rejeté");
    assert!(
        err.to_string().to_lowercase().contains("deadband"),
        "message attendu deadband, reçu : {err}"
    );
}

#[test]
fn build_refuse_artefact_perime() {
    // Sans pnex_flow_id (artefact périmé — cf. mémoire deploy/version) →
    // rejet fail-loud, pas de no-op silencieux.
    let json = reg_flows("pnex-reg-tt-heat", serde_json::json!({"pnex_flow_id": 0}));
    assert!(
        build_flows(json).is_err(),
        "artefact sans stamp doit être rejeté"
    );
}
