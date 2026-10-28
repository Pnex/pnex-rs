//! Studio tests (versioned 3D tours): creation/save/409 cycle,
//! append-only versions, publish/share, document validation (assets +
//! structure), org isolation, roles — D2 scoping, D14 envelope.
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

/// Boots the app (as in `tests/media.rs::with_app`: env set BEFORE the boot).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
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
            f(server, env).await;
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
        .expect("org personnelle")
}

fn gpano_jpeg() -> Vec<u8> {
    let prefix = b"http://ns.adobe.com/xap/1.0/\x00";
    let xmp = r#"<rdf:Description GPano:ProjectionType="equirectangular"/>"#;
    let content = [prefix.as_slice(), xmp.as_bytes()].concat();
    let len = (content.len() + 2) as u16;
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&content);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// Minimal JPEG without GPano (floor plan / plain photo).
fn plain_jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

/// Octet-stream upload (POST /api/v1/media) — for the scene assets.
async fn upload(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
    body: Vec<u8>,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/media{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(body.into())
        .await
}

async fn create_tour(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
) -> serde_json::Value {
    server
        .post("/api/v1/tours")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "name": name }))
        .await
        .json::<serde_json::Value>()
}

/// Minimal document: one floor without a plan, no scene.
fn doc_minimal() -> serde_json::Value {
    serde_json::json!({
        "mode": "panorama",
        "start_scene": null,
        "floors": [
            {"id": "f1", "name": "RDC", "level": 0, "plan": null, "north_deg": 0.0}
        ],
        "scenes": [],
        "links": []
    })
}

/// Document with a scene referencing a panorama asset + a link.
fn doc_avec_scene(asset_id: &str, link_target: Option<&str>) -> serde_json::Value {
    let mut links = Vec::new();
    if let Some(target) = link_target {
        links.push(serde_json::json!({
            "id": "l1", "from": "s1", "to": target,
            "yaw": 90.0, "pitch": 0.0, "kind": "walk", "label": "Suite"
        }));
    }
    serde_json::json!({
        "mode": "panorama",
        "start_scene": "s1",
        "floors": [
            {"id": "f1", "name": "RDC", "level": 0, "plan": null, "north_deg": 0.0}
        ],
        "scenes": [
            {"id": "s1", "floor_id": "f1", "label": "Salon",
             "media_asset_id": asset_id,
             "x": 120.0, "y": 300.0,
             "initial_yaw": 0.0, "initial_pitch": 0.0, "initial_fov": 100.0}
        ],
        "links": links
    })
}

async fn save_tour(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    tour_id: &str,
    expected: i64,
    doc: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .patch(&format!("/api/v1/tours/{tour_id}"))
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "expected_version_number": expected,
            "doc": doc,
            "author": "alice"
        }))
        .await
}

