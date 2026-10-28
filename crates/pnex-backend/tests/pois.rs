//! POI-first map tests (D35–D39, D43): POI CRUD, multiple device
//! placements + move + detach (D43 and amendment), backend clustering
//! (D37), links (D39 → D42 `resource_edges`), GPS positions (D38 —
//! observed + manual), org isolation (masked 404), roles (read-only
//! viewer), D14 envelope.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, sea_orm::DatabaseConnection, Env) -> Fut,
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
            f(server, ctx.db, env).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// The user's personal org (created by JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

/// Catalogue device (`temp_sensor`, as in builds.rs) attached to the org.
async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    slug: &str,
) -> serde_json::Value {
    let res = server
        .post("/api/v1/devices")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "device_id": slug,
            "predefined_device_name": "temp_sensor"
        }))
        .await;
    assert_eq!(res.status_code(), 201, "create device : {}", res.text());
    res.json()
}

async fn create_poi(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    label: &str,
    lat: f64,
    lon: f64,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/pois")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "label": label,
            "latitude": lat,
            "longitude": lon
        }))
        .await
}

/// D43: attaches an existing device to a POI (POST /pois/{id}/devices).
async fn attach_device(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    poi_id: &str,
    slug: &str,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/pois/{poi_id}/devices"))
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "device_id": slug }))
        .await
}

async fn get_cluster(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    extra: &str,
) -> axum_test::TestResponse {
    server
        .get(&format!(
            "/api/v1/pois/cluster?bbox=4.0,44.0,6.0,50.0&zoom=10&{extra}"
        ))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
}

// ─────────────────────────── CRUD + D14 ───────────────────────────

#[tokio::test]
#[serial]
async fn full_poi_cycle() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // 201 + default emoji 📍 + location_detail.
        let res = server
            .post("/api/v1/pois")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "label": "Serre Nord",
                "location_detail": "Bât. A — Étage 2 — Salle 204",
                "latitude": 45.764043,
                "longitude": 4.835659
            }))
            .await;
        assert_eq!(res.status_code(), 201, "create : {}", res.text());
        let poi = res.json::<serde_json::Value>();
        let poi_id = poi["id"].as_str().expect("id").to_string();
        assert_eq!(poi["emoji"], "📍", "default emoji");
        assert_eq!(poi["mode"], "geo");
        assert_eq!(
            poi["location_detail"], "Bât. A — Étage 2 — Salle 204",
            "free-form building location"
        );

        // List: D14 envelope + case-insensitive search.
        let listed: serde_json::Value = server
            .get("/api/v1/pois?limit=10&offset=0")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1);
        assert!(listed["next"].is_null());
        assert!(listed["previous"].is_null());
        assert_eq!(listed["results"].as_array().expect("results").len(), 1);
        let listed: serde_json::Value = server
            .get("/api/v1/pois?search=serre")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "case-insensitive search");
        let listed: serde_json::Value = server
            .get("/api/v1/pois?search=absent")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // PATCH: move + custom emoji + location_detail → null.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "latitude": 45.77,
                "longitude": 4.84,
                "emoji": "🌱",
                "location_detail": null
            }))
            .await;
        assert_eq!(res.status_code(), 200, "patch : {}", res.text());
        let patched = res.json::<serde_json::Value>();
        assert_eq!(patched["latitude"], 45.77);
        assert_eq!(patched["emoji"], "🌱");
        assert!(patched["location_detail"].is_null(), "null double-Option");

        // PATCH reset emoji ("" → default) + cross-org 404 further down.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "emoji": null }))
            .await;
        assert_eq!(
            res.json::<serde_json::Value>()["emoji"],
            "📍",
            "reset to default"
        );

        // DELETE → 204 → 404.
        let res = server
            .delete(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = server
            .get(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "deleted = masked 404");
    })
    .await;
}

// ──────────────── pinned preview ────────────────

