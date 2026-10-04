//! Tests du domaine flows ETL (D18) : CRUD versionné append-only,
//! concurrence optimiste 409, cycle deploy/rollback via superviseur (faux
//! runtime — fixture `tests/fixtures/flow/fake_runtime.sh`), validation de
//! graphe, isolation org. Cf. `docs/contracts/flows.http`.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Le superviseur est un static par process : un SEUL test active
//! `PNEX_FLOW_ENABLED` (cycle deploy complet) — les autres tournent moteur
//! coupé (le deploy y répond 503, ce qui est aussi testé).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

/// Réglages env du moteur, posés AVANT le boot (config Tera lue au boot).
/// Un seul répertoire d'état par process : le superviseur vit sur tout le
/// process de test, tous les tests actifs partagent le même contrat.
fn flow_state_dir() -> String {
    format!("/tmp/pnex-flow-tests-{}", std::process::id())
}

async fn with_app_flow_engine<F, Fut>(enabled: bool, f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", if enabled { "true" } else { "false" }) };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_RUNTIME_CMD",
            "./tests/fixtures/flow/fake_runtime.sh",
        )
    };
    unsafe { std::env::set_var("PNEX_FLOW_STATE_DIR", flow_state_dir()) };
    unsafe { std::env::set_var("PNEX_FLOW_RELOAD_ACK_SECS", "5") };
    // Outils de debug : ON quand le moteur tourne (tests du feed debug),
    // sinon OFF pour vérifier la garde 403 « mode run ».
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_DEBUG_TOOLS",
            if enabled { "true" } else { "false" },
        )
    };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let env = Env {
        alice: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
        bob: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        ),
    };
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env, ctx).await;
        },
    )
    .await;
}

struct Env {
    alice: String,
    bob: String,
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org personnelle")
}

/// Graphe inject → debug paramétrable (l'intervalle matérialise la version).
fn graph_inject_debug(repeat: f64) -> serde_json::Value {
    serde_json::json!({
        "nodes": [
            {
                "id": "n1", "kind": "inject",
                "config": { "repeat_secs": repeat, "payload": {"k": 1} },
                "outputs": [{ "port": 0, "targets": ["n2"] }]
            },
            { "id": "n2", "kind": "debug", "config": {} }
        ]
    })
}

async fn create_flow(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    name: &str,
    repeat: f64,
) -> serde_json::Value {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "graph": graph_inject_debug(repeat),
        }))
        .await;
    // Keep the raw status + body in the failure output: the CI-only
    // empty-body failures need the real payload to be diagnosed.
    assert_eq!(
        res.status_code(),
        201,
        "create {name}: status {} body {}",
        res.status_code(),
        res.text()
    );
    res.json::<serde_json::Value>()
}

async fn read_state_flows_json() -> serde_json::Value {
    let raw = std::fs::read_to_string(format!("{}/flows.json", flow_state_dir()))
        .expect("flows.json projeté");
    serde_json::from_str(&raw).expect("flows.json valide")
}

// ─────────────────────────── CRUD versionné ───────────────────────────

