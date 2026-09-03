//! Acceptance Phase 0/1 (a) — un flow `inject → debug` tourne headless et
//! produit des événements observables, lancé/arrêté proprement (SIGINT).

mod common;

use std::time::Duration;

#[test]
fn inject_debug_tourne_headless() {
    let home = common::tmp_home("injdbg");
    let flows = home.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("bonjour")).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);

    // Le moteur annonce son démarrage puis le debug diffuse le payload.
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));
    rt.wait_for(|v| common::is_debug_with(v, "bonjour"), Duration::from_secs(30));
    // Drop = SIGINT + reap (le harnais vérifie la sortie).
}

#[test]
fn arret_propose_code_zero() {
    let home = common::tmp_home("sigint");
    let flows = home.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("x")).unwrap();

    let mut rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGINT);
    }
    let status = rt.child.wait().expect("wait après SIGINT");
    assert!(status.success(), "exit code attendu 0, obtenu : {status}");
}