#[tokio::test]
#[serial]
async fn poi_preview_pin() {
    with_app(|server, db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let headers = |r: axum_test::TestRequest| {
            r.add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
        };
        let poi = server
            .post("/api/v1/pois")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "label": "Serre Épinglée",
                "latitude": 45.75,
                "longitude": 4.85
            }))
            .await;
        assert_eq!(poi.status_code(), 201, "{}", poi.text());
        let poi_id = poi.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_string();

        // Setting the pin (kind+id pair) → 200 + fields echoed.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "preview_kind": "media_asset",
                "preview_id": "11111111-2222-3333-4444-555555555555"
            }))
            .await;
        assert_eq!(res.status_code(), 200, "set pin: {}", res.text());
        let dto = res.json::<serde_json::Value>();
        assert_eq!(dto["preview_kind"], "media_asset");
        assert_eq!(dto["preview_id"], "11111111-2222-3333-4444-555555555555");

        // GET detail → the fields survive the round-trip.
        let dto: serde_json::Value = headers(server.get(&format!("/api/v1/pois/{poi_id}")))
            .await
            .json();
        assert_eq!(dto["preview_kind"], "media_asset");

        // Kind outside the list → 400 preview_kind.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "preview_kind": "flow",
                "preview_id": "x"
            }))
            .await;
        assert_eq!(
            res.status_code(),
            400,
            "kind outside the list: {}",
            res.text()
        );
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(body["errors"]["fields"]["preview_kind"], "invalid");

        // Pose without an id → 400 (the pair lives together).
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "preview_kind": "tour" }))
            .await;
        assert_eq!(res.status_code(), 400, "kind without an id: {}", res.text());

        // Unpinning: null on either field → both cleared.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "preview_id": null }))
            .await;
        assert_eq!(res.status_code(), 200, "unpin : {}", res.text());
        let dto = res.json::<serde_json::Value>();
        assert!(dto["preview_kind"].is_null(), "kind cleared with the id");
        assert!(dto["preview_id"].is_null());

        // Deleting the pinned resource drops the pin (no viewer on a dead id).
        let gone = "22222222-3333-4444-5555-666666666666";
        let res = headers(server.patch(&format!("/api/v1/pois/{poi_id}")))
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "preview_kind": "tour", "preview_id": gone }))
            .await;
        assert_eq!(res.status_code(), 200, "pin tour : {}", res.text());
        pnex_backend::services::resources::purge_for(&db, org, "tour", gone)
            .await
            .expect("purge");
        let dto: serde_json::Value = headers(server.get(&format!("/api/v1/pois/{poi_id}")))
            .await
            .json();
        assert!(dto["preview_kind"].is_null(), "pin dropped with the tour");
        assert!(dto["preview_id"].is_null());
    })
    .await;
}

// ──────────────── D43: multiple placements + move ────────────────

