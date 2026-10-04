//! Security regression pins (docs/architecture/security.md SEC-1, SEC-3) on
//! the real binary:
//! - user JS never reads the runtime's process environment through `env.get`
//!   (the supervisor puts platform credentials there);
//! - a host-effect builtin (`exec`) is not registered: the node loads as the
//!   inert `unknown` placeholder and never runs its command.

mod common;

use std::time::Duration;

const PROBE_VAR: &str = "PNEX_SEC3_PROBE_SECRET";
const PROBE_VALUE: &str = "sec3-leaked-value";

fn env_probe_flows(marker: &std::path::Path) -> String {
    serde_json::json!([
        { "id": "pnexflow9", "type": "tab", "label": "sec", "pnex_flow_id": 9 },
        {
            "id": "a1", "type": "inject", "z": "pnexflow9",
            "props": [{ "p": "payload" }],
            "repeat": "0.2", "once": false,
            "payload": "x", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["a2", "a4"]]
        },
        {
            "id": "a2", "type": "function", "z": "pnexflow9",
            "outputs": 1,
            "func": format!(
                "msg.payload = 'probe:' + String(env.get('{PROBE_VAR}')); return msg;"
            ),
            "x": 200, "y": 100, "wires": [["a3"]]
        },
        {
            "id": "a3", "type": "debug", "z": "pnexflow9",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 300, "y": 100, "wires": []
        },
        {
            "id": "a4", "type": "exec", "z": "pnexflow9",
            "command": format!("touch {}", marker.display()),
            "addpay": "", "append": "", "useSpawn": "false", "timer": "",
            "winHide": false, "oldrc": false,
            "x": 200, "y": 200, "wires": [[], [], []]
        }
    ])
    .to_string()
}

#[test]
fn user_code_sees_no_process_env_and_exec_never_runs() {
    let home = common::tmp_home("sec_env");
    let marker = home.join("exec-ran");
    let flows = home.join("flows.json");
    std::fs::write(&flows, env_probe_flows(&marker)).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, [(PROBE_VAR, PROBE_VALUE)]);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    let seen = rt.wait_for(
        |v| common::is_debug_with(v, "probe:"),
        Duration::from_secs(30),
    );
    let text = seen.to_string();
    assert!(!text.contains(PROBE_VALUE), "process env leaked: {text}");
    assert!(text.contains("probe:undefined"), "{text}");
    assert!(!marker.exists(), "exec node ran its command");
}
