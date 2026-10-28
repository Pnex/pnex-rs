//! ETL flows domain tests (D18): append-only versioned CRUD,
//! 409 optimistic concurrency, deploy/rollback cycle via the supervisor
//! (fake runtime — fixture `tests/fixtures/flow/fake_runtime.sh`), graph
//! validation, org isolation. Cf. `docs/contracts/flows.http`.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.
//! The supervisor is a per-process static: a SINGLE test enables
//! `PNEX_FLOW_ENABLED` (full deploy cycle) — the others run with the engine
//! off (deploy answers 503 there, which is tested too).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

/// Engine env settings, set BEFORE boot (Tera config read at boot).
/// A single state directory per process: the supervisor lives for the whole
/// test process, all active tests share the same contract.
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
    // Debug tools: ON when the engine runs (debug feed tests),
    // otherwise OFF to check the 403 "run mode" guard.
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
        .expect("personal org")
}

/// Configurable inject → debug graph (the interval materialises the version).
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
        .expect("flows.json projected");
    serde_json::from_str(&raw).expect("valid flows.json")
}

// ─────────────────────────── versioned CRUD ───────────────────────────

#[tokio::test]
#[serial]
async fn create_versioning_cycle_and_org_isolation() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Creation: 201, version 1, draft status.
        let created = create_flow(&server, &env.alice, org, "pipeline température", 1.0).await;
        assert_eq!(created["status"], "draft", "{created}");
        assert_eq!(created["latest_version_number"], 1);
        assert_eq!(created["deployed_version_number"], serde_json::Value::Null);
        assert_eq!(created["graph"]["nodes"][0]["kind"], "inject");
        let flow_id = created["id"].as_i64().unwrap();

        // Edit v2: no engine, no reload (no artifact written).
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

        // History: 2 versions, descending order, v2 deployed=false.
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

        // Detail of a specific version (historised graph).
        let v1 = server
            .get(&format!("/api/v1/flows/{flow_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(v1["graph"]["nodes"][0]["config"]["repeat_secs"], 1.0);

        // Org isolation: bob does not see alice's flow (404, not 403).
        let org_bob = personal_org(&server, &env.bob).await;
        let foreign = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await;
        assert_eq!(foreign.status_code(), 404, "{}", foreign.text());

        // D14 paginated list: alice's flow does not show up for bob.
        let list_bob = server
            .get("/api/v1/flows")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_bob.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list_bob["count"], 0, "{list_bob}");

        // Deletion: 204, history gone.
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

// ─────────────────────────── Optimistic concurrency (e) ───────────────────────────

#[tokio::test]
#[serial]
async fn stale_save_rejected_409_and_no_v3() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "concurrent", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // Two clients edit from v1.
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

        // The second save (still based on v1) is rejected 409.
        let second = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&payload_v2)
            .await;
        assert_eq!(second.status_code(), 409, "{}", second.text());

        // No v3 written.
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

// ─────────────────────────── Graph validation (f) ───────────────────────────

#[tokio::test]
#[serial]
async fn validation_rejects_invalid_graph() {
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

        // Inconsistent graph: duplicate id + wiring to an unknown node.
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
        let codes: Vec<&str> = body["errors"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"duplicate_node_id"), "{body}");
        assert!(codes.contains(&"dangling_target"), "{body}");

        // inject without a trigger: rejected.
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

        // SEC-1 / SEC-2: raw Node-RED nodes are gone; the former `red`
        // escape hatch (exec, template, pnex-* forgery) is refused outright.
        let red = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "red forbidden",
                "graph": { "nodes": [
                    { "id": "x", "kind": "red", "type_name": "exec",
                      "config": { "command": "id", "pnex_org_id": 1 } },
                ]},
            }))
            .await;
        assert!(
            red.status_code().is_client_error(),
            "{}: {}",
            red.status_code(),
            red.text()
        );
    })
    .await;
}

// ─────────────────────────── Engine off: 503 ───────────────────────────

#[tokio::test]
#[serial]
async fn deploy_without_engine_answers_503() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "jamais déployé", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // "Run mode" guard: the debug tools are disabled (env
        // PNEX_FLOW_DEBUG_TOOLS=false for this process) — 403 BEFORE any
        // engine consideration.
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
        // Coded error, the engine diagnostic in `args.detail`.
        assert_eq!(body["error"], "flow-runtime", "{body}");
        assert!(body["errors"]["args"]["detail"].is_string(), "{body}");
    })
    .await;
}

// ─────────────────────────── Cycle deploy (a)(c)(d) ───────────────────────────

