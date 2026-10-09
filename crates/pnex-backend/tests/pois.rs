//! Tests carte POI-first (D35–D39, D43) : CRUD POI, placements devices
//! multiples + déplacement + détachement (D43 et amendement), clustering
//! backend (D37), liens (D39 → D42 `resource_edges`), positions GPS (D38 —
//! observe + manuel), isolation org (404 masqué), rôles (viewer lecture
//! seule), enveloppe D14.
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

/// Device du catalogue (`temp_sensor`, école builds.rs) attaché à l'org.
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

/// D43 : attache un device existant à un POI (POST /pois/{id}/devices).
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
async fn cycle_complet_poi() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // 201 + emoji par défaut 📍 + location_detail.
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
        assert_eq!(poi["emoji"], "📍", "emoji par défaut");
        assert_eq!(poi["mode"], "geo");
        assert_eq!(
            poi["location_detail"], "Bât. A — Étage 2 — Salle 204",
            "localisation libre bâtiment"
        );

        // Liste : enveloppe D14 + search insensible à la casse.
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
        assert_eq!(listed["count"], 1, "search insensible à la casse");
        let listed: serde_json::Value = server
            .get("/api/v1/pois?search=absent")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // PATCH : déplacement + emoji custom + location_detail → null.
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

        // PATCH reset emoji ("" → défaut) + 404 cross-org plus bas.
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
            "reset défaut"
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
        assert_eq!(res.status_code(), 404, "supprimé = 404 masqué");
    })
    .await;
}

// ──────────────── aperçu épinglé ────────────────

#[tokio::test]
#[serial]
async fn pin_apercu_poi() {
    with_app(|server, _db, env| async move {
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

        // Pose du pin (couple kind+id) → 200 + écho des champs.
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
        assert_eq!(res.status_code(), 200, "pose pin : {}", res.text());
        let dto = res.json::<serde_json::Value>();
        assert_eq!(dto["preview_kind"], "media_asset");
        assert_eq!(dto["preview_id"], "11111111-2222-3333-4444-555555555555");

        // GET detail → les champs survivent au round-trip.
        let dto: serde_json::Value = headers(server.get(&format!("/api/v1/pois/{poi_id}")))
            .await
            .json();
        assert_eq!(dto["preview_kind"], "media_asset");

        // Kind hors liste → 400 preview_kind.
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
        assert_eq!(res.status_code(), 400, "kind hors liste : {}", res.text());
        assert_eq!(res.json::<serde_json::Value>()["preview_kind"], "Aperçu épinglé invalide (kind ∈ media_asset | dashboard | tour, posé avec preview_id).");

        // Pose sans id → 400 (le couple vit ensemble).
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "preview_kind": "tour" }))
            .await;
        assert_eq!(res.status_code(), 400, "kind sans id : {}", res.text());

        // Désépinglage : null sur l'un des deux champs → les deux cleared.
        let res = server
            .patch(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "preview_id": null }))
            .await;
        assert_eq!(res.status_code(), 200, "unpin : {}", res.text());
        let dto = res.json::<serde_json::Value>();
        assert!(dto["preview_kind"].is_null(), "kind cleared avec l'id");
        assert!(dto["preview_id"].is_null());
    })
    .await;
}

// ──────────────── D43 : placements multiples + déplacement ────────────────

