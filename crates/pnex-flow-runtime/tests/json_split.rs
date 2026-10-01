//! Regression pin: `json-split` fan-out on the real engine — chain
//! inject → pnex-value({"test":25,"test2":30}) → function(split) → debug.
//! Legacy config (zero declared keys): one port, one msg per key.

mod common;

use std::time::Duration;

fn legacy_split_flows() -> String {
    let func = pnex_core::functions::build_json_split_func(&[]);
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "test", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "pnex-value", "z": "pnexflow1",
            "mode": "static", "value": {"test": 25, "test2": 30},
            "min": 0, "max": 10,
            "x": 200, "y": 100, "wires": [["n3"]]
        },
        {
            "id": "n3", "type": "function", "z": "pnexflow1",
            "outputs": 1,
            "func": func,
            "x": 300, "y": 100, "wires": [["n4"]]
        },
        {
            "id": "n4", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 400, "y": 100, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn json_split_emet_un_msg_par_cle() {
    let home = common::tmp_home("jsplit");
    let flows = home.join("flows.json");
    std::fs::write(&flows, legacy_split_flows()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Split OK = debug voit 25 ET 30 (un msg par clé, payload = valeur).
    rt.wait_for(|v| common::is_debug_with(v, "25"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "30"), Duration::from_secs(30));
}

fn named_split_flows() -> String {
    // Clés déclarées ["test","test2"] = 2 ports nommés ; « extra » n'a pas
    // de port → ignoré. Le payload route par port, pas par fan-out.
    let func =
        pnex_core::functions::build_json_split_func(&["test".to_string(), "test2".to_string()]);
    serde_json::json!([
        { "id": "pnexflow2", "type": "tab", "label": "named", "pnex_flow_id": 2 },
        {
            "id": "m1", "type": "inject", "z": "pnexflow2",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["m2"]]
        },
        {
            "id": "m2", "type": "pnex-value", "z": "pnexflow2",
            "mode": "static",
            "value": {"test": 25, "test2": 30, "extra": 99},
            "min": 0, "max": 10,
            "x": 200, "y": 100, "wires": [["m3"]]
        },
        {
            "id": "m3", "type": "function", "z": "pnexflow2",
            "outputs": 2,
            "func": func,
            "x": 300, "y": 100, "wires": [["m4"], ["m5"]]
        },
        {
            "id": "m4", "type": "debug", "z": "pnexflow2",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 400, "y": 100, "wires": []
        },
        {
            "id": "m5", "type": "debug", "z": "pnexflow2",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 400, "y": 200, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn json_split_ports_nommes_routent_par_port() {
    let home = common::tmp_home("jsplitnamed");
    let flows = home.join("flows.json");
    std::fs::write(&flows, named_split_flows()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // Port « test » → 25, port « test2 » → 30 ; « extra » est ignoré.
    rt.wait_for(|v| common::is_debug_with(v, "25"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "30"), Duration::from_secs(30));
}

fn array_split_flows() -> String {
    // Payload array : tous les éléments sortent sur le port 0 (topic =
    // index), même avec 2 ports déclarés.
    let func =
        pnex_core::functions::build_json_split_func(&["test".to_string(), "test2".to_string()]);
    serde_json::json!([
        { "id": "pnexflow3", "type": "tab", "label": "array", "pnex_flow_id": 3 },
        {
            "id": "a1", "type": "inject", "z": "pnexflow3",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["a2"]]
        },
        {
            "id": "a2", "type": "pnex-value", "z": "pnexflow3",
            "mode": "static", "value": [7, 8],
            "min": 0, "max": 10,
            "x": 200, "y": 100, "wires": [["a3"]]
        },
        {
            "id": "a3", "type": "function", "z": "pnexflow3",
            "outputs": 2,
            "func": func,
            "x": 300, "y": 100, "wires": [["a4"], []]
        },
        {
            "id": "a4", "type": "debug", "z": "pnexflow3",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 400, "y": 100, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn json_split_array_passe_sur_port_zero() {
    let home = common::tmp_home("jsplitarray");
    let flows = home.join("flows.json");
    std::fs::write(&flows, array_split_flows()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    // 7 ET 8 sortent sur le port 0 (debug a4), topic = index.
    rt.wait_for(|v| common::is_debug_with(v, "7"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "8"), Duration::from_secs(30));
}
