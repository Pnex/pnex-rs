//! Edge reference tests — CRUD wifi-credentials/hosts, **upsert**
//! (200/201, never 409), field-by-field 400 validations, org isolation,
//! auth 401.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::json;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
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
            f(server, env).await;
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

// ───────────────────────── Helpers CRUD ─────────────────────────

/// POST wifi-credentials → (status, corps JSON).
async fn post_wifi(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .post("/api/v1/edge/wifi-credentials")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// POST hosts → (status, corps JSON).
async fn post_host(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    let resp = server
        .post("/api/v1/edge/hosts")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    (resp.status_code(), resp.json::<serde_json::Value>())
}

/// GET list → `results` array.
async fn list_edge(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
) -> Vec<serde_json::Value> {
    server
        .get(path)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
        .json::<serde_json::Value>()["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// DELETE → status.
async fn del(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
) -> axum::http::StatusCode {
    server
        .delete(path)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
        .status_code()
}

// ─────────────────────────────── Tests ───────────────────────────────

#[tokio::test]
#[serial]
async fn wifi_cycle_upsert_and_delete() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Creation → 201.
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "Maison", "password": { "value": "secret1" } }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["ssid"], "Maison");
        let id = body["id"].as_i64().expect("id returned");

        // Upsert same SSID, other password → 200, same id.
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "Maison", "password": { "value": "secret2" } }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["id"].as_i64(),
            Some(id),
            "upsert = update, no duplicate"
        );
        // The password lives in the vault: only its reference is returned.
        assert!(!body.to_string().contains("secret2"), "{body}");
        assert_eq!(body["password"]["name"], "wifi/Maison/password");

        // Upsert: no duplicate in the database.
        let rows = list_edge(&server, &env.alice, org, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 1);

        // Delete → 204, then empty list and delete → 404.
        let path = format!("/api/v1/edge/wifi-credentials/{id}");
        assert_eq!(del(&server, &env.alice, org, &path).await, 204);
        let rows = list_edge(&server, &env.alice, org, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 0);
        assert_eq!(del(&server, &env.alice, org, &path).await, 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn wifi_validations_400() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Empty SSID (spaces).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "  ", "password": { "value": "x" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["error"], "field-invalid", "{body}");
        assert!(body["errors"]["fields"].get("ssid").is_some());

        // SSID > 32 octets (802.11).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "a".repeat(33), "password": { "value": "x" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["error"], "field-invalid", "{body}");
        assert!(body["errors"]["fields"].get("ssid").is_some());

        // Empty password (open networks out of v1 scope).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "Maison", "password": { "value": "" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body["errors"]["fields"].get("password").is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn hosts_cycle_upsert_and_validation() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Creation → 201.
        let (status, body) = post_host(
            &server,
            &env.alice,
            org,
            json!({ "host": "192.168.1.16:5150" }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        let id = body["id"].as_i64().expect("id returned");
        // Devices always use wss: a host carries no scheme flag.
        assert!(body.get("ws_ssl").is_none(), "{body}");

        // Upsert same host → 200, same id.
        let (status, body) = post_host(
            &server,
            &env.alice,
            org,
            json!({ "host": "192.168.1.16:5150" }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["id"].as_i64(), Some(id));

        // Validation: scheme forbidden (CreateBuild.pnex_host contract = bare
        // host), space forbidden, empty forbidden.
        for payload in [
            json!({ "host": "ws://192.168.1.16:5150" }),
            json!({ "host": "192.168.1.16 5150" }),
            json!({ "host": "   " }),
        ] {
            let (status, body) = post_host(&server, &env.alice, org, payload).await;
            assert_eq!(status, 400, "{body}");
            assert_eq!(body["error"], "field-invalid", "{body}");
            assert!(body["errors"]["fields"].get("host").is_some());
        }

        // Rename onto a host used by another entry → 409 edge-host-taken.
        let (_, other) = post_host(&server, &env.alice, org, json!({ "host": "10.0.0.2" })).await;
        let res = server
            .put(&format!("/api/v1/edge/hosts/{}", other["id"]))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "host": "192.168.1.16:5150" }))
            .await;
        assert_eq!(res.status_code(), 409);
        assert_eq!(res.json::<serde_json::Value>()["error"], "edge-host-taken");

        // Delete → 204.
        let path = format!("/api/v1/edge/hosts/{id}");
        assert_eq!(del(&server, &env.alice, org, &path).await, 204);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_auth_401() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        // Without a token → 401 (both domains).
        let anon = server.get("/api/v1/edge/wifi-credentials").await;
        assert_eq!(anon.status_code(), 401);
        let anon = server
            .post("/api/v1/edge/hosts")
            .json(&json!({ "host": "h:1" }))
            .await;
        assert_eq!(anon.status_code(), 401);

        // Alice creates on her org.
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org_a,
            json!({ "ssid": "Maison", "password": { "value": "s" } }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        let id = body["id"].as_i64().expect("id returned");

        // Bob (org B): empty list + cross delete masked 404.
        let rows = list_edge(&server, &env.bob, org_b, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 0);
        let cross = format!("/api/v1/edge/wifi-credentials/{id}");
        assert_eq!(del(&server, &env.bob, org_b, &cross).await, 404);

        // Bob, viewer of org A: reads but does not write.
        server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let rows = list_edge(&server, &env.alice, org_a, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 1, "viewer reads the reference data");
        let (status, _) = post_host(&server, &env.bob, org_a, json!({ "host": "h:1" })).await;
        assert_eq!(status, 403, "viewer cannot write");

        // SEC-43: a viewer cannot make the server probe the network.
        let scan = server
            .get("/api/v1/edge/lan-scan?prefix=192.168.1.")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(scan.status_code(), 403);
        assert_eq!(scan.json::<serde_json::Value>()["error"], "access-denied");
    })
    .await;
}

/// SEC-43: an explicit scan prefix must be an RFC 1918 `a.b.c.` prefix —
/// anything else is refused before any probe (400 field-invalid).
#[tokio::test]
#[serial]
async fn lan_scan_prefix_is_rfc1918_only() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        for prefix in ["8.8.8.", "100.64.0.", "127.0.0.", "192.168.1", "nope"] {
            let res = server
                .get(&format!("/api/v1/edge/lan-scan?prefix={prefix}"))
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 400, "{prefix}");
            let body: serde_json::Value = res.json();
            assert_eq!(body["error"], "field-invalid", "{body}");
            assert_eq!(body["errors"]["fields"]["prefix"], "invalid", "{body}");
        }
    })
    .await;
}