#[tokio::test]
#[serial]
async fn placements_multiples_et_deplacement() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "soil-01").await;
        create_device(&server, &env.alice, org, "soil-02").await;

        // POI A ; le champ device_id hérité de D36 est ignoré à la création
        // (l'attache se fait délibérément via POST /pois/{id}/devices).
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
            "POI neuf sans device"
        );
        let res = create_poi(&server, &env.alice, org, "POI B", 45.8, 4.9).await;
        assert_eq!(res.status_code(), 201, "B : {}", res.text());
        let poi_b = res.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id B")
            .to_string();

        // Deux devices sur le même POI (D43 : plusieurs possibles).
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
            "second device sur le même POI : {}",
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

        // Device inconnu → 400 champ device_id.
        let res = attach_device(&server, &env.alice, org, &poi_a, "inexistant-01").await;
        assert_eq!(res.status_code(), 400, "device inconnu : {}", res.text());
        assert!(res.text().contains("device_id"));

        // Déjà placé — même POI ou autre — → 409 porteur du placement courant
        // (le front propose le déplacement, jamais silencieux).
        let res = attach_device(&server, &env.alice, org, &poi_a, "soil-01").await;
        assert_eq!(
            res.status_code(),
            409,
            "re-attach même POI : {}",
            res.text()
        );
        let conflict = res.json::<serde_json::Value>();
        assert_eq!(conflict["device_id"], "soil-01");
        assert_eq!(conflict["current_pin_id"], poi_a, "même POI");
        assert_eq!(conflict["current_pin_label"], "POI A");
        assert!(
            conflict["current_placement_id"].is_i64(),
            "id de placement fourni"
        );
        let res = attach_device(&server, &env.alice, org, &poi_b, "soil-01").await;
        assert_eq!(
            res.status_code(),
            409,
            "device déjà placé ailleurs : {}",
            res.text()
        );

        // location_detail par placement : set puis clear (null double-Option).
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

        // Déplacement de soil-01 vers B (un retrait parmi deux désormais —
        // le détachement existe aussi, amendement D43).
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
            "A a perdu soil-01 (move, pas copie)"
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
            "B a gagné soil-01"
        );

        // POI cible inconnu → 400 champ pin_id ; placement inconnu → 404.
        let res = server
            .patch(&format!("/api/v1/pois/placements/{placement_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "pin_id": "00000000-0000-0000-0000-0000000000ff"
            }))
            .await;
        assert_eq!(res.status_code(), 400, "cible inconnue : {}", res.text());
        assert!(res.text().contains("pin_id"));
        let res = server
            .patch("/api/v1/pois/placements/999999")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "location_detail": "nulle part" }))
            .await;
        assert_eq!(res.status_code(), 404, "placement inconnu masqué");

        // Détachement (amendement D43, 2026-09-13) : soil-01 redevient
        // libre, puis se rattache où l'on veut — ici de retour sur B.
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
            "A n'a plus que soil-02 (soil-01 détaché)"
        );
        let res = attach_device(&server, &env.alice, org, &poi_b, "soil-01").await;
        assert_eq!(
            res.status_code(),
            201,
            "re-attach après detach : {}",
            res.text()
        );
        let res = server
            .delete("/api/v1/pois/placements/999999")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "placement inconnu masqué");

        // Suppression du POI → cascade des placements : soil-01 redevient
        // plaçable (il « doit être replacé », avertissement côté UI).
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
            "cascade POI delete : soil-01 replaçable : {}",
            res.text()
        );
        let placement_id = res.json::<serde_json::Value>()["id"].as_i64().expect("id");

        // Hardening repris du test D36 : location_detail > 255, latitude
        // hors WGS84, emoji trop long.
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
        assert_eq!(res.status_code(), 400, "latitude hors bornes");
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
async fn cluster_pois_et_filtres() {
    with_app(|server, _db, env| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "soil-01").await;
        create_device(&server, &env.alice, org, "soil-02").await;

        // 3 POI à Lyon (dont 1 avec 2 devices attachés — D43), 1 à Paris.
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
            "2 devices sur Lyon 2 : {}",
            res.text()
        );
        let _ = create_poi(&server, &env.alice, org, "Paris", 48.8566, 2.3522).await;

        // Grille de 205 POI serrés à Lyon (dépasse CLUSTER_INDIVIDUAL_MAX,
        // un cluster ne peut apparaître qu'au-dessus du seuil).
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

        // Zoom 10 : cellule ~0.044° ≫ écart intra-grille → agrégation
        // (Paris est hors de cette bbox).
        let res = get_cluster(&server, &env.alice, org, "").await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 207, "2 POI + 205 de la grille");
        let items = body["items"].as_array().expect("items");
        assert!(
            items.len() < body["total"].as_u64().unwrap() as usize,
            "aggregé à zoom bas"
        );
        assert!(
            items.iter().any(|i| i["count"].as_u64() == Some(207)),
            "une seule cellule occupée à zoom 10"
        );
        assert!(
            items.iter().all(|i| i["id"].is_null()),
            "cluster ≠ point individuel"
        );
        let summed: u64 = items.iter().filter_map(|i| i["count"].as_u64()).sum();
        assert_eq!(summed, 207, "aucun point perdu");

        // Bbox élargie à Paris : cluster Lyon (207) + singleton Paris (1).
        let res = server
            .get("/api/v1/pois/cluster?bbox=2.0,44.0,6.0,50.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 208);
        let items = body["items"].as_array().expect("items");
        assert_eq!(items.len(), 2, "1 cluster + 1 singleton");
        assert_eq!(items[0]["count"], 207, "trié par densité desc");
        assert_eq!(items[1]["count"], 1);
        assert!(items[1]["id"].is_string(), "singleton porte son id");

        // Zoom 22 : cellule minuscule → points individuels avec ids.
        let res = server
            .get("/api/v1/pois/cluster?bbox=4.83,45.75,4.85,45.78&zoom=22")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["total"], 207, "bbox Lyon serrée");
        let items = body["items"].as_array().expect("items");
        assert!(items.iter().all(|i| i["count"] == 1), "individuels");
        assert!(items.iter().all(|i| i["id"].is_string()), "id présent");

        // Filtre has_device : seul Lyon 2 (2 placements = compté UNE fois).
        let body = get_cluster(&server, &env.alice, org, "has_device=true")
            .await
            .json::<serde_json::Value>();
        assert_eq!(
            body["total"], 1,
            "filtre has_device, pas de doublon de jointure"
        );

        // Filtre search : par label…
        let body = get_cluster(&server, &env.alice, org, "search=lyon")
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["total"], 2, "filtre search");
        // …et par slug d'un device placé (D43 : haystack via placements).
        let body = get_cluster(&server, &env.alice, org, "search=soil-01")
            .await
            .json::<serde_json::Value>();
        assert_eq!(body["total"], 1, "search par slug placé");

        // invalid bbox -> 400 per-field error.
        let res = server
            .get("/api/v1/pois/cluster?bbox=6.0,44.0,4.0,50.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400, "west>east rejeté");
        let res = server
            .get("/api/v1/pois/cluster?bbox=4.0,44.0&zoom=10")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400, "bbox incomplète");
    })
    .await;
}