#[tokio::test]
#[serial]
async fn cycle_creation_save_versions_409() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_tour(&server, &env.alice, org, "Visite atelier").await;
        assert_eq!(created["mode"], "panorama");
        assert_eq!(created["doc_version_number"], 1, "creation → v1");
        assert_eq!(created["latest_version_number"], 1);
        assert_eq!(
            created["doc"]["floors"].as_array().unwrap().len(),
            1,
            "doc minimal : RDC"
        );
        assert!(created["published_version_number"].is_null());
        let tour_id = created["id"].as_str().unwrap().to_string();

        // List + search + D14 envelope.
        let listed: serde_json::Value = server
            .get("/api/v1/tours?search=atelier")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1);
        let listed: serde_json::Value = server
            .get("/api/v1/tours?search=introuvable")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // Save v2: floor rename via a modified doc.
        let mut doc = doc_minimal();
        doc["floors"][0]["name"] = "Rez-de-chaussée".into();
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc).await;
        assert_eq!(res.status_code(), 200, "save v2 : {}", res.text());
        let saved: serde_json::Value = res.json();
        assert_eq!(saved["doc_version_number"], 2);
        assert_eq!(saved["latest_version_number"], 2);

        // Stale save (expected 1) → 409, no extra v3.
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc_minimal()).await;
        assert_eq!(res.status_code(), 409, "stale save → 409");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "tour-version-conflict");

        // History: 2 versions, most recent first.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["count"], 2, "append-only, no extra v3");
        assert_eq!(versions["results"][0]["version_number"], 2);
        assert_eq!(versions["results"][0]["author"], "alice");

        // Detail of a specific version + reload ?version=1.
        let v1: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            v1["doc"]["floors"][0]["name"], "Ground floor",
            "v1 immuable"
        );
        let old: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}?version=1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(old["doc_version_number"], 1);
        assert_eq!(old["latest_version_number"], 2);

        // Unknown version → 404.
        let res = server
            .get(&format!("/api/v1/tours/{tour_id}/versions/9"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);

        // Delete → 204 then 404.
        let res = server
            .delete(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validation_doc_structure_et_assets() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_tour(&server, &env.alice, org, "Validation").await;
        let tour_id = created["id"].as_str().unwrap().to_string();

        // Structural violation: link to a nonexistent scene.
        let res = save_tour(
            &server,
            &env.alice,
            org,
            &tour_id,
            1,
            doc_avec_scene("00000000-0000-0000-0000-0000000000aa", Some("zz")),
        )
        .await;
        assert_eq!(res.status_code(), 400, "lien orphelin → 400");
        let body: serde_json::Value = res.json();
        let codes: Vec<&str> = body["errors"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.contains(&"unknown_link_target"),
            "violations : {codes:?}"
        );

        // Unknown asset (well-formed UUID outside the org) → `tour-invalid`
        // violation `unknown_asset` on that asset.
        let res = save_tour(
            &server,
            &env.alice,
            org,
            &tour_id,
            1,
            doc_avec_scene("00000000-0000-0000-0000-0000000000bb", None),
        )
        .await;
        assert_eq!(res.status_code(), 400, "asset inconnu → 400");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "tour-invalid", "{body}");
        assert_eq!(body["errors"]["violations"][0]["code"], "unknown_asset");
        assert_eq!(
            body["errors"]["violations"][0]["subject"],
            "00000000-0000-0000-0000-0000000000bb"
        );

        // Real panorama asset (GPano upload) → save accepted, v2.
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let asset_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let res = save_tour(
            &server,
            &env.alice,
            org,
            &tour_id,
            1,
            doc_avec_scene(&asset_id, None),
        )
        .await;
        assert_eq!(
            res.status_code(),
            200,
            "valid panorama scene: {}",
            res.text()
        );
        let saved: serde_json::Value = res.json();
        assert_eq!(
            saved["doc"]["scenes"][0]["media_asset_id"],
            asset_id.as_str()
        );

        // non-UUID id → unknown (no panic).
        let res = save_tour(
            &server,
            &env.alice,
            org,
            &tour_id,
            2,
            doc_avec_scene("pas-un-uuid", None),
        )
        .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn publish_share_cycle() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = create_tour(&server, &env.alice, org, "Publiable").await;
        let tour_id = created["id"].as_str().unwrap().to_string();

        // Share without publication → 409.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 409, "share avant publish → 409");
        let body: serde_json::Value = res.json();
        // Loco CustomError body: {"error": code, "description": message}.
        assert_eq!(body["error"], "tour-not-published");
        assert_eq!(
            body["description"],
            "Publish a version before creating a public share link"
        );

        // Publish (default = latest = v1) — POST with an empty json body
        // (Option<Json>: without Content-Type the extractor would answer 415).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/publish"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 200, "publish : {}", res.text());
        let published: serde_json::Value = res.json();
        assert_eq!(published["published_version_number"], 1);

        // Share → 32 hex token, populated for a writer.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let shared: serde_json::Value = res.json();
        let token1 = shared["share_token"].as_str().unwrap().to_string();
        assert_eq!(token1.len(), 32, "token = 32 hex");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["share_enabled"], true);
        assert_eq!(
            detail["share_token"],
            token1.as_str(),
            "token visible to the writer"
        );

        // Regenerate → new token (the old one dies).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_ne!(res["share_token"].as_str().unwrap(), token1);

        // Save v2 (unpublished) then explicit publish of v1.
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc_minimal()).await;
        assert_eq!(res.status_code(), 200);
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/publish"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "version_number": 1 }))
            .await;
        let republished: serde_json::Value = res.json();
        assert_eq!(
            republished["published_version_number"], 1,
            "targeted publish v1"
        );
        assert_eq!(republished["latest_version_number"], 2);

        // Version 1 marked `published` in the history.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["results"][1]["published"], true, "v1 published");
        assert_eq!(versions["results"][0]["published"], false, "v2 unpublished");

        // Unpublish → publication AND link revoked.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let unpublished: serde_json::Value = res.json();
        assert!(unpublished["published_version_number"].is_null());
        assert_eq!(unpublished["share_enabled"], false);
        assert!(unpublished["share_token"].is_null(), "token revoked");
    })
    .await;
}

