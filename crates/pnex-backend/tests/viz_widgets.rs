//! Tests de la bibliothèque de widgets (D41) : CRUD mutable, unicité
//! (org_id, name), validation de config (400 violations), isolation org
//! (404 masqué), rôles (viewer lecture seule).
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

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org personnelle")
}

/// Template jauge valide.
fn gauge_config() -> serde_json::Value {
    serde_json::json!({
        "type": "gauge",
        "title": "Température atelier",
        "source": [ {
            "role": "primary", "metric": "soil_moisture",
            "device_id": "soil-01", "window": "1h"
        } ],
        "options": { "unit": "°C", "min": 0.0, "max": 50.0, "decimals": 1 }
    })
}

#[tokio::test]
#[serial]
async fn cycle_crud_unicite_et_validation() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // ── Création.
        let created = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Jauge °C",
                "kind": "gauge",
                "config": gauge_config(),
            }))
            .await;
        assert_eq!(created.status_code(), 201);
        let created = created.json::<serde_json::Value>();
        assert_eq!(created["kind"], "gauge");
        assert_eq!(created["config"]["type"], "gauge");
        let id = created["id"].as_str().unwrap().to_string();

        // ── Unicité (org_id, name) → 400 lisible.
        let dup = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Jauge °C",
                "kind": "gauge",
                "config": gauge_config(),
            }))
            .await;
        assert_eq!(dup.status_code(), 400);
        assert!(dup.json::<serde_json::Value>()["name"]
            .as_str()
            .unwrap()
            .contains("déjà"));

        // ── kind ≠ config.type → 400 champ.
        let mismatch = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Incohérent",
                "kind": "stat",
                "config": gauge_config(),
            }))
            .await;
        assert_eq!(mismatch.status_code(), 400);
        assert!(mismatch.json::<serde_json::Value>()["kind"].is_string());

        // ── Config invalide (source absente) → 400 violations.
        let mut bad = gauge_config();
        bad["source"] = serde_json::json!([]);
        let invalid = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "name": "Cassé", "kind": "gauge", "config": bad }))
            .await;
        assert_eq!(invalid.status_code(), 400);
        assert!(invalid.json::<serde_json::Value>()["violations"].is_array());

        // ── PATCH : renommage + config (mutable, pas de versioning).
        let mut cfg2 = gauge_config();
        cfg2["options"]["max"] = serde_json::json!(80.0);
        let updated = server
            .patch(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "Jauge four",
                "config": cfg2,
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(updated["name"], "Jauge four");
        assert_eq!(updated["config"]["options"]["max"], 80.0);

        // ── Filtre kind dans la liste D14.
        let list = server
            .get("/api/v1/viz/widgets?kind=gauge")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 1);
        let list = server
            .get("/api/v1/viz/widgets?kind=text")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 0);

        // ── DELETE : 204 puis 404.
        let deleted = server
            .delete(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204);
        let gone = server
            .get(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(gone.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_org_et_viewer() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        let created = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "name": "Jauge privée",
                "kind": "gauge",
                "config": gauge_config(),
            }))
            .await
            .json::<serde_json::Value>();
        let id = created["id"].as_str().unwrap().to_string();

        // ── Bob sur son org : la liste est vide, le détail est 404 masqué.
        let list = server
            .get("/api/v1/viz/widgets")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 0);
        let cross = server
            .get(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(cross.status_code(), 404);
        let cross_delete = server
            .delete(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(cross_delete.status_code(), 404);

        // ── Viewer de l'org : lit la bibliothèque, ne la modifie pas.
        server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let read = server
            .get(&format!("/api/v1/viz/widgets/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(read.status_code(), 200);
        let denied = server
            .post("/api/v1/viz/widgets")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "name": "Vue viewer",
                "kind": "gauge",
                "config": gauge_config(),
            }))
            .await;
        assert_eq!(denied.status_code(), 403);
    })
    .await;
}
