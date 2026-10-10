//! Integration tests of the org geo providers (geo-layers.md §8, phase F):
//! nothing until the org adds a provider, CRUD with the API key in the
//! vault, one default per capability, viewer refused on writes (R2), other
//! org → 404 (R1), and the proxy against a local mock Nominatim (key added
//! server-side only).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use std::sync::Mutex;

use axum::extract::Query;
use axum::routing::get;
use axum::{Json, Router};
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;
use tokio::net::TcpListener;

const STYLE: &str = "https://tiles.example.org/style/light";

struct Env {
    alice: String,
    bob: String,
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

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
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
            f(server, env).await;
        },
    )
    .await;
}

async fn send_json(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<serde_json::Value>,
) -> axum_test::TestResponse {
    let req = match method {
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "DELETE" => server.delete(path),
        _ => server.get(path),
    }
    .add_header("Authorization", bearer(token))
    .add_header("X-Org-Id", org.to_string());
    match body {
        Some(body) => {
            req.add_header("Content-Type", "application/json")
                .json(&body)
                .await
        }
        None => req.await,
    }
}

/// What the mock Nominatim received: (`key` parameter, `q` parameter).
static SEEN: Mutex<Vec<(Option<String>, String)>> = Mutex::new(Vec::new());

async fn spawn_mock_nominatim() -> String {
    async fn search(
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        let key = q.get("key").cloned();
        SEEN.lock()
            .expect("lock")
            .push((key, q.get("q").cloned().unwrap_or_default()));
        Json(serde_json::json!([{
            "display_name": "Tour Eiffel, Paris",
            "lat": "48.8582599", "lon": "2.2945006",
            "type": "attraction", "importance": 0.7
        }]))
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = Router::new().route("/search", get(search));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock nominatim");
    });
    format!("http://{addr}")
}

/// No basemap until the org adds one (like LLM providers); the first one
/// becomes the default and its browser key rides in the style URL.
#[tokio::test]
#[serial]
async fn basemap_only_once_added_with_its_key() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let maps: serde_json::Value = send_json(
            &server,
            "GET",
            "/api/v1/geo/basemaps",
            &env.alice,
            org,
            None,
        )
        .await
        .json();
        assert_eq!(maps, serde_json::json!([]), "nothing seeded");

        let body = serde_json::json!({
            "name": "tiles",
            "kind": "basemap",
            "capabilities": ["basemap"],
            "base_url": STYLE,
            "api_key": { "value": "browser-key" }
        });
        let created = send_json(
            &server,
            "POST",
            "/api/v1/geo/providers",
            &env.alice,
            org,
            Some(body),
        )
        .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        assert!(
            !created.text().contains("browser-key"),
            "{}",
            created.text()
        );

        let maps: serde_json::Value = send_json(
            &server,
            "GET",
            "/api/v1/geo/basemaps",
            &env.alice,
            org,
            None,
        )
        .await
        .json();
        assert_eq!(maps[0]["style_url"], format!("{STYLE}?key=browser-key"));
        assert_eq!(maps[0]["is_default"], true);
    })
    .await;
}

