//! Tests du summary dashboard (2026-08-19) : sections PG servies quoi
//! qu'il arrive, télémétrie dégradée sans OpenObserve (section absente de
//! test.yaml), X-Org-Id requis, lecture ouverte au viewer.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Le parcours télémétrie disponible (available:true) vit dans
//! tests/openobserve.rs contre le mock O2.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boot l'app + seed du catalogue minimal (même harnais que devices.rs).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_keycloak().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("KEYCLOAK_URL", &base) };
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
            f(server, env, ctx).await;
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

async fn summary(server: &axum_test::TestServer, token: &str, org_id: i64) -> axum_test::TestResponse {
    server
        .get("/api/v1/dashboard/summary")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
}

/// Sans O2 configuré : 200 quand même, sections PG correctes (liveness du
/// device jamais vu, builds vides sans NaN), télémétrie dégradée, et
/// X-Org-Id requis (400).
#[tokio::test]
#[serial]
async fn dashboard_summary_degrade_sans_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        server
            .post("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "device_id": "esp-001",
                "predefined_device_name": "soil_sensor",
            }))
            .await;

        let res = summary(&server, &env.alice, org).await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();

        // Télémétrie dégradée, jamais d'erreur depuis cette branche.
        assert_eq!(body["telemetry"]["available"], false);
        assert_eq!(body["telemetry"]["latest"].as_array().unwrap().len(), 0);

        // Liveness : le device existe mais n'a jamais émis.
        assert_eq!(body["liveness"]["total"], 1);
        assert_eq!(body["liveness"]["live"], 0);
        let device = &body["liveness"]["devices"][0];
        assert_eq!(device["device_id"], "esp-001");
        assert_eq!(device["predefined_device_name"], "soil_sensor");
        assert_eq!(device["live"], false);
        assert!(device["last_seen"].is_null());

        // Builds : zéro, sans NaN.
        assert_eq!(body["builds"]["total"], 0);
        assert_eq!(body["builds"]["succeeded"], 0);
        assert_eq!(body["builds"]["success_rate"], 0.0);

        // Cap liveness : 12 devices de plus (insertion directe, quota
        // contourné) → total 13 mais liste plafonnée à 10, compteurs
        // complets (demande user : « only latest ~10 »).
        {
            use pnex_backend::models::_entities::{device_registries, predefined_devices};
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            let soil = predefined_devices::Entity::find()
                .filter(predefined_devices::Column::Name.eq("soil_sensor"))
                .one(&_ctx.db)
                .await
                .unwrap()
                .unwrap();
            for i in 0..12 {
                device_registries::ActiveModel {
                    device_id: Set(format!("cap-{i:02}")),
                    org_id: Set(org),
                    predefined_device_id: Set(soil.id),
                    ..Default::default()
                }
                .insert(&_ctx.db)
                .await
                .unwrap();
            }
        }
        let body: serde_json::Value = summary(&server, &env.alice, org)
            .await
            .json();
        assert_eq!(body["liveness"]["total"], 13, "compteur complet");
        assert_eq!(body["liveness"]["live"], 0);
        let list = body["liveness"]["devices"].as_array().unwrap();
        assert_eq!(list.len(), 10, "liste plafonnée à ~10");

        // Sans X-Org-Id → 400 (extracteur OrgContext).
        let res = server
            .get("/api/v1/dashboard/summary")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// Lecture seule : un viewer d'une autre org voit son propre summary
/// (cloisonnement D2 — devices de l'org d'alice absents).
#[tokio::test]
#[serial]
async fn dashboard_summary_cloisonne_par_org() {
    with_app(|server, env, _ctx| async move {
        let org_alice = personal_org(&server, &env.alice).await;
        server
            .post("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_alice.to_string())
            .json(&serde_json::json!({
                "device_id": "esp-001",
                "predefined_device_name": "soil_sensor",
            }))
            .await;

        // Bob : org personnelle distincte, sans devices.
        let org_bob = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json::<serde_json::Value>()["orgs"][0]["id"]
            .as_i64()
            .expect("org perso bob");

        let body: serde_json::Value = summary(&server, &env.bob, org_bob)
            .await
            .json();
        assert_eq!(body["liveness"]["total"], 0, "org de bob vide");
        assert_eq!(body["builds"]["total"], 0);
    })
    .await;
}
