//! Tests for the cross-cutting organisation layer (D42): label CRUD + validation,
//! effective labels (containment inheritance, override), containment (single
//! parent, cycle, re-parent), edges (validity per kind, duplicate 409, symmetric
//! purge), folders, effective label filter on the media list,
//! org isolation + roles.
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

// ─────────────────────────── helpers HTTP ───────────────────────────

async fn jget(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    url: &str,
) -> axum_test::TestResponse {
    server
        .get(url)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
}

async fn jput(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    url: &str,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .put(url)
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await
}

async fn jpost(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    url: &str,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post(url)
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await
}

async fn jdelete(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    url: &str,
) -> axum_test::TestResponse {
    server
        .delete(url)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
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

/// Media asset (UUID) inserted via the upload API.
async fn create_media(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let res = server
        .post("/api/v1/media?name=Test&filename=loop.jpg&kind=photo&content_type=application/octet-stream")
        .add_header("Content-Type", "application/octet-stream")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .bytes(vec![0xFF, 0xD8, 0xFF, 0xD9].into())
        .await;
    assert_eq!(res.status_code(), 201, "media upload: {}", res.text());
    let _ = name;
    res.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("media id")
        .to_string()
}

// ─────────────────────────── labels ───────────────────────────

#[tokio::test]
#[serial]
async fn labels_roundtrip_validation_et_isolation() {
    with_app(|server, _db, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let media = create_media(&server, &env.alice, org_a, "loop").await;

        // PUT labels
        let res = server
            .put(&format!("/api/v1/resources/media_asset/{media}/labels"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "labels": {"site": "serre", "critique": null}
            }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["labels"]["site"], "serre");
        // Validation: non-normalized name -> 400
        let res = server
            .put(&format!("/api/v1/resources/media_asset/{media}/labels"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "labels": {"Site": "serre"}
            }))
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["labels"],
            "invalid"
        );

        // Isolation: bob doesn't see alice's labels (404 masks)
        let res = server
            .get(&format!("/api/v1/resources/media_asset/{media}/labels"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masque");
    })
    .await;
}

// ─────────────────────────── labels: own only (D218) ───────────────────────────

#[tokio::test]
#[serial]
async fn labels_are_never_inherited_through_folders() {
    with_app(|server, db, env| async move {
        let _ = &db;
        let org = personal_org(&server, &env.alice).await;
        let media = create_media(&server, &env.alice, org, "pano").await;
        let folder_id = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "Serre", "emoji": "🌿"}),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_i64()
            .unwrap();
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder_id}/labels"),
            serde_json::json!({"labels": {"site": "serre", "critique": null}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": folder_id.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // The folder's labels stay on the folder.
        for q in ["site:serre", "critique"] {
            let res = jget(
                &server,
                &env.alice,
                org,
                &format!("/api/v1/media?label={q}"),
            )
            .await;
            assert_eq!(res.json::<serde_json::Value>()["count"], 0, "{q}");
        }
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/labels"),
            serde_json::json!({"labels": {"site": "exterieur"}}),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let res = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/media?label=site:exterieur",
        )
        .await;
        assert_eq!(res.json::<serde_json::Value>()["count"], 1);
        // The inheritance route is gone.
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/labels/effective"),
        )
        .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

// ─────────────────────────── containment ───────────────────────────

#[tokio::test]
#[serial]
async fn containment_parent_unique_cycle_et_reparent() {
    with_app(|server, db, env| async move {
        let _ = &db;
        let org = personal_org(&server, &env.alice).await;
        let media = create_media(&server, &env.alice, org, "photo").await;

        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "A"}),
        )
        .await;
        let a = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "B"}),
        )
        .await;
        let b = res.json::<serde_json::Value>()["id"].as_i64().unwrap();

        // A in B.
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{a}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": b.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // Cycle: B in A → 400 (A is a descendant of B).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{b}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": a.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "cycle refuse");
        assert_eq!(res.json::<serde_json::Value>()["error"], "resource-cycle");

        // Single parent: the media re-parented from A → B (move = 1 PUT).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": a.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": b.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["parent"]["kind"], "folder");
        assert_eq!(body["parent"]["id"], b.to_string());
        assert_eq!(body["children"].as_array().unwrap().len(), 0);
        // A is INSIDE B: the media's path (placed in B) = [B] only.
        assert_eq!(body["path"].as_array().unwrap().len(), 1, "chemin media>B");

        // Detach: null parent → root (subtree intact, nothing deleted).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": null}),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert!(body["parent"].is_null());

        // Unknown parent → masked 404.
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": "999999"}}),
        )
        .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