/// CRUD: the key goes to the vault (reference only), the first provider
/// of a capability becomes its default, validation and unique names; the
/// proxy injects the secret and returns normalized hits; no default → 409.
#[tokio::test]
#[serial]
async fn providers_crud_and_geocode_proxy() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        SEEN.lock().expect("lock").clear();

        let none = send_json(
            &server,
            "GET",
            "/api/v1/geo/geocode?q=eiffel",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(none.status_code(), 409, "{}", none.text());
        assert!(
            none.text().contains("geo-not-configured"),
            "{}",
            none.text()
        );

        let url = spawn_mock_nominatim().await;
        let body = serde_json::json!({
            "name": "osm",
            "kind": "nominatim",
            "capabilities": ["geocode", "reverse"],
            "base_url": url,
            "api_key": { "value": "s3cret" },
            "rate_limit_per_s": 100.0
        });
        let created = send_json(
            &server,
            "POST",
            "/api/v1/geo/providers",
            &env.alice,
            org,
            Some(body.clone()),
        )
        .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        assert!(!created.text().contains("s3cret"), "secret never returned");
        let p: serde_json::Value = created.json();
        assert_eq!(p["api_key"]["name"], "geo/osm/auth", "{p}");
        assert_eq!(p["key_param"], "key");
        assert_eq!(p["default_for"], serde_json::json!(["geocode", "reverse"]));

        let dup = send_json(
            &server,
            "POST",
            "/api/v1/geo/providers",
            &env.alice,
            org,
            Some(body),
        )
        .await;
        assert_eq!(dup.status_code(), 409, "{}", dup.text());

        let bad = serde_json::json!({
            "name": "v", "kind": "nominatim", "capabilities": ["route"],
            "base_url": "https://x.example"
        });
        let bad = send_json(
            &server,
            "POST",
            "/api/v1/geo/providers",
            &env.alice,
            org,
            Some(bad),
        )
        .await;
        assert_eq!(bad.status_code(), 400, "{}", bad.text());

        let hits = send_json(
            &server,
            "GET",
            "/api/v1/geo/geocode?q=eiffel",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(hits.status_code(), 200, "{}", hits.text());
        let hits: serde_json::Value = hits.json();
        assert_eq!(hits[0]["label"], "Tour Eiffel, Paris");
        assert_eq!(hits[0]["lat"], 48.8582599);
        let seen = SEEN.lock().expect("lock").clone();
        assert_eq!(
            seen.first(),
            Some(&(Some("s3cret".to_string()), "eiffel".to_string()))
        );
    })
    .await;
}

/// A viewer lists providers and uses the proxy but cannot write (R2).
#[tokio::test]
#[serial]
async fn viewer_cannot_manage_providers() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let _ = personal_org(&server, &env.bob).await;
        let add = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"email": "bob@example.com", "role": "viewer"}))
            .await;
        assert_eq!(add.status_code(), 201, "{}", add.text());

        let body = serde_json::json!({
            "name": "osm", "kind": "nominatim", "capabilities": ["geocode"],
            "base_url": "https://nominatim.example"
        });
        let created = send_json(
            &server,
            "POST",
            "/api/v1/geo/providers",
            &env.alice,
            org,
            Some(body.clone()),
        )
        .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let list = send_json(&server, "GET", "/api/v1/geo/providers", &env.bob, org, None).await;
        assert_eq!(list.status_code(), 200, "{}", list.text());
        let id = list.json::<serde_json::Value>()[0]["id"]
            .as_str()
            .expect("provider")
            .to_string();
        for (method, path) in [
            ("POST", "/api/v1/geo/providers".to_string()),
            ("PUT", format!("/api/v1/geo/providers/{id}")),
            ("DELETE", format!("/api/v1/geo/providers/{id}")),
            ("POST", format!("/api/v1/geo/providers/{id}/test")),
        ] {
            let resp = send_json(&server, method, &path, &env.bob, org, Some(body.clone())).await;
            assert_eq!(resp.status_code(), 403, "{method} {path}: {}", resp.text());
            assert!(
                resp.text().contains("geo-provider-forbidden"),
                "{}",
                resp.text()
            );
        }

        // Another org's provider does not exist for bob's own org (R1).
        let orgs = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json::<serde_json::Value>()["orgs"]
            .clone();
        let bob_org = orgs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["id"].as_i64())
            .find(|id| *id != org)
            .expect("bob's personal org");
        let other = send_json(
            &server,
            "DELETE",
            &format!("/api/v1/geo/providers/{id}"),
            &env.bob,
            bob_org,
            None,
        )
        .await;
        assert_eq!(other.status_code(), 404, "{}", other.text());
    })
    .await;
}
