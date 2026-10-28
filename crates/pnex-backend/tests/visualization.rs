//! Tests for the Visualisation page's `/api/v1/telemetry/*` endpoints
//! (2026-08-19): without OpenObserve configured (section missing from
//! test.yaml), both endpoints answer a degraded 200
//! (`available: false`), never 500; the parameters are validated
//! (PromQL anti-injection); X-Org-Id required.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.
//! The available path (available:true) against the O2 mock lives in
//! tests/openobserve.rs.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
}

/// Boots the app + seeds the minimal catalogue (same harness as dashboard.rs).
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
        .expect("personal org")
}

async fn series(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    metric: &str,
    device_id: &str,
    window: &str,
) -> axum_test::TestResponse {
    server
        .get(&format!(
            "/api/v1/telemetry/series?metric={metric}&device_id={device_id}&window={window}"
        ))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
}

/// Without O2 configured: catalogue and series answer a degraded 200
/// (`available: false`, empty lists) — never 500 from the telemetry
/// branch. Hostile parameters are rejected with 400 BEFORE any PromQL
/// query is built (injection impossible), and X-Org-Id
/// stays required.
#[tokio::test]
#[serial]
async fn telemetry_degrades_and_validates_without_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Degraded catalogue.
        let res = server
            .get("/api/v1/telemetry/catalog")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        // D240: OpenObserve unavailable = 503 coded, no `available` envelope.
        assert_eq!(res.status_code(), 503);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "telemetry-unavailable");

        // Degraded series with valid parameters.
        let res = series(&server, &env.alice, org, "soil_moisture", "esp-001", "24h").await;
        assert_eq!(res.status_code(), 503);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "telemetry-unavailable"
        );

        // PromQL anti-injection: closed metric/device charset + preset
        // window — everything else is rejected before reaching O2. The
        // hostile payloads are percent-encoded (a real client necessarily
        // encodes these characters, the server decodes then validates).
        let res = series(
            &server,
            &env.alice,
            org,
            "x%22%7Dor%20up%7B",
            "esp-001",
            "24h",
        )
        .await;
        assert_eq!(res.status_code(), 400, "hostile metric rejected");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(body["errors"]["fields"]["metric"], "invalid");
        let res = series(
            &server,
            &env.alice,
            org,
            "soil_moisture",
            "zebra%22or%20up%28",
            "24h",
        )
        .await;
        assert_eq!(res.status_code(), 400, "hostile device_id rejected");
        let res = series(&server, &env.alice, org, "soil_moisture", "esp-001", "90d").await;
        assert_eq!(res.status_code(), 400, "non-preset window rejected");
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["window"],
            "invalid"
        );
        let res = series(&server, &env.alice, org, "", "esp-001", "24h").await;
        assert_eq!(res.status_code(), 400, "empty metric rejected");

        // Missing parameter → 400 (Query deserialisation).
        let res = server
            .get("/api/v1/telemetry/series?metric=soil_moisture")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400);

        // Without X-Org-Id → 400 (OrgContext extractor).
        let res = server
            .get("/api/v1/telemetry/catalog")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// `POST /api/v1/telemetry/series/query` (D31, SCADA studio) — without O2
/// configured: valid items are **degraded at the item level**
/// (`available: false`), an invalid spec is isolated (never a global
/// 400), the response order follows the spec order, and the spec cap
/// is a real 400 (call error, not missing data).
#[tokio::test]
#[serial]
async fn series_batch_degrades_per_item_without_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Mixed batch: 2 valid specs + 1 invalid (hostile device +
        // unknown window).
        let res = server
            .post("/api/v1/telemetry/series/query")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "specs": [
                    { "metric": "soil_moisture", "device_id": "esp-001", "window": "5m" },
                    { "metric": "x\"}or up{", "device_id": "esp-001", "window": "1h" },
                    { "metric": "soil_temperature", "device_id": "esp-002", "window": "24h" },
                    // D171 label selectors: device-less valid, hostile value isolated.
                    { "metric": "media_capture_up", "device_id": "", "window": "1h",
                      "labels": { "stream": "inter" } },
                    { "metric": "media_capture_up", "device_id": "", "window": "1h",
                      "labels": { "stream": "x\"}or up{" } }
                ]
            }))
            .await;
        assert_eq!(res.status_code(), 200, "never a 400 on an invalid spec");
        let body: serde_json::Value = res.json();
        let results = body["results"].as_array().expect("results");
        assert_eq!(results.len(), 5, "one item per spec, same order");
        assert_eq!(results[0]["available"], false);
        assert_eq!(results[0]["metric"], "soil_moisture");
        assert_eq!(results[1]["available"], false, "hostile spec isolated");
        assert_eq!(results[2]["available"], false);
        assert_eq!(results[3]["metric"], "media_capture_up");
        assert_eq!(results[4]["available"], false, "hostile label isolated");

        // Without specs: empty batch but 200 (dashboard without a source).
        let res = server
            .post("/api/v1/telemetry/series/query")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "specs": [] }))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["results"].as_array().unwrap().len(), 0);

        // Anti-abuse cap: beyond 24 specs → 400 (invalid call
        // shape, distinct from degraded data).
        let mut specs = Vec::new();
        for i in 0..25 {
            specs.push(serde_json::json!({
                "metric": "soil_moisture", "device_id": format!("esp-{i:03}"), "window": "1h"
            }));
        }
        let res = server
            .post("/api/v1/telemetry/series/query")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "specs": specs }))
            .await;
        assert_eq!(res.status_code(), 400, "spec cap exceeded → 400");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "telemetry-batch-too-large");
        assert_eq!(body["errors"]["args"]["max"], "24");

        // X-Org-Id stays required on the batch (like the rest of the domain).
        let res = server
            .post("/api/v1/telemetry/series/query")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "specs": [] }))
            .await;
        assert_eq!(res.status_code(), 400, "X-Org-Id absent → 400");
    })
    .await;
}
