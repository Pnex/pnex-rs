//! Tests référentiels Edge — CRUD wifi-credentials/hosts, **upsert**
//! (200/201, jamais 409), validations 400 champ-par-champ, isolation org,
//! auth 401.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.

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
        .expect("org personnelle")
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

/// GET liste → tableau `results`.
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
async fn wifi_cycle_upsert_et_delete() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Création → 201.
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "Maison", "password": { "value": "secret1" } }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["ssid"], "Maison");
        let id = body["id"].as_i64().expect("id renvoyé");

        // Upsert même SSID, autre mot de passe → 200, même id.
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
            "upsert = mise à jour, pas de doublon"
        );
        // The password lives in the vault: only its reference is returned.
        assert!(!body.to_string().contains("secret2"), "{body}");
        assert_eq!(body["password"]["name"], "wifi/Maison/password");

        // Upsert : pas de doublon en base.
        let rows = list_edge(&server, &env.alice, org, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 1);

        // Delete → 204, puis liste vide et delete → 404.
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

        // SSID vide (espaces).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "  ", "password": { "value": "x" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body.get("ssid").is_some());

        // SSID > 32 octets (802.11).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "a".repeat(33), "password": { "value": "x" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body.get("ssid").is_some());

        // Mot de passe vide (réseaux ouverts hors scope v1).
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org,
            json!({ "ssid": "Maison", "password": { "value": "" } }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(body.get("password").is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn hosts_cycle_upsert_et_validation() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Création → 201.
        let (status, body) = post_host(
            &server,
            &env.alice,
            org,
            json!({ "host": "192.168.1.16:5150" }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        let id = body["id"].as_i64().expect("id renvoyé");
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

        // Validation : schéma interdit (contrat CreateBuild.pnex_host = hôte
        // nu), espace interdit, vide interdit.
        for payload in [
            json!({ "host": "ws://192.168.1.16:5150" }),
            json!({ "host": "192.168.1.16 5150" }),
            json!({ "host": "   " }),
        ] {
            let (status, body) = post_host(&server, &env.alice, org, payload).await;
            assert_eq!(status, 400, "{body}");
            assert!(body.get("host").is_some());
        }

        // Delete → 204.
        let path = format!("/api/v1/edge/hosts/{id}");
        assert_eq!(del(&server, &env.alice, org, &path).await, 204);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_org_et_auth_401() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        // Sans token → 401 (les deux domaines).
        let anon = server.get("/api/v1/edge/wifi-credentials").await;
        assert_eq!(anon.status_code(), 401);
        let anon = server
            .post("/api/v1/edge/hosts")
            .json(&json!({ "host": "h:1" }))
            .await;
        assert_eq!(anon.status_code(), 401);

        // Alice crée sur son org.
        let (status, body) = post_wifi(
            &server,
            &env.alice,
            org_a,
            json!({ "ssid": "Maison", "password": { "value": "s" } }),
        )
        .await;
        assert_eq!(status, 201, "{body}");
        let id = body["id"].as_i64().expect("id renvoyé");

        // Bob (org B) : liste vide + cross delete 404 masqué.
        let rows = list_edge(&server, &env.bob, org_b, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 0);
        let cross = format!("/api/v1/edge/wifi-credentials/{id}");
        assert_eq!(del(&server, &env.bob, org_b, &cross).await, 404);

        // Bob, viewer de l'org A : lit mais n'écrit pas.
        server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let rows = list_edge(&server, &env.alice, org_a, "/api/v1/edge/wifi-credentials").await;
        assert_eq!(rows.len(), 1, "viewer lit le référentiel");
        let (status, _) = post_host(&server, &env.bob, org_a, json!({ "host": "h:1" })).await;
        assert_eq!(status, 403, "viewer ne peut pas écrire");
    })
    .await;
}