/// The full deploy/edit/reload/rollback cycle with the fake runtime.
/// A single test enables the supervisor (per-process static — see module doc).
#[tokio::test]
#[serial]
async fn deploy_edit_rollback_cycle_with_runtime() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "pipeline complet", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // (c) Edit v2: NO reload — the artifact on disk (if it already
        // exists: state directory shared between tests) stays identical.
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
            "a save must not write the artifact"
        );

        // (a) Deploy v1: projected artifact + live child runtime.
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
        // inject v1: the projected interval materialises the deployed version.
        let inject = artifact
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["type"] == "inject")
            .unwrap();
        assert_eq!(inject["repeat"], 1.0, "{inject}");

        // Child runtime alive and reported by the API — health **per flow**:
        // the engine of THIS flow is `running` (flow_started event from the
        // fixture), the version comes from the DB.
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["running"], true, "{runtime}");
        assert_eq!(runtime["engine_status"], "running", "{runtime}");
        assert_eq!(runtime["last_error"], serde_json::Value::Null, "{runtime}");

        // (c) Deploy v2: reload, the artifact carries version 2.
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

        // (d) Rollback v1: the old graph runs again.
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

        // Unknown version: 404.
        let missing = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "version_number": 99 }))
            .await;
        assert_eq!(missing.status_code(), 404);

        // Deploy without body → latest version (2... rollback set 1 back as
        // deployed, the latest VERSION is still 2).
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

        // ── Debug panel (dev/debug mode on: PNEX_FLOW_DEBUG_TOOLS=true)
        // runtime carries the activation — that is what the editor reads to
        // show/hide the buttons.
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

        // Fixture emitting attributed debug lines (flow derived from the
        // artifact): the feed must not be empty and must stay scoped.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let feed = server
            .get(&format!("/api/v1/flows/{flow_id}/debug"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(feed.status_code(), 200, "{}", feed.text());
        let feed = feed.json::<serde_json::Value>();
        let entries = feed["entries"].as_array().expect("entries");
        assert!(!entries.is_empty(), "empty feed: {feed}");
        assert_eq!(entries[0]["flow_id"], flow_id);
        assert_eq!(entries[0]["node_id"], "n2");
        assert_eq!(entries[0]["source"], "debug");

        // Out of org: 404 (Alice's feed is not readable by Bob).
        let bob_org = personal_org(&server, &env.bob).await;
        let cross = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(cross.status_code(), 404, "{}", cross.text());

        // (f) Pre-flight at deploy: a flow whose tab is rejected by the
        // runtime (debug node named with the fixture's `pnex-check-fail`
        // marker) answers **400** with the `engine_load` violation (real
        // engine error), without marking the DB (stays draft) nor touching
        // the running artifact — the healthy flow keeps running.
        // NB: creation passes (no global type-check, PRD safeguard).
        let invalid = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "invalide moteur",
                "graph": {
                    "nodes": [
                        { "id": "x1", "kind": "debug", "name": "pnex-check-fail", "config": {} }
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
        let violations = body["errors"]["violations"].as_array().expect("violations");
        assert_eq!(violations.len(), 1, "{body}");
        assert_eq!(violations[0]["code"], "engine_load", "{body}");
        assert!(
            violations[0]["message"]
                .as_str()
                .is_some_and(|m| m.contains("Flow #")),
            "real engine message expected: {body}"
        );

        // DB intact: the invalid flow stays draft (no deployed version
        // — the `deployed_flow_id` prism reflects the requested id, never a
        // deployed version).
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

        // The running artifact is intact (tab of the invalid flow never
        // written) and the healthy flow still runs.
        let artifact = read_state_flows_json().await;
        assert!(
            artifact
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["id"] != format!("pnexflow{invalid_id}")),
            "the invalid tab must not be projected: {artifact}"
        );
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["running"], true, "{runtime}");
        assert_eq!(runtime["engine_status"], "running", "{runtime}");

        // ─── Stop / Start / Restart (closed: the tab leaves and comes back) ───
        // Stop acknowledged by the fake runtime's `flow_stopped`: DB `stopped`
        // (deployed version kept) and tab absent from the artifact.
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
            "the deployed version must be kept on stop: {detail}"
        );
        let runtime = server
            .get(&format!("/api/v1/flows/{flow_id}/runtime"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(runtime["engine_status"], "stopped", "{runtime}");
        // The shared runtime process is still alive, the stopped flow is not (O60).
        assert_eq!(runtime["running"], false, "{runtime}");
        let artifact = read_state_flows_json().await;
        assert!(
            artifact
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["id"] != format!("pnexflow{flow_id}")),
            "the stopped tab must leave the artifact: {artifact}"
        );

        // Re-stop: 409 (only a deployed flow can be stopped).
        let restop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(restop.status_code(), 409, "{}", restop.text());

        // Start: resumes on the deployed version (tab back, engine
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
            "the started tab must be back in the artifact: {artifact}"
        );

        // Restart: chained stop+start — the engine ends up running at the
        // same deployed version.
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

        // ─── Delete of a **deployed** flow (the last one of the instance) ───
        // Quick 204: the removal ack is keyed on the deleted flow (the
        // fake runtime emits `flow_stopped` for the vanished tab). Regression:
        // "empty" meta (flow_id 0) = ack never resolved → 503 "reload
        // acknowledgement missing" after a deletion already applied in the
        // database, and UI list frozen until a manual refresh.
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
            "artifact emptied by the delete: {artifact}"
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

// ─────────────────────────── Stop/Start: guards without an engine ───────────────────────────

#[tokio::test]
#[serial]
async fn stop_start_guards_without_engine() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_flow(&server, &env.alice, org, "jamais déployé", 1.0).await;
        let flow_id = created["id"].as_i64().unwrap();

        // Stop on a draft: 409 (only a deployed flow can be stopped) —
        // refused BEFORE any engine interaction.
        let stop = server
            .post(&format!("/api/v1/flows/{flow_id}/stop"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(stop.status_code(), 409, "{}", stop.text());

        // Start on a never-deployed flow: 409.
        let start = server
            .post(&format!("/api/v1/flows/{flow_id}/start"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(start.status_code(), 409, "{}", start.text());

        // Restart on a never-deployed flow: 409.
        let restart = server
            .post(&format!("/api/v1/flows/{flow_id}/restart"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(restart.status_code(), 409, "{}", restart.text());
    })
    .await;
}

/// D359: a stored graph holding a retired (`ingest`) or unknown node still
/// reads, lists and deletes; the node is left out with its wires and
/// reported, and the deploy is refused with `flow-node-retired`.
#[tokio::test]
#[serial]
async fn flow_with_retired_node_reads_lists_and_deletes() {
    with_app_flow_engine(false, |server, env, ctx| async move {
        use pnex_backend::models::_entities::flow_versions;
        use sea_orm::{ActiveModelTrait, Set};
        let org = personal_org(&server, &env.alice).await;
        let flow_id = create_flow(&server, &env.alice, org, "legacy", 1.0).await["id"]
            .as_i64()
            .unwrap();
        let stored = serde_json::json!({ "nodes": [
            { "id": "n1", "kind": "inject", "config": { "repeat_secs": 1.0 },
              "outputs": [{ "port": 0, "targets": ["n2", "old"] }, { "port": 1, "targets": ["odd"] }] },
            { "id": "old", "kind": "ingest", "config": { "step": "source_resolve" } },
            { "id": "odd", "kind": "no-such-kind" },
            { "id": "n2", "kind": "debug", "config": {} }
        ]});
        flow_versions::ActiveModel {
            flow_id: Set(flow_id),
            version_number: Set(2),
            graph: Set(stored),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let get = |path: String| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                let res = server
                    .get(&path)
                    .add_header("Authorization", bearer(&token))
                    .add_header("X-Org-Id", org.to_string())
                    .await;
                (res.status_code().as_u16(), res.text())
            }
        };
        for path in ["", "/versions/2"] {
            let (st, text) = get(format!("/api/v1/flows/{flow_id}{path}")).await;
            assert_eq!(st, 200, "{path}: {text}");
            let body: serde_json::Value = serde_json::from_str(&text).unwrap();
            let ids: Vec<&str> = body["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n["id"].as_str().unwrap())
                .collect();
            assert_eq!(ids, ["n1", "n2"], "{body}");
            assert_eq!(
                body["graph"]["nodes"][0]["outputs"],
                serde_json::json!([{ "port": 0, "targets": ["n2"] }]),
                "{body}"
            );
            let codes: Vec<&str> = body["unreadable_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["code"].as_str().unwrap())
                .collect();
            assert_eq!(codes, ["flow-node-retired", "flow-node-unreadable"], "{body}");
        }
        // Routes that never re-read the graph as a whole: no 500 either.
        for path in ["", "/versions", "/runtime", "/node-status", "/debug"] {
            let path = if path.is_empty() {
                "/api/v1/flows".to_string()
            } else {
                format!("/api/v1/flows/{flow_id}{path}")
            };
            let (st, text) = get(path.clone()).await;
            assert!(st < 500, "{path}: {st} {text}");
            if path == "/api/v1/flows" {
                assert!(text.contains("\"legacy\""), "{text}");
            }
        }
        let res = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 400, "{}", res.text());
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["errors"]["violations"][0]["code"], "flow-node-retired", "{body}");
        let res = server
            .delete(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204, "{}", res.text());
        assert_eq!(get(format!("/api/v1/flows/{flow_id}")).await.0, 404);
    })
    .await;
}
