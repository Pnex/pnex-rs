//! Phase 0/1 acceptance — hot reload: SIGUSR1 → re-read of
//! flows.json → `redeploy_flows`, **without** restarting the process. The v2
//! graph (other payload) takes effect immediately.

mod common;

use std::time::Duration;

#[test]
fn sigusr1_recharge_le_graphe_sans_redemarrage() {
    let home = common::tmp_home("redeploy");
    let flows = home.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("version-1")).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    let started = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    let pid_v1 = started["pid"].as_u64().expect("pid present");
    rt.wait_for(
        |v| common::is_debug_with(v, "version-1"),
        Duration::from_secs(30),
    );

    // The supervisor rewrites the artifact (tmp + rename) then sends SIGUSR1.
    let tmp = home.join("flows.json.tmp");
    std::fs::write(&tmp, common::inject_debug_flows("version-2")).unwrap();
    std::fs::rename(&tmp, &flows).unwrap();
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR1);
    }

    // Farm contract: the reloaded flow first confirms with flow_started
    // (per-flow acknowledgement), then the cycle emits redeployed.
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 1,
        Duration::from_secs(30),
    );
    let redeployed = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("redeployed"),
        Duration::from_secs(30),
    );
    // No version meta in the events (multi-flow artifact — per-flow
    // versions live in the DB): only the graph fingerprint is emitted.
    assert!(redeployed["flow_rev"].is_string(), "flow_rev expected");

    // The v2 graph runs: the debug broadcasts the new payload.
    rt.wait_for(
        |v| common::is_debug_with(v, "version-2"),
        Duration::from_secs(30),
    );

    // Same process: no restart, the pid is unchanged (runtime.json
    // carries no version — process health only).
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("runtime.json")).expect("runtime.json"),
    )
    .unwrap();
    assert_eq!(state["pid"].as_u64(), Some(pid_v1));
    assert_eq!(state["redeploys"].as_u64(), Some(1));
    assert!(state.get("version_number").is_none());
}