#[tokio::test]
#[serial]
async fn multiple_placements_and_move() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "soil-01").await;
        create_device(&server, &env.alice, org, "soil-02").await;

        // POI A; the device_id field inherited from D36 is ignored on creation
        // (attaching is deliberately done via POST /pois/{id}/devices).
        let res = create_poi(&server, &env.alice, org, "POI A", 45.7, 4.8).await;
        assert_eq!(res.status_code(), 201, "A : {}", res.text());
        let poi_a = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id A")
            .to_string();
        assert!(
            res.json::<serde_json::Value>()["devices"]
                .as_array()
                .expect("devices")
                .is_empty(),
            "new POI without a device"
        );
        let res = create_poi(&server, &env.alice, org, "POI B", 45.8, 4.9).await;
        assert_eq!(res.status_code(), 201, "B : {}", res.text());
        let poi_b = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id B")
            .to_string();

        // Two devices on the same POI (D43: several allowed).
        let res = attach_device(&server, &env.alice, org, &poi_a, "soil-01").await;
        assert_eq!(res.status_code(), 201, "soil-01 : {}", res.text());
        let placement = res.json::<serde_json::Value>();
        let placement_id = placement["id"].as_i64().expect("placement id");
        assert_eq!(placement["device_id"], "soil-01");
        assert!(placement["location_detail"].is_null());
        let res = attach_device(&server, &env.alice, org, &poi_a, "soil-02").await;
        assert_eq!(
            res.status_code(),
            201,
            "second device on the same POI: {}",
            res.text()
        );
        let listed: serde_json::Value = server
            .get(&format!("/api/v1/pois/{poi_a}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed["devices"].as_array().expect("devices").len(),
            2,
            "POI multi-devices"
        );

        // Unknown device → 400 device_id field.
        let res = attach_device(&server, &env.alice, org, &poi_a, "inexistant-01").await;
        assert_eq!(res.status_code(), 400, "unknown device: {}", res.text());
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["device_id"],
            "not-found"
        );

        // Already placed — same POI or another — → 409 carrying the current placement
        // (the front offers the move, never silent).
        let res = attach_device(&server, &env.alice, org, &poi_a, "soil-01").await;
        assert_eq!(res.status_code(), 409, "re-attach same POI: {}", res.text());
        // The current placement travels in `errors.args` (strings).
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "poi-device-already-placed");
        let conflict = &body["errors"]["args"];
        assert_eq!(conflict["device_id"], "soil-01");
        assert_eq!(conflict["current_pin_id"], poi_a, "same POI");
        assert_eq!(conflict["current_pin_label"], "POI A");
        assert!(
            conflict["current_placement_id"]
                .as_str()
                .is_some_and(|id| id.parse::<i64>().is_ok()),
            "id de placement fourni"
        );
        let res = attach_device(&server, &env.alice, org, &poi_b, "soil-01").await;
        assert_eq!(
            res.status_code(),
            409,
            "device already placed elsewhere: {}",
            res.text()
        );

        // location_detail per placement: set then clear (null double-Option).
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "location_detail": "Rack 3 — Allée B" }))
            .await;
        assert_eq!(res.status_code(), 200, "location : {}", res.text());
        assert_eq!(
            res.json::<serde_json::Value>()["location_detail"],
            "Rack 3 — Allée B"
        );
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "location_detail": null }))
            .await;
        assert_eq!(res.status_code(), 200, "clear : {}", res.text());
        assert!(res.json::<serde_json::Value>()["location_detail"].is_null());

        // Moving soil-01 to B (one removal out of two now —
        // detaching exists too, D43 amendment).
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "pin_id": poi_b }))
            .await;
        assert_eq!(res.status_code(), 200, "move : {}", res.text());
        let listed: serde_json::Value = server
            .get(&format!("/api/v1/pois/{poi_a}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed["devices"].as_array().expect("devices").len(),
            1,
            "A lost soil-01 (move, not copy)"
        );
        let listed_b: serde_json::Value = server
            .get(&format!("/api/v1/pois/{poi_b}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed_b["devices"].as_array().expect("devices")[0]["device_id"],
            "soil-01",
            "B gained soil-01"
        );

        // Unknown target POI → 400 pin_id field; unknown placement → 404.
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "pin_id": "00000000-0000-0000-0000-0000000000ff"
            }))
            .await;
        assert_eq!(res.status_code(), 400, "unknown target: {}", res.text());
        assert!(res.text().contains("pin_id"));
        let res = server
            .patch("/api/v1/pois/placements/999999")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "location_detail": "nulle part" }))
            .await;
        assert_eq!(res.status_code(), 404, "unknown placement masked");

        // Detach (D43 amendment, 2026-09-13): soil-01 becomes free again,
        // then reattaches wherever we want — here back on B.
        let res = server
            .delete(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204, "detach : {}", res.text());
        let listed: serde_json::Value = server
            .get(&format!("/api/v1/pois/{poi_a}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed["devices"].as_array().expect("devices").len(),
            1,
            "A only has soil-02 left (soil-01 detached)"
        );
        let res = attach_device(&server, &env.alice, org, &poi_b, "soil-01").await;
        assert_eq!(
            res.status_code(),
            201,
            "re-attach after detach: {}",
            res.text()
        );
        let res = server
            .delete("/api/v1/pois/placements/999999")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "unknown placement masked");

        // Deleting the POI → placements cascade: soil-01 becomes placeable
        // again (it "must be placed again", warning on the UI side).
        let res = server
            .delete(&format!("/api/v1/pois/{poi_b}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = attach_device(&server, &env.alice, org, &poi_a, "soil-01").await;
        assert_eq!(
            res.status_code(),
            201,
            "cascade POI delete: soil-01 placeable again: {}",
            res.text()
        );
        let placement_id = res.json::<serde_json::Value>()["id"].as_i64().expect("id");

        // Hardening taken from the D36 test: location_detail > 255, latitude
        // outside WGS84, emoji too long.
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "location_detail": "x".repeat(256)
            }))
            .await;
        assert_eq!(res.status_code(), 400, "location > 255");
        assert!(res.text().contains("location_detail"));
        let res = create_poi(&server, &env.alice, org, "Pôle", 95.0, 4.8).await;
        assert_eq!(res.status_code(), 400, "latitude out of bounds");
        assert!(res.text().contains("latitude"));
        let res = server
            .post("/api/v1/pois")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "label": "Emoji", "emoji": "0123456789",
                "latitude": 45.7, "longitude": 4.8
            }))
            .await;
        assert_eq!(res.status_code(), 400, "emoji > 8 chars");
        assert!(res.text().contains("emoji"));
    })
    .await;
}

