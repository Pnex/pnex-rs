//! E2E tests of debug feed attribution: each engine emits on its broadcast
//! channel, the pump stamps `flow` + editor id (`node_red`). The
//! harness spawns the real binary (`CARGO_BIN_EXE_pnex-flow-runtime`).

mod common;

use std::time::Duration;

use common::RuntimeProc;

/// Two self-triggered tabs (repeating inject): each tab's debug must
/// be attributed to its flow, with the editor id (hex → RED attribution).
#[test]
fn attribution_multi_tabs() {
    let home = common::tmp_home("attribution-tabs");
    let flows = home.join("flows.json");
    // Tab 1: interval inject (0.1 s) → debug — continuous traffic attributed
    // flow=1. Tab 2: slow interval inject (0.5 s) → debug — attributed
    // flow=2, never mixing the two streams.
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

    // Tab 1's debug is attributed flow=1 with editor id n2 (stringified
    // builtin output — the builtin debug pre-stringifies its output via
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
        "stringified payload expected: {dbg1}"
    );

    // Tab 2's debug is attributed flow=2 with editor id m2 — the
    // flow/node correlation never leaks from one tab to the other.
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
        "stringified payload expected: {dbg2}"
    );
}
