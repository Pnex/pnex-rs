//! Acceptance of the `pnex-memory-write` / `pnex-memory-read` nodes against
//! a live Valkey: `inject → memory-write`, then a later `inject →
//! memory-read → debug` reads the value back (object port + key port).
//!
//! Gated on `PNEX_TEST_VALKEY_URL` (e.g. `redis://127.0.0.1:6379`, compose
//! valkey): skips cleanly when absent.

mod common;

use std::time::Duration;

fn memory_flows(key: &str) -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "memory", "pnex_flow_id": 1 },
        {
            "id": "i1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "", "once": true, "onceDelay": 0.1,
            "payloadType": "json", "payload": "{\"Hmass\":412.5,\"P\":3.2}",
            "wires": [["w1"]]
        },
        {
            "id": "w1", "type": "pnex-memory-write", "z": "pnexflow1",
            "key": key, "ttl_secs": 60, "pnex_org_id": 999_001,
            "wires": [[]]
        },
        {
            "id": "i2", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "", "once": true, "onceDelay": 1.0,
            "payloadType": "num", "payload": "0",
            "wires": [["r1"]]
        },
        {
            "id": "r1", "type": "pnex-memory-read", "z": "pnexflow1",
            "keys": [key, "never.written"], "max_age_secs": 30,
            "pnex_org_id": 999_001,
            "wires": [["d1"], ["d2"], ["d3"]]
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
        },
        {
            "id": "d3", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "true", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn write_then_read_back_through_valkey() {
    let Ok(valkey) = std::env::var("PNEX_TEST_VALKEY_URL") else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    // Unique key per run: entries outlive the test (TTL 60 s).
    let key = format!("test.{}", std::process::id());
    let home = common::tmp_home("memory");
    let flows = home.join("flows.json");
    std::fs::write(&flows, memory_flows(&key)).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, [("VALKEY_URL", valkey.as_str())]);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Ports fan out concurrently: collect both debug lines in any order;
    // the missing key's port (d3) stays muted.
    let is_debug = |v: &serde_json::Value| v.get("event").and_then(|e| e.as_str()) == Some("debug");
    let a = rt.wait_for(is_debug, Duration::from_secs(30));
    let b = rt.wait_for(is_debug, Duration::from_secs(30));
    let (object, scalar) = if a["node_red"] == "d1" {
        (a, b)
    } else {
        (b, a)
    };

    let object = object["msg"].as_str().unwrap().to_string();
    assert!(object.contains("412.5"), "{object}");
    assert!(object.contains("\"never.written\": null"), "{object}");
    let scalar = scalar["msg"].as_str().unwrap().to_string();
    assert!(scalar.contains(&key), "topic = key: {scalar}");
    assert!(scalar.contains("412.5"), "{scalar}");
}