#[tokio::test]
#[serial]
async fn cycle_creation_versioning_et_isolation_org() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Création : 201, version 1, statut draft.
        let created = create_flow(&server, &env.alice, org, "pipeline température", 1.0).await;
        assert_eq!(created["status"], "draft", "{created}");
        assert_eq!(created["latest_version_number"], 1);
        assert_eq!(created["deployed_version_number"], serde_json::Value::Null);
        assert_eq!(created["graph"]["nodes"][0]["kind"], "inject");
        let flow_id = created["id"].as_i64().unwrap();

        // Édition v2 : sans moteur, sans reload (aucun artifact écrit).
        let updated = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "graph": graph_inject_debug(2.0),
                "author": "alice",
                "note": "double la fréquence",
            }))
            .await;
        assert_eq!(updated.status_code(), 200, "{}", updated.text());
        let v2 = updated.json::<serde_json::Value>();
        assert_eq!(v2["latest_version_number"], 2);
        assert_eq!(v2["graph"]["nodes"][0]["config"]["repeat_secs"], 2.0);

        // Historique : 2 versions, ordre descendant, v2 déployée=false.
        let versions = server
            .get(&format!("/api/v1/flows/{flow_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(versions["count"], 2, "{versions}");
        assert_eq!(versions["results"][0]["version_number"], 2);
        assert_eq!(versions["results"][0]["note"], "double la fréquence");
        assert_eq!(versions["results"][1]["version_number"], 1);

        // Détail d'une version précise (graphe historisé).
        let v1 = server
            .get(&format!("/api/v1/flows/{flow_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(v1["graph"]["nodes"][0]["config"]["repeat_secs"], 1.0);

        // Isolation org : bob ne voit pas le flow d'alice (404, pas 403).
        let org_bob = personal_org(&server, &env.bob).await;
        let foreign = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await;
        assert_eq!(foreign.status_code(), 404, "{}", foreign.text());

        // Liste paginée D14 : le flow d'alice n'apparaît pas côté bob.
        let list_bob = server
            .get("/api/v1/flows")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list_bob["count"], 0, "{list_bob}");

        // Suppression : 204, historique parti.
        let deleted = server
            .delete(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204);
        let gone = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(gone.status_code(), 404);
    })
    .await;
}

// ─────────────────────────── Concurrence optimiste (e) ───────────────────────────

#[tokio::test]
#[serial]
async fn save_perime_rejete_409_et_aucune_v3() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "concurrent", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // Deux clients éditent depuis la v1.
        let payload_v2 = serde_json::json!({
            "expected_version_number": 1,
            "graph": graph_inject_debug(2.0),
        });
        let first = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&payload_v2)
            .await;
        assert_eq!(first.status_code(), 200, "{}", first.text());

        // Le second save (toujours basé v1) est rejeté 409.
        let second = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&payload_v2)
            .await;
        assert_eq!(second.status_code(), 409, "{}", second.text());

        // Aucune v3 écrite.
        let versions = server
            .get(&format!("/api/v1/flows/{flow_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(versions["count"], 2, "{versions}");
    })
    .await;
}

// ─────────────────────────── Validation du graphe (f) ───────────────────────────

#[tokio::test]
#[serial]
async fn validation_rejette_graphe_invalide() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // The removed `pnex_sql` node kind is rejected cleanly (no panic).
        let removed_kind = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "removed sql node",
                "graph": { "nodes": [
                    { "id": "q", "kind": "pnex_sql",
                      "config": { "query": "SELECT 1" } },
                    { "id": "d", "kind": "debug", "config": {},
                      "outputs": [] },
                ]},
            }))
            .await;
        assert!(
            removed_kind.status_code().is_client_error(),
            "{} {}",
            removed_kind.status_code(),
            removed_kind.text()
        );

        // Graphe incohérent : id dupliqué + câblage vers un nœud inconnu.
        let bad_graph = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "graphe cassé",
                "graph": { "nodes": [
                    { "id": "n1", "kind": "inject",
                      "config": { "repeat_secs": 5.0 },
                      "outputs": [{ "port": 0, "targets": ["ghost"] }] },
                    { "id": "n1", "kind": "debug", "config": {} },
                ]},
            }))
            .await;
        assert_eq!(bad_graph.status_code(), 400, "{}", bad_graph.text());
        let body = bad_graph.json::<serde_json::Value>();
        let codes: Vec<&str> = body["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"duplicate_node_id"), "{body}");
        assert!(codes.contains(&"dangling_target"), "{body}");

        // inject sans déclencheur : rejeté.
        let no_trigger = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "sans déclencheur",
                "graph": { "nodes": [
                    { "id": "n1", "kind": "inject", "config": {} },
                    { "id": "n2", "kind": "debug", "config": {} },
                ]},
            }))
            .await;
        assert_eq!(no_trigger.status_code(), 400, "{}", no_trigger.text());

        // SEC-1 / SEC-2: a raw Red node can target neither a host-effect
        // builtin nor a PNeX node type.
        for forbidden in ["exec", "template", "pnex-device-write"] {
            let red = server
                .post("/api/v1/flows")
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "name": "red forbidden",
                    "graph": { "nodes": [
                        { "id": "x", "kind": "red", "type_name": forbidden,
                          "config": { "command": "id", "pnex_org_id": 1 } },
                    ]},
                }))
                .await;
            assert_eq!(red.status_code(), 400, "{forbidden}: {}", red.text());
            let body = red.json::<serde_json::Value>();
            assert_eq!(
                body["violations"][0]["code"], "red_type_forbidden",
                "{forbidden}: {body}"
            );
        }
    })
    .await;
}

