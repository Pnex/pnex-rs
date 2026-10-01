//! Events (camera-video.md D84): API without OpenObserve (test config) and
//! a live round trip against a local O2 (`--ignored`).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, _ctx| async move {
            f(server, alice).await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

/// Without O2 the page reads `available: false` (never an error), bad
/// filters are field errors, and the internal write is token-gated.
#[tokio::test]
#[serial]
async fn events_api_without_openobserve() {
    with_app(|server, alice| async move {
        let org = personal_org(&server, &alice).await;
        let res = server
            .get("/api/v1/events?stream=doors")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status_ok();
        let body: serde_json::Value = res.json();
        assert_eq!(body["available"], false);
        assert_eq!(body["count"], 0);

        let bad = server
            .get("/api/v1/events?level=loud")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert_eq!(bad.json::<serde_json::Value>()["level"], "invalid");

        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
        let denied = server
            .post("/internal/flow/event")
            .json(&serde_json::json!({"org_id": org, "stream": "ev_events", "level": "info"}))
            .await;
        denied.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
    })
    .await;
}

/// Live round trip: ingest through the client, read back through the same
/// SQL/record mapping as the API. Needs O2 on localhost:5080 (dev compose).
#[tokio::test]
#[ignore = "requires a live OpenObserve (dev compose, localhost:5080)"]
async fn events_roundtrip_live_openobserve() {
    use pnex_backend::services::events::{document_of, record_of, search_sql, EventQuery};
    use pnex_backend::services::openobserve::{Client, OpenobserveSettings};
    let client = Client::new(&OpenobserveSettings {
        base_url: std::env::var("OPENOBSERVE_URL")
            .unwrap_or_else(|_| "http://localhost:5080".into()),
        root_email: std::env::var("OPENOBSERVE_ROOT_EMAIL")
            .unwrap_or_else(|_| "root@pnex.local".into()),
        root_password: std::env::var("OPENOBSERVE_ROOT_PASSWORD")
            .unwrap_or_else(|_| "Pnex-dev-2026!".into()),
    });
    let root = format!(
        "{}:{}",
        std::env::var("OPENOBSERVE_ROOT_EMAIL").unwrap_or_else(|_| "root@pnex.local".into()),
        std::env::var("OPENOBSERVE_ROOT_PASSWORD").unwrap_or_else(|_| "Pnex-dev-2026!".into())
    );
    let stream = format!("ev_test_{}", std::process::id());
    let ev = pnex_core::events::EventInput {
        org_id: 0,
        stream: stream.clone(),
        level: pnex_core::events::EventLevel::Warn,
        message: "person at the door".into(),
        topic: Some("cam-front".into()),
        flow_id: Some(12),
        node_id: "n7".into(),
        payload: serde_json::json!({"detections": [{"label": "person", "score": 0.91}]}),
    };
    let now = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json("default", &stream, &[document_of(&ev, now)], &root)
        .await
        .expect("ingest");
    let q = EventQuery {
        stream: stream.clone(),
        level: Some("warn".into()),
        text: Some("door".into()),
        from_us: Some(now - 60_000_000),
        to_us: Some(now + 60_000_000),
        offset: 0,
        limit: 10,
    };
    let mut found = None;
    for _ in 0..30 {
        let r = client
            .search_logs(
                "default",
                &search_sql(&q),
                now - 60_000_000,
                now + 60_000_000,
                0,
                10,
                &root,
            )
            .await
            .expect("search");
        if let Some(hit) = r.hits.first() {
            found = Some(record_of(&stream, hit));
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let rec = found.expect("event searchable");
    assert_eq!(rec.message, "person at the door");
    assert_eq!(rec.flow_id, Some(12));
    assert_eq!(rec.payload["detections"][0]["label"], "person");
    let streams = client.log_streams("default", &root).await.expect("streams");
    assert!(streams.contains(&stream));
}