// ─────────────────────────── drawer POI ───────────────────────────

/// POI drawer: the POI contains **folders** (no direct objects) and the
/// children carry their display name for all kinds (registry hydration).
#[tokio::test]
#[serial]
async fn map_pin_contient_folders_pas_objets() {
    with_app(|server, db, env| async move {
        let _ = db;
        let org = personal_org(&server, &env.alice).await;
        let poi = create_poi(&server, &env.alice, org).await;
        let media = create_media(&server, &env.alice, org, "pano").await;

        // Folder under the POI → 200.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "Électrique"}),
        )
        .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let fid = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{fid}/containment"),
            serde_json::json!({"parent": {"kind": "map_pin", "id": poi}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // The POI's children carry the folder's name.
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/map_pin/{poi}/containment"),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["children"][0]["kind"], "folder");
        assert_eq!(body["children"][0]["name"], "Électrique");

        // Media directly under the POI → 400 (map_pin only contains
        // folders).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "map_pin", "id": poi}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "direct objects refused");

        // Media in the folder → 200, and the folder lists its display name
        // (registry hydration, all kinds).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": fid.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{fid}/containment"),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["children"][0]["kind"], "media_asset");
        assert_eq!(body["children"][0]["name"], "Test");

        // Cycle: the POI can't be filed into its own folder.
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/map_pin/{poi}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": fid.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "cycle POI>folder>POI refused");
    })
    .await;
}

// ─────────────────────────── edges ───────────────────────────

#[tokio::test]
#[serial]
async fn edges_validite_doublon_et_purge_symetrique() {
    with_app(|server, db, env| async move {
        let _ = &db;
        let org = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let poi = create_poi(&server, &env.alice, org).await;
        let media = create_media(&server, &env.alice, org, "pano").await;

        // Undeclared relation (device has no relation) → 400.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "device",
                "source_id": "1",
                "target_kind": "media_asset",
                "target_id": media
            }),
        )
        .await;
        assert_eq!(
            res.status_code(),
            400,
            "device has no placed_on in its spec"
        );

        // Unknown POI → masked 404.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": "00000000-0000-0000-0000-00000000dead",
                "target_kind": "media_asset",
                "target_id": media
            }),
        )
        .await;
        assert_eq!(res.status_code(), 404);

        // Edge OK + hotspot placement.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": poi,
                "target_kind": "media_asset",
                "target_id": media,
                "placement": {"hotspot": [0.5, 0.5], "label": "Vue pano"}
            }),
        )
        .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let edge_id = res.json::<serde_json::Value>()["id"].as_i64().unwrap();

        // Doublon → 409.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": poi,
                "target_kind": "media_asset",
                "target_id": media
            }),
        )
        .await;
        assert_eq!(res.status_code(), 409, "duplicate edge");

        // Cross-org edge: bob targets alice's media → 404.
        let poi_b = create_poi(&server, &env.bob, org_b).await;
        let res = jpost(
            &server,
            &env.bob,
            org_b,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": poi_b,
                "target_kind": "media_asset",
                "target_id": media
            }),
        )
        .await;
        assert_eq!(res.status_code(), 404, "cross-org target masked");

        // PATCH placement.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": poi,
                "target_kind": "dashboard",
                "target_id": "00000000-0000-0000-0000-00000000dead"
            }),
        )
        .await;
        assert_eq!(res.status_code(), 404, "dashboard inconnu");

        // Symmetric purge: deleting the MEDIA (target) purges the edge.
        let res = jdelete(&server, &env.alice, org, &format!("/api/v1/media/{media}")).await;
        assert_eq!(res.status_code(), 204);
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!(
                "/api/v1/resources/edges?relation=placed_on&source_kind=map_pin&source_id={poi}"
            ),
        )
        .await;
        assert_eq!(
            res.json::<serde_json::Value>()["count"],
            0,
            "symmetric purge: the edge went away with its target"
        );
        let _ = edge_id;
    })
    .await;
}

