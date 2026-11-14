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
        assert_eq!(maps["count"], 0, "nothing seeded");
        assert_eq!(maps["results"], serde_json::json!([]));

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
        assert_eq!(
            maps["results"][0]["style_url"],
            format!("{STYLE}?key=browser-key")
        );
        assert_eq!(maps["results"][0]["is_default"], true);
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
        assert_eq!(hits["results"][0]["label"], "Tour Eiffel, Paris");
        assert_eq!(hits["results"][0]["lat"], 48.8582599);
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
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
        .await;

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
        let id = list.json::<serde_json::Value>()["results"][0]["id"]
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
            assert!(resp.text().contains("access-denied"), "{}", resp.text());
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

/// Square polygon feature around (lon, lat) with a `code` property.
fn square(code: &str, lon: f64, lat: f64) -> serde_json::Value {
    let d = 0.5;
    serde_json::json!({
        "type": "Feature", "id": code, "properties": { "code": code },
        "geometry": { "type": "Polygon", "coordinates": [[
            [lon - d, lat - d], [lon + d, lat - d], [lon + d, lat + d],
            [lon - d, lat + d], [lon - d, lat - d]
        ]] }
    })
}

#[tokio::test]
#[serial]
async fn layers_from_geojson_served_as_tiles() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let body = serde_json::json!({
            "name": "departments",
            "style": [{ "type": "fill", "paint": { "fill-opacity": 0.4 } }],
            "features": { "type": "FeatureCollection",
                          "features": [square("75", 2.35, 48.85), square("69", 4.83, 45.76)] }
        });
        let created = send_json(&server, "POST", "/api/v1/geo/layers", &env.alice, org, Some(body.clone())).await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let layer = created.json::<serde_json::Value>();
        assert_eq!(layer["geometry_type"], "polygon");
        assert_eq!(layer["feature_count"], 2);
        assert_eq!(layer["generation"], 1);
        let extent = layer["extent"].as_array().expect("extent");
        assert!((extent[0].as_f64().unwrap() - 1.85).abs() < 1e-6, "{extent:?}");
        let id = layer["id"].as_str().unwrap().to_string();

        let again = send_json(&server, "POST", "/api/v1/geo/layers", &env.alice, org, Some(body)).await;
        assert_eq!(again.status_code(), 409, "{}", again.text());
        assert!(again.text().contains("geo-layer-name-taken"));

        // Mixed families and broken geometries are refused, nothing written.
        let mixed = serde_json::json!({ "name": "mixed", "features": { "type": "FeatureCollection",
            "features": [square("1", 0.0, 0.0),
                         { "type": "Feature", "geometry": { "type": "Point", "coordinates": [0, 0] } }] } });
        let resp = send_json(&server, "POST", "/api/v1/geo/layers", &env.alice, org, Some(mixed)).await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        assert!(resp.text().contains("mixed_geometry"), "{}", resp.text());
        let broken = serde_json::json!({ "name": "broken", "features": { "type": "FeatureCollection",
            "features": [{ "type": "Feature", "geometry": { "type": "Polygon", "coordinates": [[[0, 0]]] } }] } });
        let resp = send_json(&server, "POST", "/api/v1/geo/layers", &env.alice, org, Some(broken)).await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        let list = send_json(&server, "GET", "/api/v1/geo/layers", &env.alice, org, None).await;
        assert_eq!(list.json::<serde_json::Value>()["count"], 1, "{}", list.text());

        // z0 holds both squares; the Paris tile at z8 only one; a far tile none.
        let tile = |z: u32, x: u32, y: u32| format!("/api/v1/geo/layers/{id}/tiles/1/{z}/{x}/{y}");
        let world = send_json(&server, "GET", &tile(0, 0, 0), &env.alice, org, None).await;
        assert_eq!(world.status_code(), 200);
        assert_eq!(world.header("content-type"), "application/vnd.mapbox-vector-tile");
        assert!(world.header("cache-control").to_str().unwrap().contains("immutable"));
        let bytes = world.as_bytes();
        let has = |b: &[u8], s: &str| b.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has(bytes, "features") && has(bytes, "75") && has(bytes, "69"));
        let paris = send_json(&server, "GET", &tile(8, 129, 88), &env.alice, org, None).await;
        assert!(has(paris.as_bytes(), "75") && !has(paris.as_bytes(), "69"));
        let far = send_json(&server, "GET", &tile(8, 0, 0), &env.alice, org, None).await;
        assert_eq!(far.status_code(), 200);
        assert!(far.as_bytes().is_empty());
        let stale = send_json(&server, "GET", &format!("/api/v1/geo/layers/{id}/tiles/0/0/0/0"), &env.alice, org, None).await;
        assert_eq!(stale.header("cache-control"), "private, no-cache");
        let bad = send_json(&server, "GET", &tile(1, 2, 0), &env.alice, org, None).await;
        assert_eq!(bad.status_code(), 400, "{}", bad.text());

        // Another org sees neither the layer nor its tiles (R1).
        let bob_org = personal_org(&server, &env.bob).await;
        let other = send_json(&server, "GET", &tile(0, 0, 0), &env.bob, bob_org, None).await;
        assert_eq!(other.status_code(), 404, "{}", other.text());

        // A viewer reads tiles, never writes (R2).
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "viewer").await;
        let read = send_json(&server, "GET", &tile(0, 0, 0), &env.bob, org, None).await;
        assert_eq!(read.status_code(), 200);
        for (method, path) in [("POST", "/api/v1/geo/layers".to_string()), ("DELETE", format!("/api/v1/geo/layers/{id}"))] {
            let resp = send_json(&server, method, &path, &env.bob, org, Some(serde_json::json!({}))).await;
            assert_eq!(resp.status_code(), 403, "{method} {path}: {}", resp.text());
        }

        let gone = send_json(&server, "DELETE", &format!("/api/v1/geo/layers/{id}"), &env.alice, org, None).await;
        assert_eq!(gone.status_code(), 204);
        let gone = send_json(&server, "GET", &tile(0, 0, 0), &env.alice, org, None).await;
        assert_eq!(gone.status_code(), 404);
    })
    .await;
}
