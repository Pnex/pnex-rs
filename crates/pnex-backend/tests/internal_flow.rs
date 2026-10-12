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

/// Model calls of the vision-caption / media-fuse nodes (D195/D196):
/// token-gated, the frame key must be in the org, no LLM provider = 409.
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
        for (path, body) in [("caption", &caption), ("fuse", &fuse)] {
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
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