/// Minimal geo POI via the API (local helper).
async fn create_poi(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let res = jpost(
        server,
        token,
        org,
        "/api/v1/pois",
        serde_json::json!({"label": "POI test", "latitude": 45.7, "longitude": 4.8}),
    )
    .await;
    assert_eq!(res.status_code(), 201, "{}", res.text());
    res.json::<serde_json::Value>()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

// ─────────────────────────── recherche cross-kind ───────────────────────────

#[tokio::test]
#[serial]
async fn recherche_effective_cross_kind_et_folders() {
    with_app(|server, db, env| async move {
        let _ = &db;
        let org = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let media = create_media(&server, &env.alice, org, "pano-serre").await;

        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "Serre"}),
        )
        .await;
        let folder = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let _ = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder}/labels"),
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await;
        let _ = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": folder.to_string()}}),
        )
        .await;

        // Device: labelled directly (no folder) — a live kind in the
        // cross-kind search.
        let dev = create_device(&server, &env.alice, org, "cap-serre").await;
        let dev_id = dev["id"].as_i64().unwrap().to_string();
        let _ = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/device/{dev_id}/labels"),
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await;

        // Recherche cross-kind.
        let res = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/search?label=site:serre",
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body: serde_json::Value = res.json();
        // Own labels only (D218): the media in the folder inherits nothing.
        assert_eq!(body["count"], 2, "device + folder : {}", body);

        // Filtered kinds: device only.
        let res = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/search?label=site:serre&kinds=device",
        )
        .await;
        let body: serde_json::Value = res.json();
        assert_eq!(body["count"], 1);
        assert_eq!(body["results"][0]["kind"], "device");

        // Folders: list + PATCH + delete (the labelled folder dies, the
        // media stays rooted).
        let res = jget(&server, &env.alice, org, "/api/v1/resources/folders").await;
        assert_eq!(res.json::<serde_json::Value>()["count"], 1);
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "X"}),
        )
        .await;
        let x = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let res = jdelete(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folders/{x}"),
        )
        .await;
        assert_eq!(res.status_code(), 204);
        // The media in "Serre" is still there (folder isolation).
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder}/containment"),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        // Isolation: bob does not see alice's tree.
        let res = jget(
            &server,
            &env.bob,
            org_b,
            &format!("/api/v1/resources/folder/{folder}/containment"),
        )
        .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

/// Catalogue device (temp_sensor) attached to the org — local helper.
async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    slug: &str,
) -> serde_json::Value {
    let res = jpost(
        server,
        token,
        org,
        "/api/v1/devices",
        serde_json::json!({"device_id": slug, "predefined_device_name": "temp_sensor"}),
    )
    .await;
    assert_eq!(res.status_code(), 201, "create device : {}", res.text());
    res.json()
}

// ──────────────────── label= on the other lists (D42) ────────────────────

#[tokio::test]
#[serial]
async fn label_filter_on_devices_and_dashboards_lists() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        // Two devices, one labelled (a folder label is never inherited).
        let mut devices = Vec::new();
        for id in ["lbl-dev-a", "lbl-dev-b"] {
            let res = jpost(
                &server,
                &env.alice,
                org,
                "/api/v1/devices",
                serde_json::json!({"device_id": id, "predefined_device_name": "generic_esp8266"}),
            )
            .await;
            res.assert_status(axum_test::http::StatusCode::CREATED);
            devices.push(res.json::<serde_json::Value>()["id"].as_i64().unwrap());
        }
        let folder_id = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "Serre lbl"}),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_i64()
            .unwrap();
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder_id}/labels"),
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await
        .assert_status_ok();
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/device/{}/containment", devices[1]),
            serde_json::json!({"parent": {"kind": "folder", "id": folder_id.to_string()}}),
        )
        .await
        .assert_status_ok();
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/device/{}/labels", devices[0]),
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await
        .assert_status_ok();

        let list = jget(&server, &env.alice, org, "/api/v1/devices?label=site:serre").await;
        list.assert_status_ok();
        let body: serde_json::Value = list.json();
        let ids: Vec<i64> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![devices[0]]);
        assert_eq!(body["count"], 1);
        let bad = jget(&server, &env.alice, org, "/api/v1/devices?label=Bad%20Name").await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);

        // Dashboards: own label.
        let dash = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({"name": "lbl-dash"}),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards",
            serde_json::json!({"name": "lbl-dash-other"}),
        )
        .await;
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/dashboard/{dash}/labels"),
            serde_json::json!({"labels": {"critique": null}}),
        )
        .await
        .assert_status_ok();
        let body: serde_json::Value = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/dashboards?label=critique",
        )
        .await
        .json();
        let ids: Vec<&str> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![dash.as_str()]);

        // Another org never matches alice's labels.
        let bob_org = personal_org(&server, &env.bob).await;
        let body: serde_json::Value = jget(
            &server,
            &env.bob,
            bob_org,
            "/api/v1/dashboards?label=critique",
        )
        .await
        .json();
        assert_eq!(body["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn label_filter_on_flows_list() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let graph = serde_json::json!({
            "nodes": [
                {
                    "id": "n1", "kind": "inject",
                    "config": { "repeat_secs": 5.0, "payload": {"k": 1} },
                    "outputs": [{ "port": 0, "targets": ["n2"] }]
                },
                { "id": "n2", "kind": "debug", "config": {} }
            ]
        });
        let mut flows = Vec::new();
        for name in ["lbl-flow-a", "lbl-flow-b"] {
            let res = jpost(
                &server,
                &env.alice,
                org,
                "/api/v1/flows",
                serde_json::json!({"name": name, "graph": graph.clone()}),
            )
            .await;
            res.assert_status(axum_test::http::StatusCode::CREATED);
            flows.push(res.json::<serde_json::Value>()["id"].as_i64().unwrap());
        }
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/flow/{}/labels", flows[1]),
            serde_json::json!({"labels": {"zone": "nord"}}),
        )
        .await
        .assert_status_ok();

        let body: serde_json::Value =
            jget(&server, &env.alice, org, "/api/v1/flows?label=zone:nord")
                .await
                .json();
        let ids: Vec<i64> = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![flows[1]]);
        assert_eq!(body["count"], 1);
        // Bare name matches any value.
        let body: serde_json::Value = jget(&server, &env.alice, org, "/api/v1/flows?label=zone")
            .await
            .json();
        assert_eq!(body["count"], 1);
        // Malformed filter → 400 on the `label` field.
        let bad = jget(&server, &env.alice, org, "/api/v1/flows?label=Bad%20Name").await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        // Another org never sees alice's labelled flow.
        let bob_org = personal_org(&server, &env.bob).await;
        let body: serde_json::Value = jget(&server, &env.bob, bob_org, "/api/v1/flows?label=zone")
            .await
            .json();
        assert_eq!(body["count"], 0);
    })
    .await;
}

