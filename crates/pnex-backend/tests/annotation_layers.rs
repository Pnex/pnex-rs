//! Tests des couches d'annotations (D55–D60) : cycle création/save/409,
//! versions append-only, publish/unpublish, validation (assets + devices),
//! read model (union couches publiées, dead device), isolation org, rôles.
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

async fn create_layer(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
) -> serde_json::Value {
    server
        .post("/api/v1/annotation-layers")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "name": name }))
        .await
        .json::<serde_json::Value>()
}

async fn save_layer(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    layer_id: &str,
    expected: i64,
    doc: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .patch(&format!("/api/v1/annotation-layers/{layer_id}"))
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

/// Trois items : device (equirect sur pano), pin (flat sur photo), note
/// (equirect sur pano) — cibles `pac-01`.
fn doc_avec_items(pano_id: &str, photo_id: &str, device: &str) -> serde_json::Value {
    serde_json::json!({
        "items": [
            {"id": "a1", "media_asset_id": pano_id, "kind": "device",
             "geometry": {"type": "equirect", "yaw": 45.0, "pitch": -10.0},
             "color": "#2563eb", "label": "PAC-01 circulateur",
             "target": {"type": "device", "device_id": device}},
            {"id": "a2", "media_asset_id": photo_id, "kind": "pin",
             "geometry": {"type": "flat", "x": 0.32, "y": 0.48},
             "label": "Circulateur gpio 4",
             "target": {"type": "pin", "device_id": device, "pin_gpio": 4}},
            {"id": "a3", "media_asset_id": pano_id, "kind": "note",
             "geometry": {"type": "equirect", "yaw": 0.0, "pitch": 0.0},
             "label": "Vanne CAU fermee l'ete",
             "target": {"type": "note", "text": "Vanne CAU fermee l'ete"}}
        ]
    })
}

async fn publish_layer(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    layer_id: &str,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/annotation-layers/{layer_id}/publish"))
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({}))
        .await
}

async fn read_annotations(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    asset_id: &str,
) -> axum_test::TestResponse {
    server
        .get(&format!("/api/v1/media/{asset_id}/annotations"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
}

/// Creates a device (seeded `generic_esp8266`) — returns the PK.
async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    device_id: &str,
) -> i64 {
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "generic_esp8266",
        }))
        .await;
    assert_eq!(res.status_code(), 201, "device créé : {}", res.text());
    res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("device pk")
}

