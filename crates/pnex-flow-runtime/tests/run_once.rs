//! Tests E2E du run-once : `cmd.json` + SIGUSR2 → inject dans les cibles,
//! acks corrélés par seq, attribution multi-tabs. Le harnais spawn le vrai
//! binaire (`CARGO_BIN_EXE_pnex-flow-runtime`).

mod common;

use std::path::Path;
use std::time::Duration;

use common::RuntimeProc;

/// Artefact RAW Node-RED : tab `pnexflow1` + inject **sans déclencheur**
/// (manual-only — toléré par le moteur) + payload JSON → debug. Ce flow
/// prouve aussi qu'un artefact sans déclencheur se déploie.
fn manual_flows() -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "test", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "payload": "{\"k\":1}", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 200, "y": 100, "wires": []
        }
    ])
    .to_string()
}

/// Écrit `cmd.json` dans le home du runtime (contrat superviseur : posé
/// avant le signal).
fn write_cmd(home: &Path, seq: u64, flow: &str) {
    std::fs::write(
        home.join("cmd.json"),
        serde_json::json!({ "seq": seq, "flow": flow }).to_string(),
    )
    .expect("écriture cmd.json");
}

#[test]
fn run_once_injecte_dans_les_cibles() {
    let home = common::tmp_home("runonce");
    let flows = home.join("flows.json");
    std::fs::write(&flows, manual_flows()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));

    // Commande posée avant le signal (contrat superviseur).
    write_cmd(&home, 1, "pnexflow1");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };

    // Ack corrélé : 1 inject, 1 injection (cible debug).
    let ack = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_done") && v["seq"] == 1,
        Duration::from_secs(30),
    );
    assert_eq!(ack["nodes"], 1, "{ack}");
    assert_eq!(ack["injected"], 1, "{ack}");
    assert_eq!(ack["flow"], "pnexflow1", "{ack}");

    // Le debug en aval a bien reçu le payload reconstruit (attribution
    // runtime : flow + id éditeur). NB : le debug builtin pré-stringifie sa
    // sortie (`format_message_for_display`) — la valeur arrive en chaîne.
    let dbg = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.get("node_red").and_then(|n| n.as_str()) == Some("n2")
                && v.get("flow").and_then(|f| f.as_i64()) == Some(1)
                && v.to_string().contains("k")
        },
        Duration::from_secs(30),
    );
    assert!(
        dbg["msg"].is_string() && dbg["msg"].as_str().unwrap().contains("\"k\""),
        "payload stringifié attendu : {dbg}"
    );

    // Le fichier de commande est supprimé après exécution (anti-rejeu).
    assert!(!home.join("cmd.json").exists(), "cmd.json doit être supprimé");
}

#[test]
fn run_once_sans_cmd_json_echoue() {
    let home = common::tmp_home("runonce-nocmd");
    let flows = home.join("flows.json");
    std::fs::write(&flows, manual_flows()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));

    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };
    let ack = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_failed") && v["seq"] == 0,
        Duration::from_secs(30),
    );
    assert_eq!(ack["error"], "cmd_illisible", "{ack}");
}

#[test]
fn run_once_flow_absente() {
    let home = common::tmp_home("runonce-absent");
    let flows = home.join("flows.json");
    std::fs::write(&flows, manual_flows()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));

    write_cmd(&home, 2, "pnexflow999");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };
    let ack = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_failed") && v["seq"] == 2,
        Duration::from_secs(30),
    );
    assert_eq!(ack["error"], "flow_absent", "{ack}");
}

#[test]
fn run_once_rejeu_seq_ignoree() {
    let home = common::tmp_home("runonce-replay");
    let flows = home.join("flows.json");
    std::fs::write(&flows, manual_flows()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));

    // 1re exécution : seq 1 → done.
    write_cmd(&home, 1, "pnexflow1");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_done") && v["seq"] == 1,
        Duration::from_secs(30),
    );

    // Rejeu : même seq ré-écrite + signal → AUCUN ack (idempotence). On
    // attend 1 s puis on prouve que le runtime répond encore sur une seq
    // fraîche (seq 2 → done) : le canal d'acks est resté sain.
    write_cmd(&home, 1, "pnexflow1");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };
    std::thread::sleep(Duration::from_secs(1));

    write_cmd(&home, 2, "pnexflow1");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };
    let ack = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_done") && v["seq"] == 2,
        Duration::from_secs(30),
    );
    assert_eq!(ack["injected"], 1, "{ack}");
}

#[test]
fn attribution_multi_tabs() {
    let home = common::tmp_home("runonce-tabs");
    let flows = home.join("flows.json");
    // Tab 1 : inject intervalle (0.1 s) → debug — trafic de fond attribué
    // flow=1. Tab 2 : inject manuel → debug — ne vit que par run-once.
    let artifact = serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "f1", "pnex_flow_id": 1 },
        {
            "id": "n1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "0.1", "payload": "\"bg\"", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["n2"]]
        },
        {
            "id": "n2", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "complete": "payload",
            "x": 200, "y": 100, "wires": []
        },
        { "id": "pnexflow2", "type": "tab", "label": "f2", "pnex_flow_id": 2 },
        {
            "id": "m1", "type": "inject", "z": "pnexflow2",
            "props": [{ "p": "payload" }],
            "payload": "{\"tab\":2}", "payloadType": "json",
            "x": 100, "y": 100, "wires": [["m2"]]
        },
        {
            "id": "m2", "type": "debug", "z": "pnexflow2",
            "active": true, "tosidebar": true, "complete": "payload",
            "x": 200, "y": 100, "wires": []
        }
    ]);
    std::fs::write(&flows, artifact.to_string()).unwrap();

    let rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));

    // Run-once sur le tab 2 uniquement.
    write_cmd(&home, 5, "pnexflow2");
    unsafe { libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR2) };

    let ack = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("run_once_done") && v["seq"] == 5,
        Duration::from_secs(30),
    );
    assert_eq!(ack["nodes"], 1, "{ack}");
    assert_eq!(ack["injected"], 1, "{ack}");

    // Le debug du tab 2 est attribué flow=2 avec l'id éditeur m2 (sortie
    // builtin stringifiée — cf. run_once_injecte_dans_les_cibles).
    let dbg = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.get("flow").and_then(|f| f.as_i64()) == Some(2)
                && v.get("node_red").and_then(|n| n.as_str()) == Some("m2")
                && v.to_string().contains("tab")
        },
        Duration::from_secs(30),
    );
    assert!(
        dbg["msg"].is_string() && dbg["msg"].as_str().unwrap().contains("\"tab\""),
        "payload stringifié attendu : {dbg}"
    );
}
