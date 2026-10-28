//! Tests for the internal route `/internal/flow/device-write` (node
//! `pnex-device-write`) — modelled on `notify.rs`.

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use loco_rs::TestServer;
use pnex_backend::app::App;
use serial_test::serial;

mod common;

/// Boot the app with the flow engine disabled.
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(TestServer) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    unsafe { std::env::remove_var("PNEX_FLOW_STATE_DIR") };
    unsafe { std::env::remove_var("PNEX_FLOW_ENABLED") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, _ctx| async move {
            f(server).await;
        },
    )
    .await;
}

/// Same boot, with the app context and a signed-in user token.
async fn with_app_ctx<F, Fut>(f: F)
where
    F: FnOnce(TestServer, loco_rs::app::AppContext, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    unsafe { std::env::remove_var("PNEX_FLOW_STATE_DIR") };
    unsafe { std::env::remove_var("PNEX_FLOW_ENABLED") };
    let token = common::valid_token(
        &base,
        "flow-ai-user-000000000000",
        "flowai",
        "flow-ai@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            f(server, ctx, token).await;
        },
    )
    .await;
}

/// Personal org of the signed-in user.
async fn personal_org(server: &TestServer, token: &str) -> i64 {
    let body: serde_json::Value = server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json();
    body["orgs"][0]["id"].as_i64().unwrap()
}

/// A deployed flow of `org` holding one node of each AI kind.
async fn deploy_ai_nodes(ctx: &loco_rs::app::AppContext, org: i64) {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("ai".into()),
        status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
        org_id: Set(org),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    let version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(serde_json::json!({ "nodes": [
            {"id": "c", "kind": "vision_caption"},
            {"id": "f", "kind": "media_fuse"},
            {"id": "t", "kind": "llm_text"},
            {"id": "d", "kind": "text_decide"}
        ] })),
        ..Default::default()
    }
    .insert(&ctx.db)
    .await
    .unwrap();
    let mut am: flows::ActiveModel = flow.into();
    am.deployed_version_id = Set(Some(version.id));
    am.update(&ctx.db).await.unwrap();
}

/// Fail-closed: without a configured token, every write is refused 401.
#[tokio::test]
#[serial]
async fn interne_sans_token_refuse() {
    with_app(|server| async move {
        let resp = server
            .post("/internal/flow/device-write")
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "org_id": 1,
                "device_id": "x",
                "values": {"relais": true}
            }))
            .await;
        assert_eq!(resp.status_code(), 401);
    })
    .await;
}

