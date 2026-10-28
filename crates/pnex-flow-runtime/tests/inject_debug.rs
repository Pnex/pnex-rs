//! Phase 0/1 (a) acceptance — an `inject → debug` flow runs headless and
//! produces observable events, started/stopped cleanly (SIGINT).

mod common;

use std::time::Duration;

#[test]
fn inject_debug_runs_headless() {
    let home = common::tmp_home("injdbg");
    let flows = home.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("bonjour")).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);

    // The engine announces its start then the debug broadcasts the payload.
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    rt.wait_for(
        |v| common::is_debug_with(v, "bonjour"),
        Duration::from_secs(30),
    );
    // Drop = SIGINT + reap (the harness checks the exit).
}

#[test]
fn stop_proposes_exit_code_zero() {
    let home = common::tmp_home("sigint");
    let flows = home.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("x")).unwrap();

    let mut rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = rt.child.wait().expect("wait after SIGINT");
    assert!(status.success(), "exit code 0 expected, got: {status}");
}
