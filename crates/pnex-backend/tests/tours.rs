//! Tests du Studio (parcours 3D versionnés) : cycle création/save/409,
//! versions append-only, publish/share, validation du document (assets +
//! structure), isolation org, rôles — scoping D2, envelope D14.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boot l'app (école `tests/media.rs::with_app` : env posées AVANT le boot).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe {
        std::env::set_var(
            "PNEX_MEDIA_DIR",
            format!(
                "{}/pnex-media-tests-{}",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
    };
    unsafe { std::env::set_var("MEDIA_BACKEND", "fs") };
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

/// Org personnelle de l'utilisateur (créée par JIT provisioning).
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

/// JPEG minimal sans GPano (plan d'étage / photo simple).
fn plain_jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

/// Upload octet-stream (POST /api/v1/media) — pour les assets des scènes.
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

/// Document minimal : un étage sans plan, aucune scène.
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

/// Document avec une scène référençant un asset panorama + un lien.
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
        assert_eq!(created["doc_version_number"], 1, "création → v1");
        assert_eq!(created["latest_version_number"], 1);
        assert_eq!(
            created["doc"]["floors"].as_array().unwrap().len(),
            1,
            "doc minimal : RDC"
        );
        assert!(created["published_version_number"].is_null());
        let tour_id = created["id"].as_str().unwrap().to_string();

        // Liste + search + envelope D14.
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

        // Save v2 : renommage d'étage via doc modifié.
        let mut doc = doc_minimal();
        doc["floors"][0]["name"] = "Rez-de-chaussée".into();
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc).await;
        assert_eq!(res.status_code(), 200, "save v2 : {}", res.text());
        let saved: serde_json::Value = res.json();
        assert_eq!(saved["doc_version_number"], 2);
        assert_eq!(saved["latest_version_number"], 2);

        // Save périmé (expected 1) → 409, aucune v3 surnuméraire.
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc_minimal()).await;
        assert_eq!(res.status_code(), 409, "save périmé → 409");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "tour-version-conflict");

        // Historique : 2 versions, la plus récente d'abord.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["count"], 2, "append-only, pas de v3 surnuméraire");
        assert_eq!(versions["results"][0]["version_number"], 2);
        assert_eq!(versions["results"][0]["author"], "alice");

        // Détail d'une version précise + rechargement ?version=1.
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

        // Version inconnue → 404.
        let res = server
            .get(&format!("/api/v1/tours/{tour_id}/versions/9"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);

        // Delete → 204 puis 404.
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

        // Violation structurelle : lien vers une scène inexistante.
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
        let codes: Vec<&str> = body["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.contains(&"unknown_link_target"),
            "violations : {codes:?}"
        );

        // Asset inexistant (UUID bien formé mais hors org) → 400 {"doc": …}.
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
        assert!(
            body["doc"].as_str().unwrap().contains("inconnu"),
            "per-field doc message: {body}"
        );

        // Asset panorama réel (upload GPano) → save accepté, v2.
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
            "scène panorama valide : {}",
            res.text()
        );
        let saved: serde_json::Value = res.json();
        assert_eq!(
            saved["doc"]["scenes"][0]["media_asset_id"],
            asset_id.as_str()
        );

        // id non-UUID → inconnu (pas de panic).
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

        // Share sans publication → 409.
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

        // Publish (défaut = latest = v1) — POST avec corps vide json
        // (Option<Json> : sans Content-Type l'extracteur répondrait 415).
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

        // Share → token 32 hex, peuplé pour un writer.
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
            "token visible du writer"
        );

        // Régénérer → nouveau token (l'ancien meurt).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_ne!(res["share_token"].as_str().unwrap(), token1);

        // Save v2 (non publiée) puis publish explicite de v1.
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
            "publish ciblé v1"
        );
        assert_eq!(republished["latest_version_number"], 2);

        // Version 1 marquée `published` dans l'historique.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["results"][1]["published"], true, "v1 publiée");
        assert_eq!(versions["results"][0]["published"], false, "v2 non publiée");

        // Unpublish → publication ET lien révoqués.
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let unpublished: serde_json::Value = res.json();
        assert!(unpublished["published_version_number"].is_null());
        assert_eq!(unpublished["share_enabled"], false);
        assert!(unpublished["share_token"].is_null(), "token révoqué");
    })
    .await;
}

