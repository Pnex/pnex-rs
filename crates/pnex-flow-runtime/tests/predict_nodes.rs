//! Acceptance of the `pnex-anomaly` / `pnex-forecast` nodes on the real
//! engine: `inject(timestamp, every 50 ms) → anomaly + forecast → debug`.
//! The timestamp is a perfectly linear series, so the linear forecast must
//! predict the breach of a threshold set a few seconds in the future.
//! In-memory windows only (`VALKEY_URL` blanked).

mod common;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn predict_flows(threshold_ms: f64) -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "predict", "pnex_flow_id": 1 },
        {
            "id": "i1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }, { "p": "topic", "vt": "str" }],
            "topic": "bearing",
            "repeat": "0.05", "once": true, "onceDelay": 0.1,
            "payloadType": "date", "payload": "",
            "wires": [["a1", "f1"]]
        },
        {
            "id": "a1", "type": "pnex-anomaly", "z": "pnexflow1",
            "method": "robust_z", "window": 50, "min_samples": 10, "threshold": 3.5,
            "pnex_node_id": "a1", "pnex_flow_id": 1, "pnex_org_id": 1,
            "wires": [["d1"], []]
        },
        {
            "id": "f1", "type": "pnex-forecast", "z": "pnexflow1",
            "model": "linear", "window": 50, "min_samples": 10, "horizon": 400,
            "level": 0.95, "threshold": threshold_ms, "direction": "above", "every": 1,
            "pnex_node_id": "f1", "pnex_flow_id": 1, "pnex_org_id": 1,
            "wires": [["d1"], [], ["d2"]]
        },
        {
            "id": "d1", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        },
        {
            "id": "d2", "type": "debug", "z": "pnexflow1", "name": "eta",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn anomaly_scores_and_forecast_predicts_breach() {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let home = common::tmp_home("predict");
    let flows = home.join("flows.json");
    std::fs::write(&flows, predict_flows(now_ms + 8_000.0)).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, [("VALKEY_URL", "")]);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Anomaly detail once warmed up (a steady ramp is not an outlier).
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v["msg"]
                    .as_str()
                    .is_some_and(|m| m.contains("robust_z") && m.contains("\"warming_up\": false"))
        },
        Duration::from_secs(30),
    );
    let msg = line["msg"].as_str().unwrap();
    assert!(msg.contains("\"anomaly\": false"), "{msg}");
    // Forecast detail with a predicted breach.
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v["msg"]
                    .as_str()
                    .is_some_and(|m| m.contains("breach_in_secs") && m.contains("\"breach\": true"))
        },
        Duration::from_secs(30),
    );
    let msg = line["msg"].as_str().unwrap();
    assert!(msg.contains("\"model\": \"linear\""), "{msg}");
    // ETA port: a bare number of seconds, below the 8 s horizon of the test.
    let eta_line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v["msg"]
                    .as_str()
                    .is_some_and(|m| m.trim().parse::<f64>().is_ok())
        },
        Duration::from_secs(30),
    );
    let eta: f64 = eta_line["msg"].as_str().unwrap().trim().parse().unwrap();
    assert!((0.0..=9.0).contains(&eta), "eta {eta}");
}
