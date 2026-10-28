//! `PATCH /api/v1/profile` — profile preferences (language, timezone, date
//! format, theme): update reflected by `GET /user-info`, invalid values
//! rejected, refused without a token.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

/// Boots the app on the test database with the fake Rauthy, runs the callback.
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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
            use pnex_backend::models::_entities::subscription_tiers as tiers;
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
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
async fn patch_profile_partial_update_then_user_info_reflects() {
    with_app(|server, token| async move {
        // JIT provisioning (user + default profile + personal org).
        server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await;

        // Partial patch: language + theme. The default timezone (UTC) stays.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "language": "fr-FR", "theme": "dark" }))
            .await;
        assert_eq!(res.status_code(), 200);
        let patched: serde_json::Value = res.json();
        assert_eq!(patched["language"], "fr", "short form normalised");
        assert_eq!(patched["theme"], "dark");
        assert_eq!(
            patched["timezone"], "UTC",
            "field not provided is untouched"
        );

        // GET /user-info reflects the update.
        let info: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await
            .json();
        assert_eq!(info["profile"]["language"], "fr");
        assert_eq!(info["profile"]["theme"], "dark");

        // Second patch: timezone only, the rest holds.
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
async fn patch_profile_rejects_invalid_values_and_empty_bodies() {
    with_app(|server, token| async move {
        server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await;

        // Unsupported language.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "language": "klingon" }))
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(body["errors"]["fields"]["language"], "invalid");

        // Unknown theme.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({ "theme": "neon" }))
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["theme"],
            "invalid"
        );

        // No field.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&token))
            .json(&serde_json::json!({}))
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "profile-patch-empty"
        );

        // Without a token.
        let res = server
            .patch("/api/v1/profile")
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "language": "fr" }))
            .await;
        assert_eq!(res.status_code(), 401);
    })
    .await;
}