// ──────────────────── location breadcrumb (sites tree) ────────────────────

#[tokio::test]
#[serial]
async fn location_breadcrumb_follows_folders_and_site_links() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let poi = create_poi(&server, &env.alice, org).await;
        let media = create_media(&server, &env.alice, org, "loc-pano").await;
        let other = create_media(&server, &env.alice, org, "loc-other").await;

        let folder = |name: &'static str| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                jpost(
                    server,
                    &token,
                    org,
                    "/api/v1/resources/folders",
                    serde_json::json!({"name": name}),
                )
                .await
                .json::<serde_json::Value>()["id"]
                    .as_i64()
                    .unwrap()
            }
        };
        let building = folder("Bâtiment A").await;
        let room = folder("Chaufferie").await;
        for (child, parent) in [
            (
                format!("/api/v1/resources/folder/{building}/containment"),
                serde_json::json!({"kind": "map_pin", "id": poi}),
            ),
            (
                format!("/api/v1/resources/folder/{room}/containment"),
                serde_json::json!({"kind": "folder", "id": building.to_string()}),
            ),
            (
                format!("/api/v1/resources/media_asset/{media}/containment"),
                serde_json::json!({"kind": "folder", "id": room.to_string()}),
            ),
        ] {
            jput(
                &server,
                &env.alice,
                org,
                &child,
                serde_json::json!({"parent": parent}),
            )
            .await
            .assert_status_ok();
        }

        // In a folder: site, then the folders from the top down.
        let body: serde_json::Value = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/location"),
        )
        .await
        .json();
        let loc = &body["locations"][0];
        assert_eq!(loc["site"]["kind"], "map_pin");
        assert_eq!(loc["site"]["id"], poi.as_str());
        let names: Vec<&str> = loc["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Bâtiment A", "Chaufferie"]);

        // Attached at the site root (placed_on edge): site only.
        jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/edges",
            serde_json::json!({
                "relation": "placed_on",
                "source_kind": "map_pin",
                "source_id": poi,
                "target_kind": "media_asset",
                "target_id": other
            }),
        )
        .await
        .assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{other}/location"),
        )
        .await
        .json();
        assert_eq!(body["locations"].as_array().unwrap().len(), 1);
        assert_eq!(body["locations"][0]["folders"].as_array().unwrap().len(), 0);

        // Another org never sees it (R1).
        let bob_org = personal_org(&server, &env.bob).await;
        jget(
            &server,
            &env.bob,
            bob_org,
            &format!("/api/v1/resources/media_asset/{media}/location"),
        )
        .await
        .assert_status(axum_test::http::StatusCode::NOT_FOUND);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn labels_batch_returns_a_page_and_stays_in_the_org() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = create_media(&server, &env.alice, org, "batch-a").await;
        let b = create_media(&server, &env.alice, org, "batch-b").await;
        jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{a}/labels"),
            serde_json::json!({"labels": {"site": "serre", "critique": null}}),
        )
        .await
        .assert_status_ok();
        let body: serde_json::Value = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/labels/query",
            serde_json::json!({"kind": "media_asset", "ids": [a, b]}),
        )
        .await
        .json();
        assert_eq!(body["labels"][&a]["site"], "serre");
        assert!(body["labels"][&a]["critique"].is_null());
        assert!(body["labels"].get(&b).is_none());

        // Bob asks for alice's ids from his org: nothing comes back.
        let bob_org = personal_org(&server, &env.bob).await;
        let body: serde_json::Value = jpost(
            &server,
            &env.bob,
            bob_org,
            "/api/v1/resources/labels/query",
            serde_json::json!({"kind": "media_asset", "ids": [a]}),
        )
        .await
        .json();
        assert_eq!(body["labels"], serde_json::json!({}));
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/labels/query",
            serde_json::json!({"kind": "nope", "ids": []}),
        )
        .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn labels_catalog_lists_org_names_and_values_only() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = create_media(&server, &env.alice, org, "cat-a").await;
        let b = create_media(&server, &env.alice, org, "cat-b").await;
        for (id, labels) in [
            (&a, serde_json::json!({"site": "serre", "critique": null})),
            (&b, serde_json::json!({"site": "atelier"})),
        ] {
            jput(
                &server,
                &env.alice,
                org,
                &format!("/api/v1/resources/media_asset/{id}/labels"),
                serde_json::json!({ "labels": labels }),
            )
            .await
            .assert_status_ok();
        }
        let body: serde_json::Value =
            jget(&server, &env.alice, org, "/api/v1/resources/labels/catalog")
                .await
                .json();
        let site = body["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "site")
            .expect("site listed");
        assert_eq!(site["count"], 2);
        let mut values: Vec<&str> = site["values"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        values.sort_unstable();
        assert_eq!(values, ["atelier", "serre"]);

        // Bob's org sees none of alice's labels.
        let bob_org = personal_org(&server, &env.bob).await;
        let body: serde_json::Value = jget(
            &server,
            &env.bob,
            bob_org,
            "/api/v1/resources/labels/catalog",
        )
        .await
        .json();
        assert_eq!(body["results"], serde_json::json!([]));
    })
    .await;
}

