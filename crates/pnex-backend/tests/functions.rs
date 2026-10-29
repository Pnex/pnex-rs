//! Tests for the "Functions" registry — append-only versioned CRUD, org
//! isolation (masked 404), read-only viewer, 409 (version conflict, function
//! referenced by a deployed flow), invalid directives 400, live test
//! endpoints (200 with the runtime fixture, 503 engine off).
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.
//! The supervisor is a per-process static: the live-test test is the only
//! one to enable `PNEX_FLOW_ENABLED` (same contract as tests/flows.rs).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::ActiveModelTrait;
use serde_json::json;
use serial_test::serial;

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

/// Engine env settings, set BEFORE boot (Tera config read at boot).
fn flow_state_dir() -> String {
    format!("/tmp/pnex-flow-tests-{}", std::process::id())
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

// ───────────────────────── Helpers CRUD ─────────────────────────

/// POST functions → (status, corps).
async fn post_fn(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .post("/api/v1/functions")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// PATCH → (status, corps).
async fn patch_fn(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .patch(&format!("/api/v1/functions/{id}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// POST /validate → (status, corps).
async fn post_validate(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .post("/api/v1/functions/validate")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// POST /{id}/test → (status, corps).
async fn post_test(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .post(&format!("/api/v1/functions/{id}/test"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// DELETE → status.
async fn del_fn(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: i64,
) -> axum::http::StatusCode {
    server
        .delete(&format!("/api/v1/functions/{id}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
        .status_code()
}

/// Minimal Starlark code reading the input `t` (see `t_input`).
fn starlark_code() -> String {
    "def handle(inputs, msg):\n    return {\"v\": inputs[\"t\"] + 1}".to_string()
}

/// The declared input of `starlark_code` (D497: interface outside the code).
fn t_input() -> serde_json::Value {
    json!([{"name": "t", "ty": "number"}])
}

/// Minimal flow graph with a function node referencing (fid, v1).
fn function_flow_graph(fid: i64, fn_id: i64) -> serde_json::Value {
    json!({
        "nodes": [{
            "id": format!("f{fid}"),
            "kind": "pnex_function",
            "config": {
                "function_id": fn_id,
                "function_name": "fonction test",
                "version_number": 1,
                "language": "starlark",
                "inputs": [],
                "outputs": []
            }
        }]
    })
}

// ─────────────────────────────── Tests ───────────────────────────────

#[tokio::test]
#[serial]
async fn create_save_delete_cycle_with_versions() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Creation → 201, detail with the interface extracted from directives.
        let (status, body) = post_fn(
            &server,
            &env.alice,
            org,
            json!({
                "name": "chauffage",
                "language": "starlark",
                "description": "Règle de chauffage",
                "code": starlark_code(),
                "inputs": t_input(),
            }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["name"], "chauffage");
        assert_eq!(body["language"], "starlark");
        assert_eq!(body["current_version_number"], 1);
        assert_eq!(body["inputs"][0]["name"], "t");
        assert_eq!(body["inputs"][0]["ty"], "number");
        let id = body["id"].as_i64().expect("id returned");

        // Save v2 (new append-only version).
        let (status, body) = patch_fn(
            &server,
            &env.alice,
            org,
            id,
            json!({
                "expected_version_number": 1,
                "code": starlark_code(),
                "note": "ajustement",
            }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["current_version_number"], 2);
        assert_eq!(body["code"], starlark_code());
        // Interface not sent: carried from v1.
        assert_eq!(body["inputs"][0]["name"], "t", "{body}");

        // Versions in DESC order [2, 1].
        let versions = server
            .get(&format!("/api/v1/functions/{id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        let numbers: Vec<i64> = versions["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["version_number"].as_i64().unwrap())
            .collect();
        assert_eq!(numbers, vec![2, 1]);

        // Delete → 204, then 404.
        assert_eq!(del_fn(&server, &env.alice, org, id).await, 204);
        let (status, _) = post_test(
            &server,
            &env.alice,
            org,
            id,
            json!({"inputs": {}, "msg": {}}),
        )
        .await;
        assert_eq!(status, 404, "detail after delete");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cross_org_isolation_404() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        assert_ne!(org_a, org_b);

        let (_, created) = post_fn(
            &server,
            &env.alice,
            org_a,
            json!({"name": "f", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        // Bob (other org): detail/list masked 404, delete 404.
        let cross = server
            .get(&format!("/api/v1/functions/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(cross.status_code(), 404);
        assert_eq!(del_fn(&server, &env.bob, org_b, id).await, 404);

        // Bob's list does not contain alice's function.
        let list = server
            .get("/api/v1/functions")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn viewer_reads_without_writing() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        // Bob must have "logged in" at least once before the invitation
        // (add-member contract: user already seen by the IdP).
        let _bob_org = personal_org(&server, &env.bob).await;
        server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&json!({"email": "bob@example.com", "role": "viewer"}))
            .await;

        let (_, created) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "f", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        // Viewer : lecture OK…
        let read = server
            .get(&format!("/api/v1/functions/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(read.status_code(), 200);
        // …writes forbidden (create/PATCH/DELETE 403)…
        let (status, _) = post_fn(
            &server,
            &env.bob,
            org,
            json!({"name": "x", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        assert_eq!(status, 403);
        let (status, _) = patch_fn(
            &server,
            &env.bob,
            org,
            id,
            json!({"expected_version_number": 1, "code": "x"}),
        )
        .await;
        assert_eq!(status, 403);
        assert_eq!(del_fn(&server, &env.bob, org, id).await, 403);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validations_400_and_conflict_409() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Empty name.
        let (status, body) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "  ", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["errors"]["fields"]["name"], "required", "{body}");

        // Invalid signature (D497): field errors on the declared ports.
        let (status, body) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "f", "language": "js", "code": "function handle(i, m) { return i; }",
                   "inputs": [{"name": "A", "ty": "number"}]}),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(
            body["errors"]["fields"]["inputs.0.name"], "invalid",
            "{body}"
        );
        let (status, body) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "f", "language": "js", "code": "function handle(i, m) { return i; }",
                   "outputs": [{"name": "a", "ty": "any"}, {"name": "a", "ty": "any"}]}),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(
            body["errors"]["fields"]["outputs.1.name"], "unique",
            "{body}"
        );

        // Version conflict: expected ≠ current → 409.
        let (_, created) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "f2", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        let id = created["id"].as_i64().unwrap();
        let (status, body) = patch_fn(
            &server,
            &env.alice,
            org,
            id,
            json!({"expected_version_number": 99, "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"], "function-version-conflict");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn delete_409_if_a_deployed_flow_references_it() {
    with_app_flow_engine(false, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, created) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "protegee", "language": "starlark", "code": starlark_code()}),
        )
        .await;
        let fn_id = created["id"].as_i64().unwrap();

        // `deployed` flow referencing the function (direct DB insert — the
        // guard reads the graphs of deployed versions, all orgs).
        let flow = pnex_backend::models::_entities::flows::ActiveModel {
            org_id: sea_orm::Set(org),
            name: sea_orm::Set("flow ref".into()),
            status: sea_orm::Set(pnex_core::FLOW_STATUS_DEPLOYED.to_string()),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("flow inserted");
        let version = pnex_backend::models::_entities::flow_versions::ActiveModel {
            flow_id: sea_orm::Set(flow.id),
            version_number: sea_orm::Set(1),
            graph: sea_orm::Set(function_flow_graph(flow.id, fn_id)),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("version inserted");
        use pnex_backend::models::_entities::flows;
        let mut link: flows::ActiveModel = flow.clone().into();
        link.deployed_version_id = sea_orm::Set(Some(version.id));
        link.update(&ctx.db).await.expect("flow marked deployed");

        // Delete: 409 function-in-use, the referencing flows in `args.flows`.
        let resp = server
            .delete(&format!("/api/v1/functions/{fn_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(resp.status_code(), 409);
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["error"], "function-in-use", "{body}");
        let flows = body["errors"]["args"]["flows"].as_str().unwrap_or_default();
        assert!(
            flows.contains(&format!("#{} ", flow.id)) && flows.ends_with(" v1"),
            "{body}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_live_200_with_fixture() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, created) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "live", "language": "starlark", "code": starlark_code(), "inputs": t_input()}),
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        // Live test with the runtime fixture: 200 ok:true, canned outputs.
        let (status, body) = post_test(
            &server,
            &env.alice,
            org,
            id,
            json!({"inputs": {"t": 41}, "msg": {"payload": {"t": 41}}}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["outputs"][0]["payload"]["echo"], true);

        // Ad-hoc test (unsaved code): same contract.
        let (status, body) = post_test(
            &server,
            &env.alice,
            org,
            id,
            json!({
                "ad_hoc": {"language": "js", "code": "function handle(i, m) { return i; }"},
                "msg": {}
            }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["ok"], true);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_live_503_engine_off() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, created) = post_fn(
            &server,
            &env.alice,
            org,
            json!({"name": "off", "language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        let id = created["id"].as_i64().unwrap();

        // Engine disabled (settings.flow.enabled=false) → 503 flow_runtime.
        let (status, body) = post_test(&server, &env.alice, org, id, json!({"msg": {}})).await;
        assert_eq!(status, 503, "{body}");
        assert_eq!(body["error"], "functions-runtime-disabled");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validate_200_with_fixture() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Viewer (bob) allowed: validation is a read tool.
        // NOTE: bob belongs to his own personal org here — membership scoping
        // is what OrgContext enforces, not the role; owner (alice) is used.
        let (status, body) = post_validate(
            &server,
            &env.alice,
            org,
            json!({"language": "starlark", "code": "def handle(inputs, msg):\n    return inputs"}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["ok"], true);
        assert_eq!(body["diagnostics"], json!([]));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validate_503_engine_off() {
    with_app_flow_engine(false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (status, body) = post_validate(
            &server,
            &env.alice,
            org,
            json!({"language": "js", "code": "function handle(i, m) { return i; }"}),
        )
        .await;
        assert_eq!(status, 503, "{body}");
        assert_eq!(body["error"], "functions-runtime-disabled");
    })
    .await;
}

/// A saturated per-pod runtime-check pool rejects the debounced editor
/// validation at once with `503 server-busy` (no child spawned, no queue).
#[tokio::test]
#[serial]
async fn validate_503_server_busy_when_runtime_check_pool_is_full() {
    with_app_flow_engine(true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let pool = pnex_backend::services::compute_limits::runtime_check();
        let mut held = Vec::new();
        while let Ok(p) = pool.try_acquire() {
            held.push(p);
        }
        let (status, body) = post_validate(
            &server,
            &env.alice,
            org,
            json!({"language": "starlark", "code": "def handle(inputs, msg):\n    return inputs"}),
        )
        .await;
        assert_eq!(status, 503, "{body}");
        assert_eq!(body["error"], pnex_core::err_codes::SERVER_BUSY);
        drop(held);
        let (status, body) = post_validate(
            &server,
            &env.alice,
            org,
            json!({"language": "starlark", "code": "def handle(inputs, msg):\n    return inputs"}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
    })
    .await;
}
