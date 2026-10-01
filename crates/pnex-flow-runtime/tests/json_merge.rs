//! Regression pin: `json-merge` (Merge to JSON) named inputs on the real
//! engine — two source chains fan-in to one merge node through topic
//! taggers (the deploy projection's synthesis), and the accumulator is
//! re-emitted on every received message. One named input = one variable.

mod common;

use std::time::Duration;

fn merge_flows() -> String {
    let tag_temp = pnex_core::functions::build_input_tag_func("temp");
    let tag_hum = pnex_core::functions::build_input_tag_func("hum");
    let merge = pnex_core::functions::build_json_merge_func("value");
    serde_json::json!([
        { "id": "pnexflow4", "type": "tab", "label": "merge", "pnex_flow_id": 4 },
        {
            "id": "t1", "type": "inject", "z": "pnexflow4",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["t2"]]
        },
        {
            "id": "t2", "type": "pnex-value", "z": "pnexflow4",
            "mode": "static", "value": 20,
            "min": 0, "max": 10,
            "x": 200, "y": 100, "wires": [["t3"]]
        },
        {
            "id": "t3", "type": "function", "z": "pnexflow4",
            "outputs": 1,
            "func": tag_temp,
            "x": 300, "y": 100, "wires": [["t5"]]
        },
        {
            "id": "u1", "type": "inject", "z": "pnexflow4",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 300, "wires": [["u2"]]
        },
        {
            "id": "u2", "type": "pnex-value", "z": "pnexflow4",
            "mode": "static", "value": 50,
            "min": 0, "max": 10,
            "x": 200, "y": 300, "wires": [["u3"]]
        },
        {
            "id": "u3", "type": "function", "z": "pnexflow4",
            "outputs": 1,
            "func": tag_hum,
            "x": 300, "y": 300, "wires": [["t5"]]
        },
        {
            "id": "t5", "type": "function", "z": "pnexflow4",
            "outputs": 1,
            "func": merge,
            "x": 400, "y": 200, "wires": [["t6"]]
        },
        {
            "id": "t6", "type": "debug", "z": "pnexflow4",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 500, "y": 200, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn json_merge_accumule_par_entree_nommee() {
    let home = common::tmp_home("jmerge");
    let flows = home.join("flows.json");
    std::fs::write(&flows, merge_flows()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Les deux variables atterrissent sous leur nom dans l'objet accumulé,
    // réémis à chaque message (debug = payload stringifié).
    rt.wait_for(
        |v| common::is_debug_with(v, "temp"),
        Duration::from_secs(30),
    );
    rt.wait_for(|v| common::is_debug_with(v, "hum"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "20"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "50"), Duration::from_secs(30));
}