/// Token configured (env) but wrong → 401; unknown device with the right
/// token → 404. The token value goes through `PNEX_FLOW_RUNTIME_TOKEN` (same
/// resolution as the route).
#[tokio::test]
#[serial]
async fn interne_mauvais_token_et_device_inconnu() {
    with_app(|server| async move {
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-test-123") };
        let bad = server
            .post("/internal/flow/device-write")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-flow-token", "wrong")
            .json(&serde_json::json!({
                "org_id": 1, "device_id": "x", "values": {"relais": true}
            }))
            .await;
        assert_eq!(bad.status_code(), 401);
        let unknown = server
            .post("/internal/flow/device-write")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&serde_json::json!({
                "org_id": 1, "device_id": "inconnu", "values": {"relais": true}
            }))
            .await;
        assert_eq!(unknown.status_code(), 404);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

/// SEC-4: a request that came through the public edge (forwarding header)
/// never reaches an internal route, even with the right token.
#[tokio::test]
#[serial]
async fn interne_via_proxy_public_404() {
    with_app(|server| async move {
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-test-123") };
        for header in ["x-forwarded-for", "x-real-ip", "forwarded"] {
            let resp = server
                .post("/internal/flow/device-write")
                .add_header("Content-Type", "application/json")
                .add_header("x-pnex-flow-token", "tok-test-123")
                .add_header(header, "203.0.113.7")
                .json(&serde_json::json!({
                    "org_id": 1, "device_id": "inconnu", "values": {"relais": true}
                }))
                .await;
            assert_eq!(resp.status_code(), 404, "{header}");
            assert!(resp.text().is_empty(), "{header}: {}", resp.text());
        }
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

/// Model calls of the vision-caption / media-fuse / text-decide nodes
/// (D195–D197): token-gated, the frame key must be in the org, no LLM
/// provider = 409, no decision model = 503, bad questions = 400.
#[tokio::test]
#[serial]
async fn internal_ai_caption_and_fuse() {
    with_app_ctx(|server, ctx, user| async move {
        let org = personal_org(&server, &user).await;
        let caption = serde_json::json!({
            "org_id": org, "frame_key": "pnex:cam:v1:999999:cam:f:1", "prompt": "what?"
        });
        let fuse = serde_json::json!({
            "org_id": org, "t0_ms": 0, "t1_ms": 1, "language": "",
            "items": [{"id": "a", "kind": "caption", "ts_ms": 0, "text": "a cat"}]
        });
        let decide = serde_json::json!({
            "org_id": org, "state": "a forklift entered the dock",
            "questions": [{"id": "incident", "kind": "choice", "instructions": "Incident?", "options": ["yes", "no"]}]
        });
        let text = serde_json::json!({
            "org_id": org, "prompt": "Summarize", "data": {"events": ["a"]}, "max_chars": 500
        });
        for (path, body) in [
            ("caption", &caption),
            ("fuse", &fuse),
            ("decide", &decide),
            ("text", &text),
        ] {
            let denied = server
                .post(&format!("/internal/flow/ai/{path}"))
                .json(body)
                .await;
            assert_eq!(denied.status_code(), 401, "{path}");
        }
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-test-123") };
        // SEC-34: no deployed flow of the org holds the calling node.
        for (path, body) in [
            ("caption", &caption),
            ("fuse", &fuse),
            ("decide", &decide),
            ("text", &text),
        ] {
            let absent = server
                .post(&format!("/internal/flow/ai/{path}"))
                .add_header("x-pnex-flow-token", "tok-test-123")
                .json(body)
                .await;
            assert_eq!(absent.status_code(), 403, "{path}");
            assert_eq!(absent.json::<serde_json::Value>()["code"], "flow-ai-node-absent");
        }
        deploy_ai_nodes(&ctx, org).await;
        let foreign = server
            .post("/internal/flow/ai/caption")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&caption)
            .await;
        assert_eq!(foreign.status_code(), 404);
        assert_eq!(
            foreign.json::<serde_json::Value>()["code"],
            "camera-no-frame"
        );
        let unconfigured = server
            .post("/internal/flow/ai/fuse")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&fuse)
            .await;
        assert_eq!(unconfigured.status_code(), 409);
        assert_eq!(
            unconfigured.json::<serde_json::Value>()["code"],
            pnex_core::err_codes::AI_NOT_CONFIGURED
        );
        let unconfigured = server
            .post("/internal/flow/ai/text")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&text)
            .await;
        assert_eq!(unconfigured.status_code(), 409);
        assert_eq!(
            unconfigured.json::<serde_json::Value>()["code"],
            pnex_core::err_codes::AI_NOT_CONFIGURED
        );
        // No decision model on the test server; invalid questions first.
        unsafe { std::env::remove_var("PNEX_DECIDE_MODEL_DIR") };
        let no_model = server
            .post("/internal/flow/ai/decide")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&decide)
            .await;
        assert_eq!(no_model.status_code(), 503);
        assert_eq!(no_model.json::<serde_json::Value>()["code"], "decide-unavailable");
        let mut bad = decide.clone();
        bad["questions"][0]["options"] = serde_json::json!(["only"]);
        let invalid = server
            .post("/internal/flow/ai/decide")
            .add_header("x-pnex-flow-token", "tok-test-123")
            .json(&bad)
            .await;
        assert_eq!(invalid.status_code(), 400);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
