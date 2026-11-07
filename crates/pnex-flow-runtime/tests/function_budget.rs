//! SEC-67 regression pin on the real binary: an endless user loop in a
//! deployed function node is interrupted by its call budget, and the same
//! node keeps serving the next messages.

mod common;

use std::time::Duration;

#[test]
fn endless_loop_is_interrupted_and_node_keeps_running() {
    let home = common::tmp_home("fn_budget");
    let flows = home.join("flows.json");
    let flow = serde_json::json!([
        { "id": "pnexflow9", "type": "tab", "label": "budget", "pnex_flow_id": 9 },
        {
            "id": "b1", "type": "inject", "z": "pnexflow9",
            "props": [{ "p": "payload" }],
            "repeat": "1", "once": true,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["b2"]]
        },
        {
            "id": "b2", "type": "function", "z": "pnexflow9", "outputs": 1,
            "func": "globalThis.__n = (globalThis.__n || 0) + 1; if (globalThis.__n === 1) { while (true) {} } msg.payload = 'after:' + globalThis.__n; return msg;",
            "x": 200, "y": 100, "wires": [["b3"]]
        },
        {
            "id": "b3", "type": "debug", "z": "pnexflow9",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 300, "y": 100, "wires": []
        }
    ]);
    std::fs::write(&flows, flow.to_string()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| common::is_debug_with(v, "after:"),
        Duration::from_secs(40),
    );
}