// ──────────────────── D39 → D42 : liens (arêtes `placed_on`) ────────────────────

#[tokio::test]
#[serial]
async fn positions_observees() {
    with_app(|server, db, env| async move {
        use pnex_backend::services::{pois as svc, telemetry::TelemetryPoint};
        let org = personal_org(&server, &env.alice).await;
        let device = create_device(&server, &env.alice, org, "tracker-01").await;
        let registry_id = device["id"].as_i64().expect("registry id");

        // Couche vide au départ.
        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // Chemin d'ingestion : trames `latitude` puis `longitude` séparées
        // (convention D38) via le service observé par le tap du sink.
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
        // Pas encore de paire → rien en base.
        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0, "demi-position invisible");
        svc::observe(&db, &point("longitude", "4.835659"))
            .await
            .expect("observe lon");

        let listed: serde_json::Value = server
            .get("/api/v1/device-positions")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "paire complète = position");
        let pos = &listed["results"].as_array().expect("results")[0];
        assert_eq!(pos["device_id"], "tracker-01");
        assert_eq!(pos["source"], "telemetry");
        let lat = pos["latitude"].as_f64().expect("lat");
        assert!((lat - 45.764043).abs() < 1e-5, "lat : {lat}");

        // Filtre has_position : le POI portant le tracker devient visible
        // (attache D43 : create puis POST devices).
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
        assert_eq!(listed["count"], 1, "filtre has_position");
    })
    .await;
}

// ─────────────────────────── isolation + rôles ───────────────────────────

#[tokio::test]
#[serial]
async fn isolation_org_et_roles_viewer() {
    with_app(|server, _db, env| async move {
        // Boot JIT des deux users.
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }
        // Org partagée : alice owner, bob viewer (école tenant_isolation).
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Co" }))
            .await
            .json();
        let shared_org = created["id"].as_i64().expect("id org partagée");
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

        // Bob sur SON org : le POI d'alice est 404 masqué (lecture/écriture).
        let bob_org = personal_org(&server, &env.bob).await;
        let res = server
            .get(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404, "cross-org masqué");
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
        // Liste de bob vide (pas de fuite).
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0);

        // Dans l'org partagée : bob viewer lit, mais n'écrit pas.
        let _ = create_poi(&server, &env.alice, shared_org, "POI Commun", 45.7, 4.8).await;
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1, "viewer lit la liste");
        let res = server
            .post("/api/v1/pois")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .json(&serde_json::json!({"label": "Interdit", "latitude": 45.7, "longitude": 4.8}))
            .await;
        assert_eq!(res.status_code(), 403, "viewer n'écrit pas");
        // Viewer ne peut pas attacher un device non plus (D43).
        create_device(&server, &env.alice, shared_org, "soil-shared").await;
        let listed: serde_json::Value = server
            .get("/api/v1/pois")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", shared_org.to_string())
            .await
            .json();
        let shared_poi = listed["results"].as_array().expect("results")[0]["id"]
            .as_str()
            .expect("id POI partagé")
            .to_string();
        let res = attach_device(&server, &env.bob, shared_org, &shared_poi, "soil-shared").await;
        assert_eq!(res.status_code(), 403, "viewer n'attache pas");
        // Viewer ne peut pas détacher non plus (amendement D43).
        let res = attach_device(&server, &env.alice, shared_org, &shared_poi, "soil-shared").await;
        assert_eq!(res.status_code(), 201, "alice attache : {}", res.text());
        let detach_id = res.json::<serde_json::Value>()["id"].as_i64().expect("id");
        let res = server
            .delete(&format!("/api/v1/pois/placements/{detach_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer ne détache pas");
        let res = server
            .delete(&format!("/api/v1/pois/{poi_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", shared_org.to_string())
            .await;
        assert_eq!(
            res.status_code(),
            404,
            "POI d'alice invisible depuis l'org partagée"
        );
    })
    .await;
}
