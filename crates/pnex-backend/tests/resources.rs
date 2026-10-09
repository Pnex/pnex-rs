//! Tests couche d'organisation transverse (D42) : CRUD labels + validation,
//! labels effectifs (héritage containment, override), containment (parent
//! unique, cycle, re-parent), arêtes (validité par kind, doublon 409, purge
//! symétrique), folders, filtre label effectif sur la liste médias,
//! isolation org + rôles.
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

/// Asset média (UUID) inséré via l'API upload.
async fn create_media(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let res = server
        .post("/api/v1/media?name=Test&filename=loop.bin&kind=photo&content_type=application/octet-stream")
        .add_header("Content-Type", "application/octet-stream")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .bytes("bytes-stub".into())
        .await;
    assert_eq!(res.status_code(), 201, "upload média : {}", res.text());
    let _ = name;
    res.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("id média")
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
        // Validation : nom non normalise -> 400
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

        // Isolation : bob ne voit pas les labels d'alice (404 masque)
        let res = server
            .get(&format!("/api/v1/resources/media_asset/{media}/labels"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masque");
    })
    .await;
}

// ─────────────────────────── labels effectifs ───────────────────────────

#[tokio::test]
#[serial]
async fn labels_effectifs_heritage_et_override() {
    with_app(|server, db, env| async move {
        let _ = &db;
        let org = personal_org(&server, &env.alice).await;
        let media = create_media(&server, &env.alice, org, "pano").await;

        // Folder "Serre" taggé site:serre.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/folders",
            serde_json::json!({"name": "Serre", "emoji": "🌿"}),
        )
        .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let folder_id = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder_id}/labels"),
            serde_json::json!({"labels": {"site": "serre"}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // Média placé dans le folder (containment).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": folder_id.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // Labels effectifs du média : merged = {site: serre}, hérité du folder.
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/labels/effective"),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body: serde_json::Value = res.json();
        assert_eq!(body["merged"]["site"], "serre", "{}", body);
        assert_eq!(body["inherited"][0]["id"], folder_id.to_string());

        // Override : le média écrase site=exterieur (le plus proche gagne).
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
            &format!("/api/v1/resources/media_asset/{media}/labels/effective"),
        )
        .await;
        let body: serde_json::Value = res.json();
        assert_eq!(body["merged"]["site"], "exterieur", "override propre gagne");

        // Le filtre EFFECTIF de la liste médias voit l'héritage…
        let res = jget(&server, &env.alice, org, "/api/v1/media?label=site:serre").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.json::<serde_json::Value>()["count"],
            0,
            "média surchargé localement : plus de site:serre effectif"
        );
        // …et le filtre par tag nu "critique" (héritage) :
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder_id}/labels"),
            serde_json::json!({"labels": {"critique": null}}),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let res = jget(&server, &env.alice, org, "/api/v1/media?label=critique").await;
        assert_eq!(
            res.json::<serde_json::Value>()["count"],
            1,
            "héritage d'un tag nu via l'arbre"
        );
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

        // A dans B.
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{a}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": b.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        // Cycle : B dans A → 400 (A est descendant de B).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{b}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": a.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "cycle refuse");

        // Parent unique : le média re-parenté de A → B (déplacement = 1 PUT).
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
        // A est DANS B : le chemin du média (placé dans B) = [B] seulement.
        assert_eq!(body["path"].as_array().unwrap().len(), 1, "chemin media>B");

        // Détach : parent null → racine (sous-arbre intact, rien supprimé).
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

        // Parent inconnu → 404 masqué.
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

/// Drawer POI : le POI contient des **dossiers** (pas d'objets directs) et
/// les enfants portent leur nom d'affichage tous kinds (hydratation registre).
#[tokio::test]
#[serial]
async fn map_pin_contient_folders_pas_objets() {
    with_app(|server, db, env| async move {
        let _ = db;
        let org = personal_org(&server, &env.alice).await;
        let poi = create_poi(&server, &env.alice, org).await;
        let media = create_media(&server, &env.alice, org, "pano").await;

        // Folder sous le POI → 200.
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

        // Les enfants du POI portent le nom du folder.
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

        // Média directement sous le POI → 400 (map_pin ne contient que des
        // folders).
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/media_asset/{media}/containment"),
            serde_json::json!({"parent": {"kind": "map_pin", "id": poi}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "objets directs refusés");

        // Média dans le folder → 200, et le folder liste son nom d'affichage
        // (hydratation registre, tous kinds).
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

        // Cycle : le POI ne peut pas être rangé dans son propre dossier.
        let res = jput(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/map_pin/{poi}/containment"),
            serde_json::json!({"parent": {"kind": "folder", "id": fid.to_string()}}),
        )
        .await;
        assert_eq!(res.status_code(), 400, "cycle POI>dossier>POI refusé");
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

        // Relation non déclarée (device n'a aucune relation) → 400.
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
            "device n'a pas placed_on dans sa spec"
        );

        // POI inconnu → 404 masqué.
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

        // Arête OK + placement hotspot.
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
        assert_eq!(res.status_code(), 409, "arête en double");

        // Arête cross-org : bob cible le média d'alice → 404.
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
        assert_eq!(res.status_code(), 404, "cible cross-org masquée");

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

        // Purge symétrique : suppression du MÉDIA (cible) purge l'arête.
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
            "purge symétrique : l'arête est partie avec sa cible"
        );
        let _ = edge_id;
    })
    .await;
}

/// POI géo minimal via l'API (helper local).
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

        // Device : étiqueté directement (sans folder) — kind vivant dans la
        // recherche cross-kind.
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
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/search",
            serde_json::json!({"label": "site:serre"}),
        )
        .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body: serde_json::Value = res.json();
        // 3 porteurs légitimes : device (propre), folder (porteur), média
        // (hérité du folder — l'arête d'héritage traverse le containment).
        assert_eq!(body["count"], 3, "device + folder + média : {}", body);

        // Kinds filtrés : device uniquement.
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/search",
            serde_json::json!({"label": "site:serre", "kinds": ["device"]}),
        )
        .await;
        let body: serde_json::Value = res.json();
        assert_eq!(body["count"], 1);
        assert_eq!(body["results"][0]["kind"], "device");

        // Folders : liste + PATCH + delete (le folder labelé meurt, le
        // média reste rooté).
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
        // Le média dans « Serre » est toujours là (isolation folder).
        let res = jget(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/resources/folder/{folder}/containment"),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        // Isolation : bob ne voit pas l'arbre d'alice.
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

/// Device du catalogue (soil_sensor) attaché à l'org — helper local.
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
        serde_json::json!({"device_id": slug, "predefined_device_name": "soil_sensor"}),
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
        // Two devices, one labelled through a folder (inherited label).
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
            &format!("/api/v1/resources/device/{}/containment", devices[0]),
            serde_json::json!({"parent": {"kind": "folder", "id": folder_id.to_string()}}),
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
            "/api/v1/resources/labels/batch",
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
            "/api/v1/resources/labels/batch",
            serde_json::json!({"kind": "media_asset", "ids": [a]}),
        )
        .await
        .json();
        assert_eq!(body["labels"], serde_json::json!({}));
        let res = jpost(
            &server,
            &env.alice,
            org,
            "/api/v1/resources/labels/batch",
            serde_json::json!({"kind": "nope", "ids": []}),
        )
        .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
    })
    .await;
}