// ─────────────────────────── D37 : clustering ───────────────────────────

#[tokio::test]
#[serial]
async fn cluster_pois_and_filters() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "soil-01").await;
        create_device(&server, &env.alice, org, "soil-02").await;

        // 3 POIs in Lyon (one with 2 attached devices — D43), 1 in Paris.
        let _ = create_poi(&server, &env.alice, org, "Lyon 1", 45.7640, 4.8356).await;
        let res = create_poi(&server, &env.alice, org, "Lyon 2 device", 45.7650, 4.8360).await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let lyon2 = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id Lyon 2")
            .to_string();
        let res = attach_device(&server, &env.alice, org, &lyon2, "soil-01").await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let res = attach_device(&server, &env.alice, org, &lyon2, "soil-02").await;
        assert_eq!(
            res.status_code(),
            201,
            "2 devices on Lyon 2: {}",
            res.text()
        );
        let _ = create_poi(&server, &env.alice, org, "Paris", 48.8566, 2.3522).await;

        // Grid of 205 tight POIs in Lyon (exceeds CLUSTER_INDIVIDUAL_MAX,
        // a cluster can only appear above the threshold).
        for i in 0..205 {
            let res = server
                .post("/api/v1/pois")
                .add_header("Content-Type", "application/json")
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
                .json(&serde_json::json!({
                    "label": format!("Grille {i}"),
                    "latitude": 45.7630 + f64::from(i) * 0.00002,
                    "longitude": 4.8350 + f64::from(i) * 0.00002
                }))
                .await;
            assert_eq!(res.status_code(), 201, "grille {i} : {}", res.text());
        }

        // Zoom 10: cell ~0.044° ≫ intra-grid spacing → aggregation
        // (Paris is outside this bbox).
        let res = get_cluster(&server, &env.alice, org, "").await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 207, "2 POIs + 205 from the grid");
        let items = body["items"].as_array().expect("items");
        assert!(
            items.len() < body["total"].as_u64().unwrap() as usize,
            "aggregated at low zoom"
        );
        assert!(
            items.iter().any(|i| i["count"].as_u64() == Some(207)),
            "a single occupied cell at zoom 10"
        );
        assert!(
            items.iter().all(|i| i["id"].is_null()),
            "cluster ≠ point individuel"
        );
        let summed: u64 = items.iter().filter_map(|i| i["count"].as_u64()).sum();
        assert_eq!(summed, 207, "no point lost");

        // Bbox widened to Paris: Lyon cluster (207) + Paris singleton (1).
        let res = server
            .get("/api/v1/pois/cluster?bbox=2.0,44.0,6.0,50.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 208);
        let items = body["items"].as_array().expect("items");
        assert_eq!(items.len(), 2, "1 cluster + 1 singleton");
        assert_eq!(items[0]["count"], 207, "sorted by density desc");
        assert_eq!(items[1]["count"], 1);
        assert!(items[1]["id"].is_string(), "a singleton carries its id");

        // Zoom 22: tiny cell → individual points with ids.
        let res = server
            .get("/api/v1/pois/cluster?bbox=4.83,45.75,4.85,45.78&zoom=22")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 207, "tight Lyon bbox");
        let items = body["items"].as_array().expect("items");
        assert!(items.iter().all(|i| i["count"] == 1), "individual points");
        assert!(items.iter().all(|i| i["id"].is_string()), "id present");

        // has_device filter: only Lyon 2 (2 placements = counted ONCE).
        let body = get_cluster(&server, &env.alice, org, "has_device=true")
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["total"], 1, "has_device filter, no join duplicate");

        // search filter: by label…
        let body = get_cluster(&server, &env.alice, org, "search=lyon")
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["total"], 2, "search filter");
        // …and by the slug of a placed device (D43: haystack via placements).
        let body = get_cluster(&server, &env.alice, org, "search=soil-01")
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["total"], 1, "search by placed slug");

        // invalid bbox -> 400 per-field error.
        let res = server
            .get("/api/v1/pois/cluster?bbox=6.0,44.0,4.0,50.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400, "west>east rejected");
        let res = server
            .get("/api/v1/pois/cluster?bbox=4.0,44.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400, "incomplete bbox");
    })
    .await;
}

// ──────────────────── D39 → D42: links (`placed_on` edges) ────────────────────

