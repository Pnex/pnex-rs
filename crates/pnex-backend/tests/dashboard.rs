//! Dashboard summary tests (2026-08-19): PG sections served whatever
//! happens, telemetry degraded without OpenObserve (section absent from
//! test.yaml), X-Org-Id required, read open to the viewer.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.
//! The telemetry-available path (available:true) lives in
//! tests/openobserve.rs against the O2 mock.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boots the app + seeds the minimal catalog (same harness as devices.rs).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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

async fn summary(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
) -> axum_test::TestResponse {
    server
        .get("/api/v1/dashboard/summary")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
}

/// Without O2 configured: 200 anyway, correct PG sections (liveness of a
/// never-seen device, empty builds without NaN), degraded telemetry, and
/// X-Org-Id required (400).
#[tokio::test]
#[serial]
async fn dashboard_summary_degrade_sans_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        server
            .post("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "device_id": "esp-001",
                "predefined_device_name": "temp_sensor",
            }))
            .await;

        let res = summary(&server, &env.alice, org).await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();

        // Degraded telemetry, never an error from this branch.
        assert_eq!(body["telemetry"]["available"], false);
        assert_eq!(body["telemetry"]["latest"].as_array().unwrap().len(), 0);

        // Liveness: the device exists but never emitted.
        assert_eq!(body["liveness"]["total"], 1);
        assert_eq!(body["liveness"]["live"], 0);
        let device = &body["liveness"]["devices"][0];
        assert_eq!(device["device_id"], "esp-001");
        assert_eq!(device["predefined_device_name"], "temp_sensor");
        assert_eq!(device["live"], false);
        assert!(device["last_seen"].is_null());

        // Builds: zero, without NaN.
        assert_eq!(body["builds"]["total"], 0);
        assert_eq!(body["builds"]["succeeded"], 0);
        assert_eq!(body["builds"]["success_rate"], 0.0);

        // Liveness cap: 12 more devices (direct insert, quota
        // bypassed) → total 13 but list capped at 10, full counters
        // (user request: "only latest ~10").
        {
            use pnex_backend::models::_entities::{device_registries, predefined_devices};
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            let soil = predefined_devices::Entity::find()
                .filter(predefined_devices::Column::Name.eq("temp_sensor"))
                .one(&_ctx.db)
                .await
                .unwrap()
                .unwrap();
            for i in 0..12 {
                device_registries::ActiveModel {
                    device_id: Set(format!("cap-{i:02}")),
                    org_id: Set(org),
                    predefined_device_id: Set(soil.id),
                    ..Default::default()
                }
                .insert(&_ctx.db)
                .await
                .unwrap();
            }
        }
        let body: serde_json::Value = summary(&server, &env.alice, org).await.json();
        assert_eq!(body["liveness"]["total"], 13, "compteur complet");
        assert_eq!(body["liveness"]["live"], 0);
        let list = body["liveness"]["devices"].as_array().unwrap();
        assert_eq!(list.len(), 10, "list capped at ~10");

        // Without X-Org-Id → 400 (OrgContext extractor).
        let res = server
            .get("/api/v1/dashboard/summary")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "org-header-required"
        );

        // Another user's org → 403 coded, never a French message.
        let res = server
            .get("/api/v1/dashboard/summary")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 403);
        assert_eq!(res.json::<serde_json::Value>()["error"], "org-not-member");
    })
    .await;
}

/// Read only: a viewer of another org sees its own summary
/// (D2 partitioning — devices of alice's org absent).
#[tokio::test]
#[serial]
async fn dashboard_summary_cloisonne_par_org() {
    with_app(|server, env, _ctx| async move {
        let org_alice = personal_org(&server, &env.alice).await;
        server
            .post("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_alice.to_string())
            .json(&serde_json::json!({
                "device_id": "esp-001",
                "predefined_device_name": "temp_sensor",
            }))
            .await;

        // Bob: distinct personal org, without devices.
        let org_bob = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json::<serde_json::Value>()["orgs"][0]["id"]
            .as_i64()
            .expect("org perso bob");

        let body: serde_json::Value = summary(&server, &env.bob, org_bob).await.json();
        assert_eq!(body["liveness"]["total"], 0, "bob's org empty");
        assert_eq!(body["builds"]["total"], 0);
    })
    .await;
}
