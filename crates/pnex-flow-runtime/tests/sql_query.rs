//! Acceptance Phase 1 (b) — un flow `inject → pnex-sql → debug` exécute une
//! **vraie requête SQL** et diffuse les lignes. Nécessite un Postgres via
//! `DATABASE_URL` (fallback `TEST_DATABASE_URL`, Postgres de compose/CI) —
//! le test est ignoré silencieusement sans base disponible.

mod common;

use std::time::Duration;

fn pg_url() -> Option<String> {
    for key in ["DATABASE_URL", "TEST_DATABASE_URL"] {
        if let Ok(u) = std::env::var(key) {
            if !u.is_empty() {
                return Some(u);
            }
        }
    }
    None
}

fn sql_flows(query: &str) -> String {
    serde_json::json!([
        { "id": "t", "type": "tab", "label": "sql" },
        {
            "id": "i1", "type": "inject", "z": "t",
            "props": [{ "p": "payload" }],
            "repeat": "1.0", "once": true, "onceDelay": 0.1,
            "payloadType": "date", "payload": "",
            "x": 100, "y": 100, "wires": [["q1"]]
        },
        {
            "id": "q1", "type": "pnex-sql", "z": "t",
            "query": query,
            "x": 200, "y": 100, "wires": [["d1"]]
        },
        {
            "id": "d1", "type": "debug", "z": "t",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "x": 300, "y": 100, "wires": []
        }
    ])
    .to_string()
}

#[test]
fn inject_pnex_sql_debug_renvoie_les_lignes() {
    let Some(url) = pg_url() else {
        eprintln!("Postgres absent (DATABASE_URL/TEST_DATABASE_URL) — test ignoré");
        return;
    };

    let home = common::tmp_home("sql");
    let flows = home.join("flows.json");
    std::fs::write(&flows, sql_flows("SELECT 42 AS answer, 'pnex' AS source")).unwrap();

    let rt = common::RuntimeProc::spawn_with_env(&flows, &home, [("DATABASE_URL", url.as_str())]);

    rt.wait_for(|v| v.get("event").and_then(|e| e.as_str()) == Some("started"), Duration::from_secs(30));
    // Le debug EdgeLinkd sérialise les tableaux en chaîne pretty : le payload
    // du nœud SQL (tableau de lignes) apparaît donc comme texte JSON.
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v["msg"].as_str().is_some_and(|m| m.contains("\"answer\": 42"))
        },
        Duration::from_secs(30),
    );
    assert!(line["msg"].as_str().unwrap().contains("\"source\": \"pnex\""));
}

#[test]
fn requete_ecriture_rejetee_au_deploiement() {
    // Sans base : le rejet a lieu au BUILD du nœud, avant toute connexion.
    let home = common::tmp_home("sqldeny");
    let flows = home.join("flows.json");
    std::fs::write(&flows, sql_flows("DELETE FROM t")).unwrap();

    let mut rt = common::RuntimeProc::spawn_with_env(&flows, &home, []);
    rt.wait_for_exit_failure(Duration::from_secs(30));
}