#[tokio::test]
#[serial]
async fn observed_positions() {
    with_app(|server, db, env| async move {
        use pnex_backend::services::{pois as svc, telemetry::TelemetryPoint};
        let org = personal_org(&server, &env.alice).await;
        let device = create_device(&server, &env.alice, org, "tracker-01").await;
        let registry_id = device["id"].as_i64().expect("registry id");

        // Empty layer at the start.
        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // Ingestion path: separate `latitude` then `longitude` frames
        // (D38 convention) via the service observed by the sink tap.
        let point = |metric: &str, value: &str| TelemetryPoint {
            org_id: org,
            device_registry_id: registry_id,
            device_id: "tracker-01".into(),
            pred_dev: "temp_sensor".into(),
            metric_name: metric.into(),
            value: value.into(),
            timestamp: chrono::Utc::now(),
            ts_source: "server",
            source_type: "sensor",
            record: true,
        };
        svc::observe(&db, &point("latitude", "45.764043"))
            .await
            .expect("observe lat");
        // No pair yet → nothing in the database.
        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0, "half position is invisible");
        svc::observe(&db, &point("longitude", "4.835659"))
            .await
            .expect("observe lon");

        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "complete pair = position");
        let pos = &listed["results"].as_array().expect("results")[0];
        assert_eq!(pos["device_id"], "tracker-01");
        assert_eq!(pos["source"], "telemetry");
        let lat = pos["latitude"].as_f64().expect("lat");
        assert!((lat - 45.764043).abs() < 1e-5, "lat : {lat}");

        // has_position filter: the POI carrying the tracker becomes visible
        // (D43 attachment: create then POST devices).
        let res = create_poi(&server, &env.alice, org, "POI tracker", 45.76, 4.83).await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let poi_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_string();
        let res = attach_device(&server, &env.alice, org, &poi_id, "tracker-01").await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let listed: serde_json::Value = server
            .get("/api/v1/pois?has_position=true")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "has_position filter");
    })
    .await;
}

// ─────────────────────────── isolation + roles ───────────────────────────

#[tokio::test]
#[serial]
async fn org_isolation_and_viewer_roles() {
    with_app(|server, _db, env| async move {
        // JIT boot of both users.
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }
        // Shared org: alice owner, bob viewer (as in tenant_isolation).
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Co" }))
            .await
            .json();
        let shared_org = created["id"].as_i64().expect("shared org id");
        server
            .post(&format!("/api/v1/orgs/{shared_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;

        let org = personal_org(&server, &env.alice).await;
        let res = create_poi(&server, &env.alice, org, "POI Privé", 45.7, 4.8).await;
        let poi_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_string();

        // Bob on HIS org: alice's POI is a masked 404 (read/write).
        let bob_org = personal_org(&server, &env.bob).await;
        let res = server
            .get(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masked");
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .json(&serde_json::json!({"label": "Détourné"}))
            .await;
        assert_eq!(res.status_code(), 404);
        let res = server
            .delete(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
        // Bob's list empty (no leak).
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // In the shared org: bob the viewer reads, but doesn't write.
        let _ = create_poi(&server, &env.alice, shared_org, "POI Commun", 45.7, 4.8).await;
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "viewer reads the list");
        let res = server
            .post("/api/v1/pois")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .json(&serde_json::json!({"label": "Interdit", "latitude": 45.7, "longitude": 4.8}))
            .await;
        assert_eq!(res.status_code(), 403, "viewer doesn't write");
        // A viewer cannot attach a device either (D43).
        create_device(&server, &env.alice, shared_org, "soil-shared").await;
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", shared_org.to_string())
            .await
            .json();
        let shared_poi = listed["results"].as_array().expect("results")[0]["id"]
            .as_str()
            .expect("shared POI id")
            .to_string();
        let res = attach_device(&server, &env.bob, shared_org, &shared_poi, "soil-shared").await;
        assert_eq!(res.status_code(), 403, "viewer does not attach");
        // A viewer can't detach either (D43 amendment).
        let res = attach_device(&server, &env.alice, shared_org, &shared_poi, "soil-shared").await;
        assert_eq!(res.status_code(), 201, "alice attaches: {}", res.text());
        let detach_id = res.json::<serde_json::Value>()["id"].as_i64().expect("id");
        let res = server
            .delete(&format!("/api/v1/pois/placements/{detach_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer doesn't detach");
        let res = server
            .delete(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(
            res.status_code(),
            404,
            "alice's POI invisible from the shared org"
        );
    })
    .await;
}
