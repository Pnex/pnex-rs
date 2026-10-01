//! Cross-pod auth/security state: rate-limit layer (HTTP 429 + Retry-After),
//! Valkey-shared counters between two "pods", and the OAuth2 native bridge
//! surviving a callback and a poll served by different pods.
//!
//! Valkey cases are gated on `PNEX_TEST_VALKEY_URL` (skip cleanly when
//! absent, like `tests/last_cache.rs`).

mod common;

use std::time::Duration;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::controllers::oauth2::{native_bridge_put_with, native_bridge_take_with};
use pnex_backend::services::rate_limit::{Decision, RateLimiter};
use serial_test::serial;

fn valkey_url() -> Option<String> {
    std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

async fn manager(url: &str) -> redis::aio::ConnectionManager {
    let client = redis::Client::open(url).expect("client");
    redis::aio::ConnectionManager::new(client)
        .await
        .expect("valkey connection")
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

#[tokio::test]
#[serial]
async fn enroll_is_rate_limited_with_retry_after() {
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        |server, _ctx| async move {
            // 10 attempts per minute: invalid codes answer 400, never 429.
            for _ in 0..10 {
                let res = server
                    .post("/api/v1/agent/enroll")
                    .json(&serde_json::json!({"code": "AAAA-BBBB-CCCC"}))
                    .await;
                assert_eq!(res.status_code(), 400, "{}", res.text());
            }
            let res = server
                .post("/api/v1/agent/enroll")
                .json(&serde_json::json!({"code": "AAAA-BBBB-CCCC"}))
                .await;
            assert_eq!(res.status_code(), 429, "{}", res.text());
            let retry: u64 = res
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .expect("Retry-After header");
            assert!((1..=60).contains(&retry), "{retry}");
            let body: serde_json::Value = res.json();
            assert_eq!(
                body["error"],
                pnex_core::err_codes::AGENT_ENROLL_RATE_LIMITED
            );

            // Unprotected routes are untouched by the enroll budget.
            let res = server.get("/api/v1/meta/version").await;
            assert_eq!(res.status_code(), 200);
        },
    )
    .await;
}

#[tokio::test]
async fn valkey_counters_are_shared_between_pods() {
    let Some(url) = valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let pod_a = RateLimiter::new(Some(manager(&url).await));
    let pod_b = RateLimiter::new(Some(manager(&url).await));
    let bucket = unique("test-shared");
    let window = Duration::from_secs(30);
    // 4 hits spread over two pods consume one shared budget of 4.
    for limiter in [&pod_a, &pod_b, &pod_a, &pod_b] {
        assert_eq!(limiter.hit(&bucket, 4, window).await, Decision::Allow);
    }
    match pod_b.hit(&bucket, 4, window).await {
        Decision::Deny { retry_after } => {
            assert!(retry_after <= window && retry_after > Duration::ZERO)
        }
        Decision::Allow => panic!("budget must be shared across pods"),
    }
    assert!(matches!(
        pod_a.hit(&bucket, 4, window).await,
        Decision::Deny { .. }
    ));
}

#[tokio::test]
async fn native_bridge_crosses_pods_and_is_single_use() {
    let Some(url) = valkey_url() else {
        eprintln!("skip: PNEX_TEST_VALKEY_URL absent (no compose valkey)");
        return;
    };
    let state = unique("state");
    // Callback on pod A, polls on pod B.
    native_bridge_put_with(Some(manager(&url).await), &state, "the-code").await;
    let pod_b = manager(&url).await;
    assert_eq!(
        native_bridge_take_with(Some(pod_b.clone()), &state)
            .await
            .as_deref(),
        Some("the-code")
    );
    assert_eq!(
        native_bridge_take_with(Some(pod_b.clone()), &state).await,
        None
    );
    // The entry carries the 5-minute expiry.
    let mut conn = pod_b;
    native_bridge_put_with(Some(conn.clone()), &state, "c2").await;
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("pnex:oauth2:native:*")
        .query_async(&mut conn)
        .await
        .expect("keys");
    let mut found = false;
    for k in keys {
        let ttl: i64 = redis::cmd("TTL")
            .arg(&k)
            .query_async(&mut conn)
            .await
            .expect("ttl");
        if ttl > 0 && ttl <= 300 {
            found = true;
        }
    }
    assert!(found, "bridge entry expires within 300 s");
    let _ = native_bridge_take_with(Some(conn), &state).await;
}