/// Full document: floor with a plan + 2 scenes + a link.
fn doc_complet(plan_id: &str, pano_id: &str) -> serde_json::Value {
    serde_json::json!({
        "mode": "panorama",
        "start_scene": "s1",
        "floors": [
            {"id": "f1", "name": "RDC", "level": 0,
             "plan": {"media_asset_id": plan_id, "width": 2048.0, "height": 1024.0,
                      "scale_m_per_px": 0.05},
             "north_deg": 0.0}
        ],
        "scenes": [
            {"id": "s1", "floor_id": "f1", "label": "Salon",
             "media_asset_id": pano_id,
             "x": 512.0, "y": 300.0,
             "initial_yaw": 0.0, "initial_pitch": 0.0, "initial_fov": 100.0},
            {"id": "s2", "floor_id": "f1", "label": "Cuisine",
             "media_asset_id": pano_id,
             "x": 900.0, "y": 300.0,
             "initial_yaw": 90.0, "initial_pitch": 0.0, "initial_fov": 100.0}
        ],
        "links": [
            {"id": "l1", "from": "s1", "to": "s2", "yaw": 90.0, "pitch": 0.0,
             "kind": "walk", "label": "Vers cuisine"}
        ]
    })
}

#[tokio::test]
#[serial]
async fn kind_floorplan_et_plan_d_etage() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Client kind takes priority: GPano bytes declared floorplan
        // stay floorplan (the sniff doesn't reclassify).
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.jpg&kind=floorplan&content_type=image%2Fpng",
            gpano_jpeg(),
        )
        .await;
        assert_eq!(
            res.status_code(),
            201,
            "floorplan kind accepted: {}",
            res.text()
        );
        let plan = res.json::<serde_json::Value>();
        assert_eq!(
            plan["kind"], "floorplan",
            "client kind takes precedence over sniffing"
        );
        let plan_id = plan["id"].as_str().unwrap().to_string();

        // Scene panorama.
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await;
        let pano_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let created = create_tour(&server, &env.alice, org, "Avec plan").await;
        let tour_id = created["id"].as_str().unwrap().to_string();

        // A panorama used as a plan → 400 kind required (a panorama
        // stays forbidden as a plan: only a flat image fits).
        let doc = doc_complet(&pano_id, &pano_id);
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc).await;
        assert_eq!(res.status_code(), 400, "panorama en plan → 400");
        let body: serde_json::Value = res.json();
        let violation = &body["errors"]["violations"][0];
        assert_eq!(violation["code"], "asset_kind_mismatch", "{body}");
        assert!(
            violation["message"]
                .as_str()
                .unwrap()
                .contains("kind floorplan or photo required"),
            "message kind : {body}"
        );

        // A photo (photo sniff, kind not forced) accepted as a plan —
        // any flat image displays as a floor plan.
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=photo-plan.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 201, "upload photo : {}", res.text());
        let photo_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let doc = doc_complet(&photo_id, &pano_id);
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc).await;
        assert_eq!(
            res.status_code(),
            200,
            "photo en plan → 200 : {}",
            res.text()
        );

        // Full and consistent document → 200 (dedicated floorplan kind). The
        // photo save above went to v2: optimistic concurrency with
        // expected=2.
        let doc = doc_complet(&plan_id, &pano_id);
        let res = save_tour(&server, &env.alice, org, &tour_id, 2, doc).await;
        assert_eq!(res.status_code(), 200, "doc complet : {}", res.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn endpoint_public_et_cache() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Assets: plan (floorplan) + scene panorama. The floor plan is
        // declared `text/html` by the client (SEC-5 attack shape).
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.jpg&kind=floorplan&content_type=text%2Fhtml",
            plain_jpeg(),
        )
        .await;
        let plan_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await;
        let pano_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        let created = create_tour(&server, &env.alice, org, "Public").await;
        let tour_id = created["id"].as_str().unwrap().to_string();
        let res = save_tour(
            &server,
            &env.alice,
            org,
            &tour_id,
            1,
            doc_complet(&plan_id, &pano_id),
        )
        .await;
        assert_eq!(res.status_code(), 200);

        // Unknown token → uniform 404.
        let res = server
            .get("/api/v1/public/tours/0123456789abcdef0123456789abcdef")
            .await;
        assert_eq!(res.status_code(), 404, "unknown token masked");

        // Share avant publish → 409.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 409);

        // Publish + share.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/publish"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 200);
        let shared: serde_json::Value = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await
            .json();
        let token = shared["share_token"].as_str().unwrap().to_string();

        // Public doc: no auth, no-store, published doc + asset map.
        let res = server.get(&format!("/api/v1/public/tours/{token}")).await;
        assert_eq!(res.status_code(), 200, "public doc : {}", res.text());
        assert!(
            res.header("cache-control")
                .to_str()
                .unwrap()
                .contains("no-store"),
            "doc public no-store"
        );
        let body: serde_json::Value = res.json();
        assert_eq!(body["name"], "Public");
        // Publish without a version = latest (v2, the full doc having created v2).
        assert_eq!(body["published_version_number"], 2);
        assert_eq!(body["doc"]["start_scene"], "s1");
        assert_eq!(body["assets"][&pano_id]["version_number"], 1);
        assert_eq!(body["assets"][&pano_id]["content_type"], "image/jpeg");

        // Bytes: current version by default; the unversioned URL follows
        // the current version, so it is revalidated, never immutable (O74).
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{pano_id}"))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.header("cache-control").to_str().unwrap(), "no-cache");
        assert_eq!(
            res.header("x-content-type-options").to_str().unwrap(),
            "nosniff"
        );
        assert_eq!(res.into_bytes(), gpano_jpeg(), "exact panorama bytes");

        // SEC-5: a client-declared HTML type is never served as such on the
        // public (app-origin) route.
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{plan_id}"))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.header("content-type").to_str().unwrap(),
            "application/octet-stream"
        );
        assert!(res
            .header("content-disposition")
            .to_str()
            .unwrap()
            .starts_with("attachment"));
        assert!(res
            .header("content-security-policy")
            .to_str()
            .unwrap()
            .contains("sandbox"));
        let res = server
            .get(&format!(
                "/api/v1/public/tours/{token}/assets/{pano_id}?v=1"
            ))
            .await;
        assert_eq!(res.status_code(), 200);
        // A versioned URL is append-only bytes: immutable for a year (O74).
        assert_eq!(
            res.header("cache-control").to_str().unwrap(),
            "public, max-age=31536000, immutable"
        );

        // Unknown version → 404.
        let res = server
            .get(&format!(
                "/api/v1/public/tours/{token}/assets/{pano_id}?v=9"
            ))
            .await;
        assert_eq!(res.status_code(), 404);

        // Asset NOT referenced by the published doc → 404 (no free reading).
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=hors-doc.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await;
        let outsider = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{outsider}"))
            .await;
        assert_eq!(res.status_code(), 404, "unreferenced asset masked");

        // Unpublish → uniform 404 on the same token (the token alone is no
        // longer enough).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let res = server.get(&format!("/api/v1/public/tours/{token}")).await;
        assert_eq!(res.status_code(), 404, "unpublished: dead token");
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{pano_id}"))
            .await;
        assert_eq!(res.status_code(), 404, "dead bytes after unpublishing");

        // Republish: the publication comes back, but the token killed by
        // unpublish isn't revived (S5: two separate switches).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/publish"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 200);
        let res = server.get(&format!("/api/v1/public/tours/{token}")).await;
        assert_eq!(
            res.status_code(),
            404,
            "token killed: republish without re-share"
        );

        // New share → new live token.
        let shared2: serde_json::Value = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await
            .json();
        let token2 = shared2["share_token"].as_str().unwrap().to_string();
        assert_ne!(token2, token, "new token after revocation");
        let res = server.get(&format!("/api/v1/public/tours/{token2}")).await;
        assert_eq!(res.status_code(), 200, "nouveau lien vivant");

        // DELETE /share: revokes the link while keeping the publication.
        let res = server
            .delete(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = server.get(&format!("/api/v1/public/tours/{token2}")).await;
        assert_eq!(res.status_code(), 404, "link revoked");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["published_version_number"], 2, "publication kept");
        assert_eq!(detail["share_enabled"], false);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_org_et_roles() {
    with_app(|server, env| async move {
        let org1 = personal_org(&server, &env.alice).await;
        let org2 = personal_org(&server, &env.bob).await;
        let created = create_tour(&server, &env.alice, org1, "Privé alice").await;
        let tour_id = created["id"].as_str().unwrap().to_string();

        // Cross-org: GET/PATCH/publish/share → masked 404 (never 403).
        let res = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org GET masked");
        let res = save_tour(&server, &env.bob, org2, &tour_id, 1, doc_minimal()).await;
        assert_eq!(res.status_code(), 404, "cross-org PATCH masked");
        for path in [
            format!("/api/v1/tours/{tour_id}/publish"),
            format!("/api/v1/tours/{tour_id}/share"),
        ] {
            let res = server
                .post(&path)
                .add_header("Content-Type", "application/json")
                .add_header("Authorization", bearer(&env.bob))
                .add_header("X-Org-Id", org2.to_string())
                .json(&serde_json::json!({}))
                .await;
            assert_eq!(res.status_code(), 404, "cross-org POST masked: {path}");
        }

        // Without X-Org-Id → 400.
        let res = server
            .get("/api/v1/tours")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);

        // Shared org: alice owner, bob viewer (as in tenant_isolation).
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }
        let created_org: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Tours" }))
            .await
            .json();
        let shared_org = created_org["id"].as_i64().unwrap();
        server
            .post(&format!("/api/v1/orgs/{shared_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;

        let listed_in_shared = create_tour(&server, &env.alice, shared_org, "Partagé").await;
        let shared_tour = listed_in_shared["id"].as_str().unwrap().to_string();

        // Viewer : lecture OK…
        let res = server
            .get(&format!("/api/v1/tours/{shared_tour}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert!(
            body["share_token"].is_null(),
            "token never exposed to a viewer"
        );

        // …writes forbidden (save/publish/share/delete → 403).
        let res = save_tour(
            &server,
            &env.bob,
            shared_org,
            &shared_tour,
            1,
            doc_minimal(),
        )
        .await;
        assert_eq!(res.status_code(), 403, "viewer save → 403");
        for path in [
            format!("/api/v1/tours/{shared_tour}/publish"),
            format!("/api/v1/tours/{shared_tour}/share"),
        ] {
            let res = server
                .post(&path)
                .add_header("Content-Type", "application/json")
                .add_header("Authorization", bearer(&env.bob))
                .add_header("X-Org-Id", shared_org.to_string())
                .json(&serde_json::json!({}))
                .await;
            assert_eq!(res.status_code(), 403, "viewer → 403 : {path}");
        }
        let res = server
            .delete(&format!("/api/v1/tours/{shared_tour}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer delete → 403 avant 404");

        // Unknown tour → 404 (the owner themself).
        let res = server
            .get("/api/v1/tours/00000000-0000-0000-0000-0000000000ff")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}
