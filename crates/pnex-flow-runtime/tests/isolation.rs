//! E2E isolation tests "N Engines, 1 process": an invalid tab only fails
//! its own engine (`flow_error`), the others keep running; reloading
//! a tab does not stop the others; the last-good keeps running when
//! a swap fails. The harness spawns the real binary
//! (`CARGO_BIN_EXE_pnex-flow-runtime`).

mod common;

use std::time::Duration;

use common::RuntimeProc;

/// 2-tab artifact: flow 1 healthy in the background (inject 0.1 s → debug), flow 2
/// configurable (variable payload, or broken by a wire to a non-existent
/// node — fatal to the tab's build, see vendor flow.rs
/// "Referenced node not found").
fn two_flows(tab2_payload: &str, tab2_broken: bool) -> String {
    let mut v = serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "f1", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": "bg-1", "payloadType": "str",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 200, "y": 100, "wires": []
        },
        { "id": "pnexflow2", "type": "tab", "label": "f2", "pnex_flow_id": 2 },
        {
            "id": "m1", "type": "inject", "z": "pnexflow2",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "once": false,
            "payload": tab2_payload, "payloadType": "str",
            "x": 100, "y": 100, "wires": [["m2"]]
        },
        {
            "id": "m2", "type": "debug", "z": "pnexflow2",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 200, "y": 100, "wires": []
        }
    ]);

    // Ghost wire = tab build failure (Referenced node not found).
    if tab2_broken {
        v[4]["wires"] = serde_json::json!([["ghost"]]);
    }
    v.to_string()
}

fn child_alive(rt: &mut RuntimeProc) -> bool {
    rt.child.try_wait().expect("try_wait").is_none()
}

#[test]
fn invalid_tab_does_not_kill_the_other_flows() {
    let home = common::tmp_home("iso-1");
    let flows = home.join("flows.json");
    std::fs::write(&flows, two_flows("ok-2", true)).unwrap();

    let mut rt = RuntimeProc::spawn(&flows, &home);
    // Boot: flow 1 started, flow 2 in error — the process lives. Per-flow
    // events precede `started` (wait_for consumes without putting back).
    let started1 = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 1,
        Duration::from_secs(30),
    );
    assert!(started1["rev"].is_string(), "rev expected: {started1}");
    let failed2 = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("flow_error")
                && v["flow"] == 2
                && v.get("error")
                    .and_then(|e| e.as_str())
                    .is_some_and(|e| !e.is_empty())
        },
        Duration::from_secs(30),
    );
    println!("flow 2 in error: {failed2}");
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // The healthy flow keeps emitting.
    rt.wait_for(
        |v| common::is_debug_with(v, "bg-1"),
        Duration::from_secs(30),
    );
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        child_alive(&mut rt),
        "the runtime must survive the invalid tab"
    );
}

#[test]
fn reloading_one_tab_spares_the_other() {
    let home = common::tmp_home("iso-2");
    let flows = home.join("flows.json");
    std::fs::write(&flows, two_flows("ok-2", true)).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 1,
        Duration::from_secs(30),
    );
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_error") && v["flow"] == 2,
        Duration::from_secs(30),
    );

    // Fix of tab 2 only (new payload) — tab 1 is unchanged.
    let tmp = home.join("flows.json.tmp");
    std::fs::write(&tmp, two_flows("fixed-2", false)).unwrap();
    std::fs::rename(&tmp, &flows).unwrap();
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR1);
    }

    // Idempotent confirmations: BOTH flows emit flow_started
    // (2 after a successful swap, 1 unchanged — idempotent acknowledgement).
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 2,
        Duration::from_secs(30),
    );
    rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("redeployed")
                && v["changed"].as_u64() == Some(1)
        },
        Duration::from_secs(30),
    );
    // The repaired graph runs...
    rt.wait_for(
        |v| common::is_debug_with(v, "fixed-2"),
        Duration::from_secs(30),
    );
    // ...and so does the unchanged flow (never interrupted).
    rt.wait_for(
        |v| common::is_debug_with(v, "bg-1"),
        Duration::from_secs(30),
    );

    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("runtime.json")).expect("runtime.json"),
    )
    .unwrap();
    assert_eq!(state["redeploys"].as_u64(), Some(1));
}

#[test]
fn swap_failure_preserves_the_last_good() {
    let home = common::tmp_home("iso-3");
    let flows = home.join("flows.json");
    std::fs::write(&flows, two_flows("ok-2", false)).unwrap();

    let mut rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 2,
        Duration::from_secs(30),
    );

    // Tab 1 becomes invalid (ghost wire) — tab 2 unchanged.
    let mut artifact = two_flows("ok-2", false);
    // Replaces n1's wires (index 1) with a ghost reference.
    let mut value: serde_json::Value = serde_json::from_str(&artifact).unwrap();
    value[1]["wires"] = serde_json::json!([["ghost"]]);
    artifact = value.to_string();

    let tmp = home.join("flows.json.tmp");
    std::fs::write(&tmp, &artifact).unwrap();
    std::fs::rename(&tmp, &flows).unwrap();
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR1);
    }

    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_error") && v["flow"] == 1,
        Duration::from_secs(30),
    );

    // Flow 1's last-good keeps emitting (ingestion preserved) —
    // its engine was not replaced.
    rt.wait_for(
        |v| common::is_debug_with(v, "bg-1"),
        Duration::from_secs(30),
    );
    std::thread::sleep(Duration::from_millis(400));
    rt.wait_for(
        |v| common::is_debug_with(v, "bg-1"),
        Duration::from_secs(30),
    );
    assert!(
        child_alive(&mut rt),
        "the runtime must survive the swap failure"
    );
}
