//! Acceptance nœud `pnex-coolprop` : un flow
//! `inject(json) → pnex-coolprop → debug` calcule les propriétés
//! thermophysiques in-process — aucune base, aucune env requise.

mod common;

use std::time::Duration;

/// Flows JSON projeté (miroir de `to_red_flows_json`).
fn coolprop_flows() -> String {
    serde_json::json!([
        {
            "id": "pnexflow1", "type": "tab", "label": "coolprop",
            "pnex_flow_id": 1
        },
        {
            "id": "i1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "", "once": true, "onceDelay": 0.1,
            "payloadType": "json",
            "payload": "{\"t\":400.0,\"p\":101325.0}",
            "wires": [["c1"]]
        },
        {
            "id": "c1", "type": "pnex-coolprop", "z": "pnexflow1",
            "fluid_spec": "Water",
            "input1": "T", "input2": "P",
            "v1_key": "t", "v2_key": "p",
            "outputs": ["Dmolar", "Hmolar", "Smolar"],
            "include_phase": true,
            "wires": [["d1"]]
        },
        {
            "id": "d1", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn inject_coolprop_debug_renvoie_les_proprietes() {
    let home = common::tmp_home("coolprop");
    let flows = home.join("flows.json");
    std::fs::write(&flows, coolprop_flows()).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, []);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v["msg"].as_str().is_some_and(|m| m.contains("Dmolar"))
        },
        Duration::from_secs(30),
    );
    let msg = line["msg"].as_str().unwrap();
    assert!(msg.contains("phase"), "{msg}");
    // Vapeur d'eau à 400 K / 1 bar : Dmolar ≈ P/(RT) ≈ 30,5 mol/m³.
    // (gaz parfait ≈ 0.996 de compressibilité — garde-fou anti-garbage).
    let dmolar: f64 = msg
        .split("\"Dmolar\": ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().trim_end_matches('}').parse().ok())
        .expect("Dmolar numérique dans le debug");
    assert!(
        (25.0..35.0).contains(&dmolar),
        "Dmolar hors plage physique : {dmolar}"
    );
}

/// Named-anchor mode: the two inputs arrive in separate messages
/// (`topic` = input key, stamped by the deploy taggers), in user units.
fn coolprop_named_inputs_flows() -> String {
    serde_json::json!([
        {
            "id": "pnexflow1", "type": "tab", "label": "coolprop-units",
            "pnex_flow_id": 1
        },
        {
            "id": "i1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }, { "p": "topic", "vt": "str", "v": "in1" }],
            "topic": "in1",
            "repeat": "", "once": true, "onceDelay": 0.1,
            "payloadType": "num", "payload": "25",
            "wires": [["c1"]]
        },
        {
            "id": "i2", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }, { "p": "topic", "vt": "str", "v": "in2" }],
            "topic": "in2",
            "repeat": "", "once": true, "onceDelay": 0.4,
            "payloadType": "num", "payload": "0",
            "wires": [["c1"]]
        },
        {
            "id": "c1", "type": "pnex-coolprop", "z": "pnexflow1",
            "fluid_spec": "Water",
            "input1": "T", "input2": "P",
            "v1_key": "in1", "v2_key": "in2",
            "unit1": "degC", "unit2": "barg",
            "outputs": ["Hmass", "subcooling"],
            "output_units": { "Hmass": "kJ/kg", "subcooling": "K" },
            "include_phase": false,
            "wires": [["d1"], ["d2"], []]
        },
        {
            "id": "d1", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        },
        {
            "id": "d2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "true", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn named_inputs_with_units_feed_object_and_scalar_ports() {
    let home = common::tmp_home("coolprop-units");
    let flows = home.join("flows.json");
    std::fs::write(&flows, coolprop_named_inputs_flows()).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, []);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Ports fan out concurrently: collect both debug lines in any order.
    let is_debug = |v: &serde_json::Value| v.get("event").and_then(|e| e.as_str()) == Some("debug");
    let a = rt.wait_for(is_debug, Duration::from_secs(30));
    let b = rt.wait_for(is_debug, Duration::from_secs(30));
    let (object, scalar) = if a["node_red"] == "d1" {
        (a, b)
    } else {
        (b, a)
    };

    // Port 0: object with both outputs. Liquid water at 25 °C / 1 atm:
    // h ≈ 104.9 kJ/kg, subcooling ≈ 100 − 25 = 75 K.
    let msg = object["msg"].as_str().unwrap();
    let field = |key: &str| -> f64 {
        msg.split(&format!("\"{key}\": "))
            .nth(1)
            .and_then(|s| s.split([',', '\n', '}']).next())
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| panic!("{key} missing in {msg}"))
    };
    let h = field("Hmass");
    assert!((100.0..110.0).contains(&h), "Hmass out of range: {h}");
    let sc = field("subcooling");
    assert!((74.0..76.0).contains(&sc), "subcooling out of range: {sc}");

    // Port 1: the Hmass scalar alone, topic = output id.
    let msg = scalar["msg"].as_str().unwrap();
    assert!(msg.contains("\"topic\": \"Hmass\""), "{msg}");
    assert!(msg.contains("\"payload\": 104."), "{msg}");
}
