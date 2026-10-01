//! Tests E2E du mode `--check` (pré-flight du deploy) : construit chaque
//! engine sans `start()`, rapport par tab sur stdout, exit 0/1 — sans `--home`.

use std::process::{Command, Stdio};

mod common;

use serde_json::Value;

/// Exécute `pnex-flow-runtime --check <flows.json>` et retourne
/// (exit code, lignes JSON stdout).
fn run_check(flows: &std::path::Path) -> (std::process::ExitStatus, Vec<Value>) {
    let out = Command::new(env!("CARGO_BIN_EXE_pnex-flow-runtime"))
        .arg("--check")
        .arg(flows)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("spawn pnex-flow-runtime --check");
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    (out.status, lines)
}

fn event_of(v: &Value) -> Option<&str> {
    v.get("event").and_then(|e| e.as_str())
}

#[test]
fn check_rapporte_par_tab_et_exit_1_sur_echec() {
    // Mixte : tab 1 valide (inject → debug), tab 2 cassé (wire fantôme).
    let dir = common::tmp_home("check-mixte");
    let flows = dir.join("flows.json");
    std::fs::write(
        &flows,
        serde_json::json!([
            { "id": "pnexflow1", "type": "tab", "label": "f1", "pnex_flow_id": 1 },
            {
                "id": "n1", "type": "inject", "z": "pnexflow1",
                "props": [{ "p": "payload" }],
                "payload": "x", "payloadType": "str",
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
                "payload": "y", "payloadType": "str",
                "x": 100, "y": 100, "wires": [["ghost"]]
            }
        ])
        .to_string(),
    )
    .unwrap();

    let (status, lines) = run_check(&flows);
    assert!(!status.success(), "exit 1 attendu (un tab invalide)");

    let flows1: Vec<&Value> = lines
        .iter()
        .filter(|v| event_of(v) == Some("check_flow") && v["flow"] == 1)
        .collect();
    let flows2: Vec<&Value> = lines
        .iter()
        .filter(|v| event_of(v) == Some("check_flow") && v["flow"] == 2)
        .collect();
    assert_eq!(flows1.len(), 1);
    assert_eq!(flows1[0]["ok"], true);
    assert_eq!(flows2.len(), 1);
    assert_eq!(flows2[0]["ok"], false);
    assert!(
        flows2[0]
            .get("error")
            .and_then(|e| e.as_str())
            .is_some_and(|e| !e.is_empty()),
        "erreur moteur réelle attendue : {flows2:?}"
    );

    let done = lines
        .iter()
        .find(|v| event_of(v) == Some("check_done"))
        .expect("check_done");
    assert_eq!(done["ok"], false);
    assert_eq!(done["total"], 2);
    assert_eq!(done["errors"], 1);
}

#[test]
fn check_sur_artefact_valide_exit_0() {
    let dir = common::tmp_home("check-ok");
    let flows = dir.join("flows.json");
    std::fs::write(&flows, common::inject_debug_flows("x")).unwrap();

    let (status, lines) = run_check(&flows);
    assert!(status.success(), "exit 0 attendu");
    let done = lines
        .iter()
        .find(|v| event_of(v) == Some("check_done"))
        .expect("check_done");
    assert_eq!(done["ok"], true);
    assert_eq!(done["total"], 1);
    assert_eq!(done["errors"], 0);
}

#[test]
fn check_sur_fichier_illisible_exit_1() {
    let dir = common::tmp_home("check-bad");
    let flows = dir.join("flows.json");
    std::fs::write(&flows, "pas-du-json").unwrap();

    let (status, lines) = run_check(&flows);
    assert!(!status.success());
    let done = lines
        .iter()
        .find(|v| event_of(v) == Some("check_done"))
        .expect("check_done");
    assert_eq!(done["ok"], false);
    assert!(done.get("error").is_some(), "erreur fichier attendue");
}
