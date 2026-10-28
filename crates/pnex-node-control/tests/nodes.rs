//! Control card tests: `inventory` registry (the 3 types) and
//! fail-loud rejection at build (`BadFlowsJson`) of an invalid card — the
//! vendor's silent degradation into an `unknown` node is exactly what
//! these tests make impossible.

use edgelink_core::runtime::registry::RegistryBuilder;

#[test]
fn registry_contains_the_three_cards() {
    // Anti-pruning anchor: without a reference to the crate, the linker may drop the
    // `inventory` submissions (same guard as the runtime binary).
    pnex_node_control::registered();
    let reg = RegistryBuilder::default().build().expect("registry");
    for type_name in ["pnex-reg-tt-heat", "pnex-reg-tt-cool", "pnex-reg-pid"] {
        let meta = reg
            .get(type_name)
            .unwrap_or_else(|| panic!("card {type_name} missing from the registry"));
        assert_eq!(meta.type_, type_name);
    }
}

/// RAW artefact of a valid card (projection stamps present).
fn reg_flows(type_name: &str, extra: serde_json::Value) -> String {
    let mut node = serde_json::json!({
        "id": "r1", "type": type_name, "z": "t",
        "device_id": "serre-1", "sensor_pin": "A0", "actuator_pin": "D1",
        "setpoint": 19.0,
        "pnex_node_id": "r1", "pnex_flow_id": 1,
        "x": 200, "y": 100, "wires": []
    });
    for (k, v) in extra.as_object().expect("object") {
        node[k] = v.clone();
    }
    serde_json::json!([
        { "id": "t", "type": "tab", "label": "test" },
        node
    ])
    .to_string()
}

fn build_flows(json: String) -> edgelink_core::Result<()> {
    let reg = RegistryBuilder::default().build().expect("registry");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        // The full engine build guarantees fail-loud: without the crate
        // linked, the type would fall back to `unknown` (warn only).
        edgelink_core::runtime::engine::Engine::with_json_string(&reg, json, None)?;
        Ok(())
    })
}

#[test]
fn build_accepts_valid_cards() {
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
            .unwrap_or_else(|e| panic!("valid {type_name} must build: {e}"));
    }
}

#[test]
fn build_refuses_without_deadband_tt() {
    // TT without deadband → reg_deadband_range violation at build.
    let json = reg_flows("pnex-reg-tt-heat", serde_json::json!({}));
    let err = build_flows(json).expect_err("TT without deadband must be rejected");
    assert!(
        err.to_string().to_lowercase().contains("deadband"),
        "expected deadband message, got: {err}"
    );
}

#[test]
fn build_refuses_stale_artifact() {
    // Without pnex_flow_id (stale artefact — see deploy/version memory) →
    // fail-loud rejection, not a silent no-op.
    let json = reg_flows("pnex-reg-tt-heat", serde_json::json!({"pnex_flow_id": 0}));
    assert!(
        build_flows(json).is_err(),
        "artefact without stamp must be rejected"
    );
}
