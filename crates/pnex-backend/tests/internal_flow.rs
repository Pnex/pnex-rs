//! Tests de la route interne `/internal/flow/device-write` (nœud
//! `pnex-device-write`) — école `notify.rs`.

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

/// Fail-closed : sans token configuré, toute écriture est refusée 401.
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

/// Token configuré (env) mais faux → 401 ; device inconnu avec bon token
/// → 404. La valeur du token passe par `PNEX_FLOW_RUNTIME_TOKEN` (même
/// résolution que la route).
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
    with_app(|server| async move {
        let caption = serde_json::json!({
            "org_id": 1, "frame_key": "pnex:cam:v1:2:cam:f:1", "prompt": "what?"
        });
        let fuse = serde_json::json!({
            "org_id": 1, "t0_ms": 0, "t1_ms": 1, "language": "",
            "items": [{"id": "a", "kind": "caption", "ts_ms": 0, "text": "a cat"}]
        });
        let decide = serde_json::json!({
            "org_id": 1, "state": "a forklift entered the dock",
            "questions": [{"id": "incident", "kind": "choice", "instructions": "Incident?", "options": ["yes", "no"]}]
        });
        let text = serde_json::json!({
            "org_id": 1, "prompt": "Summarize", "data": {"events": ["a"]}, "max_chars": 500
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