// ─────────────────────────── Moteur coupé : 503 ───────────────────────────

#[tokio::test]
#[serial]
async fn deploy_sans_moteur_repond_503() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "jamais déployé", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // Garde « mode run » : les outils de debug sont désactivés (env
        // PNEX_FLOW_DEBUG_TOOLS=false de ce process) — 403 AVANT toute
        // considération de moteur.
        let dbg = server
            .get(&format!("/api/v1/flows/{flow_id}/debug"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(dbg.status_code(), 403, "{}", dbg.text());

        let res = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 503, "{}", res.text());
        let body = res.json::<serde_json::Value>();
        // Corps Loco CustomError : {"error": code, "description": message}.
        assert_eq!(body["error"], "flow_runtime", "{body}");
    })
    .await;
}

// ─────────────────────────── Cycle deploy (a)(c)(d) ───────────────────────────

/// Le cycle complet deploy/édition/recharge/rollback avec le faux runtime.
/// Un seul test active le superviseur (static par process — cf. module doc).
#[tokio::test]
#[serial]
async fn cycle_deploy_edit_rollback_avec_runtime() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "pipeline complet", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // (c) Édition v2 : AUCUN rechargement — l'artefact au disque (s'il
        // existe déjà : répertoire d'état partagé entre tests) reste identique.
        let before_save = std::fs::read_to_string(format!("{}/flows.json", flow_state_dir())).ok();
        let updated = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "graph": graph_inject_debug(2.0),
            }))
            .await;
        assert_eq!(updated.status_code(), 200, "{}", updated.text());
        assert_eq!(
            std::fs::read_to_string(format!("{}/flows.json", flow_state_dir())).ok(),
            before_save,
            "un save ne doit pas écrire l'artefact"
        );

        // (a) Deploy v1 : artefact projeté + runtime enfant vivant.
        let deployed = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 1 }))
            .await;
        assert_eq!(deployed.status_code(), 200, "{}", deployed.text());
        let dto = deployed.json::<serde_json::Value>();
        assert_eq!(dto["status"], "deployed");
        assert_eq!(dto["deployed_version_number"], 1);

        let artifact = read_state_flows_json().await;
        let tab = &artifact[0];
        assert_eq!(tab["type"], "tab");
        assert_eq!(tab["pnex_flow_id"], flow_id);
        assert_eq!(tab["pnex_version"], 1);
        // inject v1 : l'intervalle projeté matérialise la version déployée.
        let inject = artifact
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "inject")
            .unwrap();
        assert_eq!(inject["repeat"], 1.0, "{inject}");

        // Runtime enfant vivant et rapporté par l'API — santé **par flow** :
        // l'engine de CE flow est `running` (événement flow_started de la
        // fixture), la version vient de la DB.
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["running"], true, "{runtime}");
        assert_eq!(runtime["engine_status"], "running", "{runtime}");
        assert_eq!(runtime["last_error"], serde_json::Value::Null, "{runtime}");

        // (c) Deploy v2 : rechargement, l'artefact porte la version 2.
        let deployed_v2 = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 2 }))
            .await;
        assert_eq!(deployed_v2.status_code(), 200, "{}", deployed_v2.text());
        let artifact = read_state_flows_json().await;
        assert_eq!(artifact[0]["pnex_version"], 2);
        let inject = artifact
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "inject")
            .unwrap();
        assert_eq!(inject["repeat"], 2.0, "{inject}");

        // (d) Rollback v1 : l'ancien graphe revient en exécution.
        let rolled = server
            .post(&format!("/api/v1/flows/{flow_id}/rollback"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 1 }))
            .await;
        assert_eq!(rolled.status_code(), 200, "{}", rolled.text());
        let artifact = read_state_flows_json().await;
        assert_eq!(artifact[0]["pnex_version"], 1);
        let inject = artifact
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "inject")
            .unwrap();
        assert_eq!(inject["repeat"], 1.0, "{inject}");

        // Version inconnue : 404.
        let missing = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 99 }))
            .await;
        assert_eq!(missing.status_code(), 404);

        // Deploy sans body → dernière version (2... rollback a remis 1 en
        // déployée, la dernière VERSION reste la 2).
        let latest = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(latest.status_code(), 200, "{}", latest.text());
        assert_eq!(
            latest.json::<serde_json::Value>()["deployed_version_number"],
            2
        );

        // ── Panneau debug (mode dev/debug actif : PNEX_FLOW_DEBUG_TOOLS=true)
        // runtime porte l'activation — c'est ce que lit l'éditeur pour
        // afficher/masquer les boutons.
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["debug_tools"], true, "{runtime}");
        // Multi-user awareness fields polled by the editor.
        assert_eq!(runtime["latest_version_number"], 2, "{runtime}");
        assert_eq!(runtime["flow_status"], "deployed", "{runtime}");

        // Fixture émettant des lignes debug attribuées (flow dérivé de
        // l'artefact) : le feed ne doit pas être vide et rester scoped.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let feed = server
            .get(&format!("/api/v1/flows/{flow_id}/debug"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(feed.status_code(), 200, "{}", feed.text());
        let feed = feed.json::<serde_json::Value>();
        let entries = feed["entries"].as_array().expect("entries");
        assert!(!entries.is_empty(), "feed vide : {feed}");
        assert_eq!(entries[0]["flow_id"], flow_id);
        assert_eq!(entries[0]["node_id"], "n2");
        assert_eq!(entries[0]["source"], "debug");

        // Hors-org : 404 (le feed d'Alice n'est pas lisible par Bob).
        let bob_org = personal_org(&server, &env.bob).await;
        let cross = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(cross.status_code(), 404, "{}", cross.text());

        // (f) Pré-flight au deploy : un flow dont le tab est rejeté par le
        // runtime (allowlisted `red` comment node carrying the fixture's
        // `pnex-check-fail` marker) répond **400** avec la violation `engine_load` (erreur
        // moteur réelle), sans marquer la DB (reste draft) ni toucher
        // l'artefact en exécution — le flow sain continue de tourner.
        // NB : la création passe (pas de type-check global, garde-fou PRD).
        let invalid = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "invalide moteur",
                "graph": {
                    "nodes": [
                        { "id": "x1", "kind": "red", "type_name": "comment", "config": { "info": "pnex-check-fail" } }
                    ]
                }
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(invalid["status"], "draft", "{invalid}");
        let invalid_id = invalid["id"].as_i64().unwrap();

        let rejected = server
            .post(&format!("/api/v1/flows/{invalid_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(rejected.status_code(), 400, "{}", rejected.text());
        let body = rejected.json::<serde_json::Value>();
        let violations = body["violations"].as_array().expect("violations");
        assert_eq!(violations.len(), 1, "{body}");
        assert_eq!(violations[0]["code"], "engine_load", "{body}");
        assert!(
            violations[0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("Flow #")),
            "message moteur réel attendu : {body}"
        );

        // DB intacte : le flow invalide reste draft (aucune version déployée
        // — le prisme `deployed_flow_id` reflète l'id demandé, jamais une
        // version déployée).
        let detail = server
            .get(&format!("/api/v1/flows/{invalid_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(
            detail["deployed_version_number"],
            serde_json::Value::Null,
            "{detail}"
        );

        // L'artefact en exécution est intact (tab du flow invalide jamais
        // écrit) et le flow sain tourne toujours.
        let artifact = read_state_flows_json().await;
        assert!(
            artifact
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["id"] != format!("pnexflow{invalid_id}")),
            "le tab invalide ne doit pas être projeté : {artifact}"
        );
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["running"], true, "{runtime}");
        assert_eq!(runtime["engine_status"], "running", "{runtime}");

        // ─── Stop / Start / Restart (ferme : le tab quitte et revient) ───
        // Stop acquitté par `flow_stopped` du faux runtime : DB `stopped`
        // (version déployée conservée) et tab absent de l'artefact.
        let stop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(stop.status_code(), 200, "{}", stop.text());
        let detail = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(detail["status"], "stopped", "{detail}");
        assert!(
            detail["deployed_version_number"].is_i64(),
            "la version déployée doit être conservée au stop : {detail}"
        );
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["engine_status"], "stopped", "{runtime}");
        let artifact = read_state_flows_json().await;
        assert!(
            artifact
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["id"] != format!("pnexflow{flow_id}")),
            "le tab stoppé doit sortir de l'artefact : {artifact}"
        );

        // Re-stop : 409 (seul un flow déployé peut être arrêté).
        let restop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(restop.status_code(), 409, "{}", restop.text());

        // Start : reprise sur la version déployée (tab de retour, engine
        // running, DB `deployed`).
        let start = server
            .post(&format!("/api/v1/flows/{flow_id}/start"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(start.status_code(), 200, "{}", start.text());
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["engine_status"], "running", "{runtime}");
        let artifact = read_state_flows_json().await;
        assert!(
            artifact
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == format!("pnexflow{flow_id}")),
            "le tab démaré doit être de retour dans l'artefact : {artifact}"
        );

        // Restart : stop+start chaînés — l'engine finit running à la même
        // version déployée.
        let restart = server
            .post(&format!("/api/v1/flows/{flow_id}/restart"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(restart.status_code(), 200, "{}", restart.text());
        let detail = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(detail["status"], "deployed", "{detail}");
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["engine_status"], "running", "{runtime}");

        // ─── Delete d'un flow **déployé** (le dernier de l'instance) ───
        // 204 rapide : l'ack du retrait est keyé sur le flow supprimé (le
        // faux runtime émet `flow_stopped` pour le tab disparu). Régression :
        // méta « à vide » (flow_id 0) = ack jamais résolu → 503 « acquitte-
        // ment de rechargement absent » après une suppression pourtant déjà
        // appliquée en base, et liste UI figée jusqu'au refresh manuel.
        let deleted = server
            .delete(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204, "{}", deleted.text());
        let gone = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(gone.status_code(), 404);
        let artifact = read_state_flows_json().await;
        assert_eq!(
            artifact,
            serde_json::json!([]),
            "artefact vidé par le delete : {artifact}"
        );
    })
    .await;
}

