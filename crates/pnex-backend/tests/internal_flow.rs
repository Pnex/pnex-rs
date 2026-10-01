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