#[tokio::test]
#[serial]
async fn cycle_creation_save_versions_409() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let photo_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=photo.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let device = create_device(&server, &env.alice, org, "pac-01").await;
        let created = create_layer(&server, &env.alice, org, "Salle machine").await;
        assert_eq!(created["doc_version_number"], 1, "creation -> v1");
        assert_eq!(created["latest_version_number"], 1);
        assert_eq!(
            created["doc"]["items"].as_array().unwrap().len(),
            0,
            "doc vide"
        );
        assert!(created["published_version_number"].is_null());
        let layer_id = created["id"].as_str().unwrap().to_string();

        // Liste + search + envelope D14.
        let listed: serde_json::Value = server
            .get("/api/v1/annotation-layers?search=salle")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1);
        let listed: serde_json::Value = server
            .get("/api/v1/annotation-layers?search=introuvable")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // Save v2 avec items.
        let doc = doc_avec_items(&pano_id, &photo_id, "pac-01");
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc).await;
        assert_eq!(res.status_code(), 200, "save v2 : {}", res.text());
        let saved: serde_json::Value = res.json();
        assert_eq!(saved["doc_version_number"], 2);
        assert_eq!(saved["doc"]["items"].as_array().unwrap().len(), 3);

        // Save perime (expected 1) -> 409.
        let res = save_layer(
            &server,
            &env.alice,
            org,
            &layer_id,
            1,
            doc_avec_items(&pano_id, &photo_id, "pac-01"),
        )
        .await;
        assert_eq!(res.status_code(), 409, "save perime -> 409");
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "annot-version-conflict");

        // Historique : 2 versions, la plus recente d'abord ; v1 vide.
        let versions: serde_json::Value = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(versions["count"], 2);
        assert_eq!(versions["results"][0]["version_number"], 2);
        assert_eq!(versions["results"][0]["author"], "alice");
        let v1: serde_json::Value = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            v1["doc"]["items"].as_array().unwrap().len(),
            0,
            "v1 immuable"
        );
        let old: serde_json::Value = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}?version=1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(old["doc_version_number"], 1);
        assert_eq!(old["latest_version_number"], 2);

        // Version inconnue -> 404.
        let res = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}/versions/9"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);

        // Publish (defaut = latest v2) puis unpublish.
        let res = publish_layer(&server, &env.alice, org, &layer_id).await;
        assert_eq!(res.status_code(), 200, "publish : {}", res.text());
        let published: serde_json::Value = res.json();
        assert_eq!(published["published_version_number"], 2);
        let res = server
            .post(&format!("/api/v1/annotation-layers/{layer_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let unpublished: serde_json::Value = res.json();
        assert!(unpublished["published_version_number"].is_null());

        // Delete -> 204 puis 404.
        let _ = device;
        let res = server
            .delete(&format!("/api/v1/annotation-layers/{layer_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        let res = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn validation_doc_assets_et_devices() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let photo_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=photo.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let created = create_layer(&server, &env.alice, org, "Validation").await;
        let layer_id = created["id"].as_str().unwrap().to_string();

        // Violation structurelle : id duplique.
        let mut doc = doc_avec_items(&pano_id, &photo_id, "pac-01");
        doc["items"][1]["id"] = "a1".into();
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc).await;
        assert_eq!(res.status_code(), 400, "id duplique -> 400");
        let codes: Vec<String> = res.json::<serde_json::Value>()["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap().to_string())
            .collect();
        assert!(
            codes.contains(&"duplicate_item_id".to_string()),
            "{codes:?}"
        );

        // Asset inconnu (UUID bien forme mais hors org) -> 400.
        let res = save_layer(
            &server,
            &env.alice,
            org,
            &layer_id,
            1,
            doc_avec_items("00000000-0000-0000-0000-0000000000bb", &photo_id, "pac-01"),
        )
        .await;
        assert_eq!(res.status_code(), 400, "asset inconnu -> 400");
        assert!(
            res.json::<serde_json::Value>()["doc"]
                .as_str()
                .unwrap()
                .contains("inconnu"),
            "per-field doc message"
        );

        // Geometrie equirect sur un asset photo -> 400 kind panorama requis.
        let doc = doc_avec_items(&photo_id, &photo_id, "pac-01");
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc.clone()).await;
        assert_eq!(res.status_code(), 400, "equirect sur photo -> 400");
        assert!(
            res.json::<serde_json::Value>()["doc"]
                .as_str()
                .unwrap()
                .contains("panorama"),
            "message kind panorama"
        );

        // Device inconnu -> 400.
        let res = save_layer(
            &server,
            &env.alice,
            org,
            &layer_id,
            1,
            doc_avec_items(&pano_id, &photo_id, "ghost"),
        )
        .await;
        assert_eq!(res.status_code(), 400, "device inconnu -> 400");
        assert!(
            res.json::<serde_json::Value>()["doc"]
                .as_str()
                .unwrap()
                .contains("ghost"),
            "message device inconnu"
        );

        // id non-UUID -> inconnu (pas de panic).
        let res = save_layer(
            &server,
            &env.alice,
            org,
            &layer_id,
            1,
            doc_avec_items("pas-un-uuid", &photo_id, "pac-01"),
        )
        .await;
        assert_eq!(res.status_code(), 400);

        // Tout est valide -> 200 v2 (device pac-01 cree, items coherents).
        let _ = create_device(&server, &env.alice, org, "pac-01").await;
        let doc = doc_avec_items(&pano_id, &photo_id, "pac-01");
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc).await;
        assert_eq!(res.status_code(), 200, "save valide : {}", res.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn read_model_union_et_dead_device() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let photo_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=photo.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let device_pk = create_device(&server, &env.alice, org, "pac-01").await;

        // Couche A : publiee (3 items). Couche B : brouillon (1 item sur le
        // pano) — doit rester absente du read model.
        let a = create_layer(&server, &env.alice, org, "Circulation").await;
        let a_id = a["id"].as_str().unwrap().to_string();
        let res = save_layer(
            &server,
            &env.alice,
            org,
            &a_id,
            1,
            doc_avec_items(&pano_id, &photo_id, "pac-01"),
        )
        .await;
        assert_eq!(res.status_code(), 200, "save A : {}", res.text());
        let res = publish_layer(&server, &env.alice, org, &a_id).await;
        assert_eq!(res.status_code(), 200, "publish A : {}", res.text());

        let b = create_layer(&server, &env.alice, org, "Brouillon").await;
        let b_id = b["id"].as_str().unwrap().to_string();
        let mut doc_b = doc_avec_items(&pano_id, &photo_id, "pac-01");
        doc_b["items"] = serde_json::json!([
            {"id": "b1", "media_asset_id": pano_id, "kind": "note",
             "geometry": {"type": "equirect", "yaw": 10.0, "pitch": 0.0},
             "label": "brouillon",
             "target": {"type": "note", "text": "brouillon"}}
        ]);
        let res = save_layer(&server, &env.alice, org, &b_id, 1, doc_b).await;
        assert_eq!(res.status_code(), 200, "save B : {}", res.text());

        // Union : seuls les items de A (a1 + a3 sur le pano) ; B exclu.
        let body = read_annotations(&server, &env.alice, org, &pano_id)
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["media_asset_id"], pano_id.as_str());
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "a1 + a3 (B brouillon exclu) : {body}");
        assert_eq!(items[0]["id"], "a1");
        assert_eq!(items[0]["layer_name"], "Circulation");
        assert_eq!(items[0]["resolved"]["device_label"], "pac-01");
        assert!(!items[0]["resolved"]["dead"].as_bool().unwrap());
        assert!(
            items[0]["resolved"]["device_pk"].as_i64().is_some(),
            "pk device vivant"
        );
        assert_eq!(items[1]["id"], "a3");
        assert!(items[1]["resolved"].is_null(), "note : pas de resolution");

        // Photo : a2 (pin gpio 4) résolu sur le device pac-01.
        let body = read_annotations(&server, &env.alice, org, &photo_id)
            .await
            .json::<serde_json::Value>();
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["target"]["pin_gpio"], 4);
        assert_eq!(items[0]["resolved"]["device_label"], "pac-01");

        // Asset inconnu -> 404 masque.
        let res = read_annotations(
            &server,
            &env.alice,
            org,
            "00000000-0000-0000-0000-0000000000ff",
        )
        .await;
        assert_eq!(res.status_code(), 404);

        // Reference morte toleree : device supprime sous nos pieds (D57,
        // cible faible — dead: true, jamais 500).
        let res = server
            .delete(&format!("/api/v1/devices/{device_pk}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204, "device supprime : {}", res.text());
        let body = read_annotations(&server, &env.alice, org, &pano_id)
            .await
            .json::<serde_json::Value>();
        let items = body["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "les items survivent au device");
        assert_eq!(items[0]["resolved"]["dead"], true, "device disparu -> dead");
        assert!(
            items[0]["resolved"]["device_pk"].is_null(),
            "pk morte = null"
        );
        assert_eq!(
            items[0]["resolved"]["device_label"], "pac-01",
            "slug conserve"
        );

        // Depublication : les items quittent tous les viewers.
        let res = server
            .post(&format!("/api/v1/annotation-layers/{a_id}/unpublish"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let body = read_annotations(&server, &env.alice, org, &pano_id)
            .await
            .json::<serde_json::Value>();
        assert_eq!(
            body["items"].as_array().unwrap().len(),
            0,
            "depublie = items absents"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_org_et_roles() {
    with_app(|server, env| async move {
        let org1 = personal_org(&server, &env.alice).await;
        let org2 = personal_org(&server, &env.bob).await;

        // Cross-org : GET/PATCH/publish → 404 masqué (jamais 403).
        let created = create_layer(&server, &env.alice, org1, "Prive alice").await;
        let layer_id = created["id"].as_str().unwrap().to_string();
        let res = server
            .get(&format!("/api/v1/annotation-layers/{layer_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "GET cross-org masque");
        let res = server
            .patch(&format!("/api/v1/annotation-layers/{layer_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "doc": {"items": []},
                "author": "bob"
            }))
            .await;
        assert_eq!(res.status_code(), 404, "PATCH cross-org masque");
        let res = server
            .post(&format!("/api/v1/annotation-layers/{layer_id}/publish"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 404, "publish cross-org masque");

        // Sans X-Org-Id -> 400.
        let res = server
            .get("/api/v1/annotation-layers")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);

        // Org partagée : alice owner, bob viewer (école tenant_isolation).
        let created_org: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Annot" }))
            .await
            .json();
        let shared_org = created_org["id"].as_i64().unwrap();
        server
            .post(&format!("/api/v1/orgs/{shared_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;

        // Lecture OK pour un viewer, écriture 403 (read model inclus).
        let pano_id = upload(
            &server,
            &env.alice,
            shared_org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let created = create_layer(&server, &env.alice, shared_org, "Partage").await;
        let shared_layer = created["id"].as_str().unwrap().to_string();
        let res = publish_layer(&server, &env.alice, shared_org, &shared_layer).await;
        assert_eq!(res.status_code(), 200, "publish v1 vide : {}", res.text());

        let res = server
            .get(&format!("/api/v1/annotation-layers/{shared_layer}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 200, "viewer lit");
        let res = read_annotations(&server, &env.bob, shared_org, &pano_id).await;
        assert_eq!(
            res.status_code(),
            200,
            "viewer lit le read model (0 items, doc v1 vide)"
        );
        assert_eq!(
            res.json::<serde_json::Value>()["items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        for path in [
            format!("/api/v1/annotation-layers/{shared_layer}/unpublish"),
            format!("/api/v1/annotation-layers/{shared_layer}/publish"),
        ] {
            let res = server
                .post(&path)
                .add_header("Content-Type", "application/json")
                .add_header("Authorization", bearer(&env.bob))
                .add_header("X-Org-Id", shared_org.to_string())
                .json(&serde_json::json!({}))
                .await;
            assert_eq!(res.status_code(), 403, "viewer -> 403 : {path}");
        }
        let res = server
            .delete(&format!("/api/v1/annotation-layers/{shared_layer}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer delete -> 403");

        // Tour inconnu → 404 (owner lui-même).
        let res = server
            .get("/api/v1/annotation-layers/00000000-0000-0000-0000-0000000000ff")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ensemble_annote_media_associe() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let photo_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Création avec média déclaré + lecture du lien.
        let res = server
            .post("/api/v1/annotation-layers")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Salle machines - RDC",
                "media_asset_id": pano_id,
            }))
            .await;
        assert_eq!(res.status_code(), 201);
        let layer = res.json::<serde_json::Value>();
        assert_eq!(layer["media_asset_id"], serde_json::json!(pano_id));
        let layer_id = layer["id"].as_str().unwrap().to_string();

        // Save d'un item sur le média déclaré : OK (device ciblé créé avant).
        let _device_pk = create_device(&server, &env.alice, org, "pac-01").await;
        let doc = serde_json::json!({ "items": [{
            "id": "a1", "media_asset_id": pano_id,
            "kind": "device",
            "geometry": {"type": "equirect", "yaw": 10.0, "pitch": 0.0},
            "label": "PAC-01",
            "target": {"type": "device", "device_id": "pac-01"},
        }]});
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc).await;
        assert_eq!(res.status_code(), 200, "save ancré sur le média déclaré");

        // Item ancré sur un AUTRE média : refusé (un ensemble = un média).
        let bad = serde_json::json!({ "items": [{
            "id": "a1", "media_asset_id": photo_id,
            "kind": "note",
            "geometry": {"type": "flat", "x": 0.5, "y": 0.5},
            "label": "hors média",
            "target": {"type": "note", "text": "x"},
        }]});
        let res = save_layer(&server, &env.alice, org, &layer_id, 2, bad).await;
        assert_eq!(res.status_code(), 400, "item hors média déclaré -> 400");

        // Filtre list?media= : l'ensemble apparaît pour son média.
        let res = server
            .get(&format!("/api/v1/annotation-layers?media={pano_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let page = res.json::<serde_json::Value>();
        let medias: Vec<&str> = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| l["media_asset_id"].as_str())
            .collect();
        assert!(
            medias.contains(&pano_id.as_str()),
            "le filtre media= ramène l'ensemble : {page}"
        );

        // Un média sans ensemble : liste vide.
        let res = server
            .get(&format!("/api/v1/annotation-layers?media={photo_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let page = res.json::<serde_json::Value>();
        assert_eq!(
            page["results"].as_array().unwrap().len(),
            0,
            "aucun ensemble sur ce média"
        );
    })
    .await;
}

/// Ensemble rattaché à un TOUR : création avec tour_id, save
/// d'un item ancré sur un média de scène, refus hors tour, filtre
/// ?tour=.
#[tokio::test]
#[serial]
async fn ensemble_annote_tour_associe() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let photo_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Tour avec une scène sur le pano (v2 : doc avec scène).
        let tour_id = server
            .post("/api/v1/tours")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "name": "Tour E2E annot" }))
            .await
            .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let doc = serde_json::json!({
            "mode": "panorama",
            "floors": [{"id": "f1", "name": "RDC", "level": 0}],
            "scenes": [
                {"id": "s1", "floor_id": "f1", "media_asset_id": pano_id},
            ],
        });
        let res = server
            .patch(&format!("/api/v1/tours/{tour_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "doc": doc,
            }))
            .await;
        assert_eq!(
            res.status_code(),
            200,
            "version tour avec scène : {}",
            res.text()
        );

        // Création d'un ensemble rattaché AU TOUR.
        let res = server
            .post("/api/v1/annotation-layers")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Annotations du tour",
                "tour_id": tour_id,
            }))
            .await;
        assert_eq!(res.status_code(), 201, "create tour-set : {}", res.text());
        let layer = res.json::<serde_json::Value>();
        assert_eq!(layer["tour_id"], serde_json::json!(tour_id));
        assert!(layer["media_asset_id"].is_null());
        let layer_id = layer["id"].as_str().unwrap().to_string();

        // Save d'un item ancré sur le média de la scène : OK.
        let doc = serde_json::json!({ "items": [{
            "id": "a1", "media_asset_id": pano_id,
            "kind": "note",
            "geometry": {"type": "equirect", "yaw": 10.0, "pitch": 0.0},
            "label": "scène 1",
            "target": {"type": "note", "text": "scène 1"},
        }]});
        let res = save_layer(&server, &env.alice, org, &layer_id, 1, doc).await;
        assert_eq!(
            res.status_code(),
            200,
            "save sur média de scène : {}",
            res.text()
        );

        // Item ancré sur un média HORS tour : refusé (AnchorMismatch).
        let bad = serde_json::json!({ "items": [{
            "id": "a1", "media_asset_id": photo_id,
            "kind": "note",
            "geometry": {"type": "flat", "x": 0.5, "y": 0.5},
            "label": "hors tour",
            "target": {"type": "note", "text": "x"},
        }]});
        let res = save_layer(&server, &env.alice, org, &layer_id, 2, bad).await;
        assert_eq!(
            res.status_code(),
            400,
            "item hors tour -> 400 : {}",
            res.text()
        );

        // XOR : média ET tour -> 400.
        let res = server
            .post("/api/v1/annotation-layers")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "X",
                "media_asset_id": pano_id,
                "tour_id": tour_id,
            }))
            .await;
        assert_eq!(
            res.status_code(),
            400,
            "média ET tour -> 400 : {}",
            res.text()
        );

        // Filtre list?tour= : l'ensemble apparaît pour son tour.
        let res = server
            .get(&format!("/api/v1/annotation-layers?tour={tour_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let page = res.json::<serde_json::Value>();
        let tours_seen: Vec<&str> = page["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|l| l["tour_id"].as_str())
            .collect();
        assert!(
            tours_seen.contains(&tour_id.as_str()),
            "le filtre tour= ramène l'ensemble : {page}"
        );
    })
    .await;
}

/// D128/D129: a `control` item references an org control (an unknown one is
/// refused at save), a `reading` item carries a dashboard source and may
/// target a flow virtual device (never registered).
#[tokio::test]
#[serial]
async fn control_and_reading_items() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let pano_id = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await
        .json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let layer = create_layer(&server, &env.alice, org, "Surfaces").await;
        let layer_id = layer["id"].as_str().unwrap().to_string();
        let control = server
            .post("/api/v1/controls")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "key": "light.room", "label": "Room light", "spec": { "kind": "switch" }
            }))
            .await
            .json::<serde_json::Value>();
        let control_id = control["id"].as_str().unwrap().to_string();
        let doc = |control: &str| {
            serde_json::json!({ "items": [
                {"id": "c1", "media_asset_id": pano_id, "kind": "control",
                 "geometry": {"type": "equirect", "yaw": 10.0, "pitch": 0.0},
                 "label": "Light", "target": {"type": "control", "control_id": control}},
                {"id": "r1", "media_asset_id": pano_id, "kind": "reading",
                 "geometry": {"type": "equirect", "yaw": 20.0, "pitch": 0.0},
                 "label": "Temp", "target": {"type": "reading", "spark": true, "source": {
                     "role": "primary", "metric": "temperature",
                     "device_id": "flow_12", "window": "1h"}}}
            ]})
        };

        let unknown = save_layer(
            &server,
            &env.alice,
            org,
            &layer_id,
            1,
            doc("00000000-0000-0000-0000-0000000000aa"),
        )
        .await;
        assert_eq!(unknown.status_code(), 400, "unknown control -> 400");

        let saved = save_layer(&server, &env.alice, org, &layer_id, 1, doc(&control_id)).await;
        assert_eq!(saved.status_code(), 200, "{}", saved.text());
        assert_eq!(
            publish_layer(&server, &env.alice, org, &layer_id)
                .await
                .status_code(),
            200
        );
        let read = read_annotations(&server, &env.alice, org, &pano_id)
            .await
            .json::<serde_json::Value>();
        let kinds: Vec<&str> = read["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds.len(), 2, "{read}");
        assert!(kinds.contains(&"control") && kinds.contains(&"reading"));
        let reading = read["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["kind"] == "reading")
            .unwrap();
        assert_eq!(reading["target"]["source"]["device_id"], "flow_12");
        assert!(reading.get("resolved").is_none() || reading["resolved"].is_null());
    })
    .await;
}
