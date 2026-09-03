//! Acceptance Phase 0/1 — rechargement à chaud : SIGUSR1 → relecture du
//! flows.json → `redeploy_flows`, **sans** redémarrage du process. Le graphe
//! v2 (autre payload) prend effet immédiatement.

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
    let pid_v1 = started["pid"].as_u64().expect("pid présent");
    rt.wait_for(|v| common::is_debug_with(v, "version-1"), Duration::from_secs(30));

    // Le superviseur réécrit l'artefact (tmp + rename) puis envoie SIGUSR1.
    let tmp = home.join("flows.json.tmp");
    std::fs::write(&tmp, common::inject_debug_flows("version-2")).unwrap();
    std::fs::rename(&tmp, &flows).unwrap();
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR1);
    }

    let redeployed = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("redeployed"),
        Duration::from_secs(30),
    );
    assert_eq!(redeployed["version"], serde_json::json!(null), "flows de test sans méta de version");

    // Le graphe v2 s'exécute : le debug diffuse le nouveau payload.
    rt.wait_for(|v| common::is_debug_with(v, "version-2"), Duration::from_secs(30));

    // Même process : pas de redémarrage, le pid est inchangé (runtime.json).
    let state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(home.join("runtime.json")).expect("runtime.json"),
    )
    .unwrap();
    assert_eq!(state["pid"].as_u64(), Some(pid_v1));
    assert_eq!(state["redeploys"].as_u64(), Some(1));
}

#[test]
fn meta_de_version_lues_du_tab() {
    // Artefact projeté PNEX : le tab porte pnex_flow_id/pnex_version, que le
    // runtime doit refléter dans runtime.json et dans l'event redeployed.
    let home = common::tmp_home("meta");
    let flows = home.join("flows.json");
    let mut v: serde_json::Value = serde_json::from_str(&common::inject_debug_flows("m")).unwrap();
    v[0]["pnex_flow_id"] = serde_json::json!(12);
    v[0]["pnex_version"] = serde_json::json!(3);
    std::fs::write(&flows, v.to_string()).unwrap();

    let rt = common::RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("runtime.json")).unwrap()).unwrap();
    assert_eq!(state["flow_id"], serde_json::json!(12));
    assert_eq!(state["version_number"], serde_json::json!(3));
}
