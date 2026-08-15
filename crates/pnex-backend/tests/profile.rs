//! `PATCH /api/v1/profile` — préférences du profil (langue, timezone, format
//! de date, thème) : mise à jour reflétée par `GET /user-info`, rejet des
//! valeurs invalides, refus sans token.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

/// Boot l'app sur la base de test avec le faux Keycloak, exécute le callback.
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_keycloak().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("KEYCLOAK_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let token = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            use pnex_backend::models::_entities::subscription_tiers as tiers;
            if tiers::Entity::find()
                .filter(tiers::Column::Name.eq("Free"))
                .one(&ctx.db)
                .await
                .expect("lookup tier Free")
                .is_none()
            {
                tiers::ActiveModel {
                    name: Set("Free".into()),
                    max_sensor_devices: Set(3),
                    max_actuator_devices: Set(1),
                    max_mixed_devices: Set(0),
                    min_build_interval_secs: Set(300),
                    data_retention_secs: Set(Some(86_400)),
                    ..Default::default()
                }
                .insert(&ctx.db)
                .await
                .expect("insert tier Free");
            }
            f(server, token).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
#[serial]
async fn patch_profil_maj_partielle_puis_user_info_reflete() {
    with_app(|server, token| async move {
        // JIT provisioning (user + profil par défaut + org personnelle).
        server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await;

        // Patch partiel : langue + thème. La timezone par défaut (UTC) reste.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "language": "fr-FR", "theme": "dark" }))
            .await;
        assert_eq!(res.status_code(), 200);
        let patched: serde_json::Value = res.json();
        assert_eq!(patched["language"], "fr", "forme courte normalisée");
        assert_eq!(patched["theme"], "Dark");
        assert_eq!(patched["timezone"], "UTC", "champ non fourni intact");

        // GET /user-info reflète la mise à jour.
        let info: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await
            .json();
        assert_eq!(info["profile"]["language"], "fr");
        assert_eq!(info["profile"]["theme"], "Dark");

        // Deuxième patch : timezone seule, le reste tient.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "timezone": "Europe/Paris" }))
            .await;
        assert_eq!(res.status_code(), 200);
        let patched: serde_json::Value = res.json();
        assert_eq!(patched["timezone"], "Europe/Paris");
        assert_eq!(patched["language"], "fr");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn patch_profil_rejette_valeurs_invalides_et_corps_vides() {
    with_app(|server, token| async move {
        server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await;

        // Langue non supportée.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "language": "klingon" }))
            .await;
        assert_eq!(res.status_code(), 400);

        // Thème inconnu.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "theme": "neon" }))
            .await;
        assert_eq!(res.status_code(), 400);

        // Aucun champ.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 400);

        // Sans token.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "language": "fr" }))
            .await;
        assert_eq!(res.status_code(), 401);
    })
    .await;
}
