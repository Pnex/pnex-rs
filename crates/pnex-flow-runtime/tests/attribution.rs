//! Tests E2E d'attribution du feed debug : chaque engine émet sur son canal
//! broadcast, la pompe estampille `flow` + id éditeur (`node_red`). Le
//! harnais spawn le vrai binaire (`CARGO_BIN_EXE_pnex-flow-runtime`).

mod common;

use std::time::Duration;

use common::RuntimeProc;

/// Deux tabs auto-déclenchés (inject en repeat) : le debug de chaque tab doit
/// être attribué à son flow, avec l'id éditeur (attribution hex → RED).
#[test]
fn attribution_multi_tabs() {
    let home = common::tmp_home("attribution-tabs");
    let flows = home.join("flows.json");
    // Tab 1 : inject intervalle (0.1 s) → debug — trafic continu attribué
    // flow=1. Tab 2 : inject intervalle lent (0.5 s) → debug — attribué
    // flow=2, sans jamais mélanger les deux flux.
    let artifact = serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "f1", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "payload": "\"bg\"", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "complete": "payload",
            "x": 200, "y": 100, "wires": []
        },
        { "id": "pnexflow2", "type": "tab", "label": "f2", "pnex_flow_id": 2 },
        {
            "id": "m1", "type": "inject", "z": "pnexflow2",
            "props": [{ "p": "payload" }],
            "repeat": "0.5", "payload": "{\"tab\":2}", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["m2"]]
        },
        {
            "id": "m2", "type": "debug", "z": "pnexflow2",
            "active": true, "tosidebar": true, "complete": "payload",
            "x": 200, "y": 100, "wires": []
        }
    ]);
    std::fs::write(&flows, artifact.to_string()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // Le debug du tab 1 est attribué flow=1 avec l'id éditeur n2 (sortie
    // builtin stringifiée — le debug builtin pré-stringifie sa sortie via
    // `format_message_for_display`).
    let dbg1 = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.get("flow").and_then(|f| f.as_i64()) == Some(1)
                && v.get("node_red").and_then(|n| n.as_str()) == Some("n2")
        },
        Duration::from_secs(30),
    );
    assert!(
        dbg1["msg"].is_string() && dbg1["msg"].as_str().unwrap().contains("bg"),
        "payload stringifié attendu : {dbg1}"
    );

    // Le debug du tab 2 est attribué flow=2 avec l'id éditeur m2 — la
    // corrélation flow/nœud ne fuit jamais d'un tab vers l'autre.
    let dbg2 = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.get("flow").and_then(|f| f.as_i64()) == Some(2)
                && v.get("node_red").and_then(|n| n.as_str()) == Some("m2")
                && v.to_string().contains("tab")
        },
        Duration::from_secs(30),
    );
    assert!(
        dbg2["msg"].is_string() && dbg2["msg"].as_str().unwrap().contains("\"tab\""),
        "payload stringifié attendu : {dbg2}"
    );
}