// ─────────────────────────── Delete of a stopped flow ───────────────────────────

/// A stopped flow keeps its deployed version but is not running: deleting
/// it must not wait for a `flow_stopped` ack the runtime never sends (the
/// delete used to take the whole reload-ack timeout).
#[tokio::test]
#[serial]
async fn delete_stopped_flow_does_not_wait_for_runtime_ack() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "stopped then deleted", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();
        let deployed = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 1 }))
            .await;
        assert_eq!(deployed.status_code(), 200, "{}", deployed.text());
        let stop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(stop.status_code(), 200, "{}", stop.text());

        let started = std::time::Instant::now();
        let deleted = server
            .delete(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204, "{}", deleted.text());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "delete of a stopped flow took {:?}",
            started.elapsed()
        );
    })
    .await;
}

// ─────────────────────────── Stop/Start : gardes sans moteur ───────────────────────────

#[tokio::test]
#[serial]
async fn stop_start_gardes_sans_moteur() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "jamais déployé", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // Stop sur draft : 409 (seul un flow déployé peut être arrêté) —
        // refus AVANT toute interaction moteur.
        let stop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(stop.status_code(), 409, "{}", stop.text());

        // Start sur un flow jamais déployé : 409.
        let start = server
            .post(&format!("/api/v1/flows/{flow_id}/start"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(start.status_code(), 409, "{}", start.text());

        // Restart sur un flow jamais déployé : 409.
        let restart = server
            .post(&format!("/api/v1/flows/{flow_id}/restart"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(restart.status_code(), 409, "{}", restart.text());
    })
    .await;
}