// ─────────────────────────── label-only kinds (D218) ───────────────────────────

#[tokio::test]
#[serial]
async fn label_only_kind_filter_search_and_purge() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let tpl = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/notify/templates",
            serde_json::json!({"name": "seuil serre", "subject": "s", "body": "b", "vars": []}),
        )
        .await;
        assert_eq!(tpl.status_code(), 201, "{}", tpl.text());
        let id = tpl.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let path = format!("/api/v1/resources/notify_template/{id}/labels");

        let res = jput(
            &server,
            &env.alice,
            org,
            &path,
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        assert_eq!(
            jget(&server, &env.alice, org, &path)
                .await
                .json::<serde_json::Value>()["labels"]["site"],
            "serre"
        );
        // Another org cannot see or write it; an unknown row is a 404.
        assert_eq!(
            jget(&server, &env.bob, org_b, &path).await.status_code(),
            404
        );
        let unknown =
            "/api/v1/resources/notify_template/00000000-0000-0000-0000-000000000000/labels";
        assert_eq!(
            jget(&server, &env.alice, org, unknown).await.status_code(),
            404
        );

        // List filter of its own page.
        let list = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/notify/templates?label=site:serre",
        )
        .await;
        assert_eq!(list.json::<serde_json::Value>()["count"], 1);
        let list = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/notify/templates?label=site:nord",
        )
        .await;
        assert_eq!(list.json::<serde_json::Value>()["count"], 0);

        // Global search: `#value` finds it in the labels group with its kind.
        let found: serde_json::Value = jget(&server, &env.alice, org, "/api/v1/search?q=%23serre")
            .await
            .json();
        let group = found["groups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["entity_type"] == "label")
            .unwrap_or_else(|| panic!("no label group: {found}"));
        assert_eq!(group["results"][0]["kind"], "notify_template", "{found}");
        assert_eq!(group["results"][0]["title"], "seuil serre", "{found}");
        assert_eq!(group["results"][0]["subtitle"], "site:serre", "{found}");

        // Deleting the row drops its labels (database trigger).
        let res = jdelete(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/notify/templates/{id}"),
        )
        .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let res = jget(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/search?label=site:serre",
        )
        .await;
        assert_eq!(res.json::<serde_json::Value>()["count"], 0, "labels purged");
    })
    .await;
}
