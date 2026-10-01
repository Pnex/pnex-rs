//! Tests E2E d'isolation « N Engines, 1 process » : un tab invalide ne fait
//! échouer que son engine (`flow_error`), les autres continuent ; le reload
//! d'un tab n'arrête pas les autres ; le last-good continue de tourner quand
//! un swap échoue. Le harnais spawn le vrai binaire
//! (`CARGO_BIN_EXE_pnex-flow-runtime`).

mod common;

use std::time::Duration;

use common::RuntimeProc;

/// Artefact 2 tabs : flow 1 sain en fond (inject 0.1 s → debug), flow 2
/// paramétrable (payload variable, ou cassé par une wire vers un nœud
/// inexistant — fatal au build du tab, cf. vendor flow.rs
/// « Referenced node not found »).
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

    // Wire fantôme = échec de build du tab (Referenced node not found).
    if tab2_broken {
        v[4]["wires"] = serde_json::json!([["ghost"]]);
    }
    v.to_string()
}

fn enfant_vivant(rt: &mut RuntimeProc) -> bool {
    rt.child.try_wait().expect("try_wait").is_none()
}

#[test]
fn tab_invalide_ne_tue_pas_les_autres_flows() {
    let home = common::tmp_home("iso-1");
    let flows = home.join("flows.json");
    std::fs::write(&flows, two_flows("ok-2", true)).unwrap();

    let mut rt = RuntimeProc::spawn(&flows, &home);
    // Boot : flow 1 démarré, flow 2 en erreur — le process vit. Les events
    // par flow précèdent `started` (wait_for consomme sans remise).
    let started1 = rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 1,
        Duration::from_secs(30),
    );
    assert!(started1["rev"].is_string(), "rev attendu : {started1}");
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
    println!("flow 2 en erreur : {failed2}");
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // Le flow sain continue d'émettre.
    rt.wait_for(
        |v| common::is_debug_with(v, "bg-1"),
        Duration::from_secs(30),
    );
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        enfant_vivant(&mut rt),
        "le runtime doit survivre au tab invalide"
    );
}

#[test]
fn reload_d_un_seul_tab_epargne_l_autre() {
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

    // Fix du tab 2 seul (payload nouveau) — le tab 1 est inchangé.
    let tmp = home.join("flows.json.tmp");
    std::fs::write(&tmp, two_flows("fixed-2", false)).unwrap();
    std::fs::rename(&tmp, &flows).unwrap();
    unsafe {
        libc::kill(rt.child.id() as libc::pid_t, libc::SIGUSR1);
    }

    // Confirmations idempotentes : les DEUX flows émettent flow_started
    // (le 2 après swap réussi, le 1 inchangé — acquittement idempotent).
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
    // Le graphe réparé s'exécute...
    rt.wait_for(
        |v| common::is_debug_with(v, "fixed-2"),
        Duration::from_secs(30),
    );
    // ...et le flow inchangé aussi (jamais interrompu).
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
fn echec_de_swap_preserve_le_last_good() {
    let home = common::tmp_home("iso-3");
    let flows = home.join("flows.json");
    std::fs::write(&flows, two_flows("ok-2", false)).unwrap();

    let mut rt = RuntimeProc::spawn(&flows, &home);
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("flow_started") && v["flow"] == 2,
        Duration::from_secs(30),
    );

    // Le tab 1 devient invalide (wire fantôme) — tab 2 inchangé.
    let mut artifact = two_flows("ok-2", false);
    // Remplace la wires du n1 (index 1) par une référence fantôme.
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

    // Le last-good du flow 1 continue d'émettre (ingestion préservée) —
    // son engine n'a pas été remplacé.
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
        enfant_vivant(&mut rt),
        "le runtime doit survivre à l'échec de swap"
    );
}