/// Document complet : étage avec plan + 2 scènes + lien.
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

        // Kind client prioritaire : des octets GPano déclarés floorplan
        // restent floorplan (le sniff ne reclassifie pas).
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.png&kind=floorplan&content_type=image%2Fpng",
            gpano_jpeg(),
        )
        .await;
        assert_eq!(
            res.status_code(),
            201,
            "kind floorplan accepté : {}",
            res.text()
        );
        let plan = res.json::<serde_json::Value>();
        assert_eq!(
            plan["kind"], "floorplan",
            "kind client prioritaire au sniff"
        );
        let plan_id = plan["id"].as_str().unwrap().to_string();

        // Panorama de scène.
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

        // Un panorama utilisé comme plan → 400 kind requis (un panorama
        // reste interdit en plan : seule une image plate convient).
        let doc = doc_complet(&pano_id, &pano_id);
        let res = save_tour(&server, &env.alice, org, &tour_id, 1, doc).await;
        assert_eq!(res.status_code(), 400, "panorama en plan → 400");
        let body: serde_json::Value = res.json();
        assert!(
            body["doc"]
                .as_str()
                .unwrap()
                .contains("kind floorplan ou photo requis"),
            "message kind : {body}"
        );

        // Une photo (sniff photo, kind non forcé) acceptée comme plan —
        // n'importe quelle image plate s'affiche comme plan d'étage.
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

        // Document complet et cohérent → 200 (kind floorplan dédié). Le
        // save photo ci-dessus est passé en v2 : concurrence optimiste avec
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

        // Assets : plan (floorplan) + panorama de scène.
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.png&kind=floorplan&content_type=image%2Fpng",
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

        // Token inconnu → 404 uniforme.
        let res = server
            .get("/api/v1/public/tours/0123456789abcdef0123456789abcdef")
            .await;
        assert_eq!(res.status_code(), 404, "token inconnu masqué");

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

        // Doc public : sans auth, no-store, doc publié + carte des assets.
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
        // Publish sans version = latest (v2, le doc complet ayant créé v2).
        assert_eq!(body["published_version_number"], 2);
        assert_eq!(body["doc"]["start_scene"], "s1");
        assert_eq!(body["assets"][&pano_id]["version_number"], 1);
        assert_eq!(body["assets"][&pano_id]["content_type"], "image/jpeg");

        // Octets : version courante par défaut, cache immutable.
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{pano_id}"))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.header("cache-control").to_str().unwrap(),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(res.into_bytes(), gpano_jpeg(), "octets exacts du panorama");
        let res = server
            .get(&format!(
                "/api/v1/public/tours/{token}/assets/{pano_id}?v=1"
            ))
            .await;
        assert_eq!(res.status_code(), 200);

        // Version inconnue → 404.
        let res = server
            .get(&format!(
                "/api/v1/public/tours/{token}/assets/{pano_id}?v=9"
            ))
            .await;
        assert_eq!(res.status_code(), 404);

        // Asset NON référencé par le doc publié → 404 (pas de lecture libre).
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
        assert_eq!(res.status_code(), 404, "asset non référencé masqué");

        // Dépublier → 404 uniforme sur le même token (le token seul ne suffit
        // plus).
        let res = server
            .post(&format!("/api/v1/tours/{tour_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let res = server.get(&format!("/api/v1/public/tours/{token}")).await;
        assert_eq!(res.status_code(), 404, "dépublié : token mort");
        let res = server
            .get(&format!("/api/v1/public/tours/{token}/assets/{pano_id}"))
            .await;
        assert_eq!(res.status_code(), 404, "octets morts après dépublication");

        // Republish : la publication revient, mais le token tué par
        // unpublish ne se ressuscite pas (S5 : deux interrupteurs distincts).
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
            "token tué : republish sans re-share"
        );

        // Nouveau share → nouveau token vivant.
        let shared2: serde_json::Value = server
            .post(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await
            .json();
        let token2 = shared2["share_token"].as_str().unwrap().to_string();
        assert_ne!(token2, token, "nouveau token après révocation");
        let res = server.get(&format!("/api/v1/public/tours/{token2}")).await;
        assert_eq!(res.status_code(), 200, "nouveau lien vivant");

        // DELETE /share : révoque le lien en gardant la publication.
        let res = server
            .delete(&format!("/api/v1/tours/{tour_id}/share"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = server.get(&format!("/api/v1/public/tours/{token2}")).await;
        assert_eq!(res.status_code(), 404, "lien révoqué");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            detail["published_version_number"], 2,
            "publication conservée"
        );
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

        // Cross-org : GET/PATCH/publish/share → 404 masqué (jamais 403).
        let res = server
            .get(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "GET cross-org masqué");
        let res = save_tour(&server, &env.bob, org2, &tour_id, 1, doc_minimal()).await;
        assert_eq!(res.status_code(), 404, "PATCH cross-org masqué");
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
            assert_eq!(res.status_code(), 404, "POST cross-org masqué : {path}");
        }

        // Sans X-Org-Id → 400.
        let res = server
            .get("/api/v1/tours")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);

        // Org partagée : alice owner, bob viewer (école tenant_isolation).
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
            "token jamais exposé à un viewer"
        );

        // …écriture interdite (save/publish/share/delete → 403).
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

        // Tour inconnu → 404 (owner lui-même).
        let res = server
            .get("/api/v1/tours/00000000-0000-0000-0000-0000000000ff")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}
