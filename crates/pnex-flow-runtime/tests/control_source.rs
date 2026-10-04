//! Acceptance of the `pnex-control-source` node (D127) against a live
//! Valkey: a stored control value is replayed at start (`emit_on_start`),
//! then a live write published on the org channel comes out on the port of
//! its control with `topic` = control key.
//!
//! Gated on `PNEX_TEST_VALKEY_URL` (e.g. `redis://127.0.0.1:6379`, compose
//! valkey): skips cleanly when absent.

mod common;

use std::time::Duration;

use pnex_core::ui_control::{control_channel, control_value_key, ControlEvent, ControlValue};

/// Org id unique per run: the channel and keys only see this test.
fn org() -> i64 {
    800_000_000 + i64::from(std::process::id())
}

fn flows(a: uuid::Uuid, b: uuid::Uuid) -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "controls", "pnex_flow_id": 1 },
        {
            "id": "cs", "type": "pnex-control-source", "z": "pnexflow1",
            "controls": [a.to_string(), b.to_string()], "emit_on_start": true,
            "pnex_org_id": org(), "pnex_node_id": "cs",
            "wires": [["da"], ["db"]]
        },
        {
            "id": "da", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "true", "wires": []
        },
        {
            "id": "db", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "true", "wires": []
        }
    ])
    .to_string()
}

fn event(id: uuid::Uuid, key: &str, v: f64) -> ControlEvent {
    ControlEvent {
        control_id: id,
        key: key.into(),
        value: ControlValue {
            v,
            ts_ms: chrono::Utc::now().timestamp_millis(),
            by: Some("e2e".into()),
            via: Some("dashboard:e2e".into()),
            option: None,
        },
    }
}

#[test]
fn replays_stored_value_then_routes_live_writes_by_control() {
    let Ok(valkey) = std::env::var("PNEX_TEST_VALKEY_URL") else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let client = redis::Client::open(valkey.as_str()).expect("valkey url");
    let mut conn = client.get_connection().expect("valkey connection");
    // Stored before the engine starts: must be replayed on port 0.
    let stored = serde_json::to_string(&event(a, "light.room", 1.0)).unwrap();
    let _: () = redis::cmd("SET")
        .arg(control_value_key(org(), a))
        .arg(stored)
        .arg("EX")
        .arg(60)
        .query(&mut conn)
        .expect("seed value");

    let home = common::tmp_home("control-source");
    let flows_path = home.join("flows.json");
    std::fs::write(&flows_path, flows(a, b)).unwrap();
    let rt =
        common::RuntimeProc::spawn_with_env(&flows_path, &home, [("VALKEY_URL", valkey.as_str())]);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    let debug_of = |node: &'static str| {
        move |v: &serde_json::Value| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug") && v["node_red"] == node
        }
    };

    // Replay: proves the subscription is live (replay runs after SUBSCRIBE).
    let replay = rt.wait_for(debug_of("da"), Duration::from_secs(30));
    let msg = replay["msg"].as_str().unwrap().to_string();
    assert!(msg.contains("\"topic\": \"light.room\""), "{msg}");
    assert!(msg.contains("\"payload\": 1"), "{msg}");

    // Live write of control b (PWM duty) → port 1 only.
    let live = serde_json::to_string(&event(b, "fan.duty", 42.5)).unwrap();
    let _: () = redis::cmd("PUBLISH")
        .arg(control_channel(org()))
        .arg(live)
        .query(&mut conn)
        .expect("publish");
    let got = rt.wait_for(debug_of("db"), Duration::from_secs(30));
    let msg = got["msg"].as_str().unwrap().to_string();
    assert!(msg.contains("\"topic\": \"fan.duty\""), "{msg}");
    assert!(msg.contains("42.5"), "{msg}");
    assert!(msg.contains("dashboard:e2e"), "msg.control.via: {msg}");

    // A control of another node is ignored, then a write of a is routed.
    let other = serde_json::to_string(&event(uuid::Uuid::new_v4(), "other", 7.0)).unwrap();
    let again = serde_json::to_string(&event(a, "light.room", 0.0)).unwrap();
    for frame in [other, again] {
        let _: () = redis::cmd("PUBLISH")
            .arg(control_channel(org()))
            .arg(frame)
            .query(&mut conn)
            .expect("publish");
    }
    let next = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("debug"),
        Duration::from_secs(30),
    );
    assert_eq!(next["node_red"], "da", "{next}");
    let msg = next["msg"].as_str().unwrap().to_string();
    assert!(msg.contains("\"payload\": 0"), "{msg}");
}
