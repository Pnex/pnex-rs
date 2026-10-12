//! Tracking + zones on the real engine (media-vision-studio.md D192/D193):
//! a function node fakes `vision-detect` output (one person walking left to
//! right, 100 px/s on a 400 px frame) → `vision-track` → `vision-zone`
//! (left half) → debug. No Valkey, no model: pure flow logic.

mod common;

use std::time::Duration;

fn flows() -> String {
    let fake_detect = r#"
const x = (Date.now() / 10) % 400;
msg.payload = {
  device_id: "cam-1", ts_ms: Date.now(), width: 400, height: 100,
  frame_key: "k", detections: [{label: "person", class_id: 0, score: 0.9, bbox: [x, 20, 20, 60]}]
};
return msg;
"#;
    serde_json::json!([
        { "id": "pnexflow7", "type": "tab", "label": "zones", "pnex_flow_id": 7 },
        {
            "id": "tick", "type": "inject", "z": "pnexflow7",
            "props": [{ "p": "payload" }], "repeat": "0.1", "once": true,
            "payload": "x", "payloadType": "str", "wires": [["det"]]
        },
        {
            "id": "det", "type": "function", "z": "pnexflow7",
            "outputs": 1, "func": fake_detect, "wires": [["trk"]]
        },
        {
            "id": "trk", "type": "pnex-vision-track", "z": "pnexflow7",
            "high_score": 0.5, "min_iou": 0.2, "min_hits": 2, "lost_after_secs": 1.0,
            "wires": [["zone"], ["dbg"]]
        },
        {
            "id": "zone", "type": "pnex-vision-zone", "z": "pnexflow7",
            "zones": [{"name": "dock", "points": [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]]}],
            "labels": [], "anchor": "bottom", "min_dwell_secs": 0.0,
            "wires": [["dbg"]]
        },
        {
            "id": "dbg", "type": "debug", "z": "pnexflow7",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn tracked_person_enters_and_leaves_the_zone() {
    let home = common::tmp_home("vzone");
    let path = home.join("flows.json");
    std::fs::write(&path, flows()).unwrap();
    let rt = common::RuntimeProc::spawn(&path, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Debug stringifies the payload: match on the JSON text.
    rt.wait_for(
        |v| common::is_debug_with(v, "appear"),
        Duration::from_secs(15),
    );
    rt.wait_for(
        |v| common::is_debug_with(v, "enter") && common::is_debug_with(v, "dock"),
        Duration::from_secs(15),
    );
    rt.wait_for(
        |v| common::is_debug_with(v, "exit") && common::is_debug_with(v, "duration_s"),
        Duration::from_secs(15),
    );
    // The wrap-around (x 400 → 0) leaves the old track unmatched: it expires.
    rt.wait_for(
        |v| common::is_debug_with(v, "disappear"),
        Duration::from_secs(15),
    );
}
