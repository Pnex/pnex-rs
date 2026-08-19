//! Smoke test e2e du tier hobbyist sqlite (D5 v2) : boot complet sur un
//! fichier sqlite temporaire (migrations + truncate portable + seed),
//! cycle build firmware avec la toolchain de fixture, artefact en table
//! `firmware_artifacts`, download proxifié. Sans aucun service externe —
//! ni PostgreSQL ni queue : ForegroundBlocking (test.yaml).
//!
//! La suite principale reste sur PostgreSQL (TEST_DATABASE_URL) ; ce test
//! garantit la portabilité des migrations/SQL du tier sqlite.

mod common;

use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

/// Restaure les vars d'env mutées, même en panique — les autres tests
/// `#[serial]` du binaire tourment dans le même process.
struct EnvGuard {
    prev_db: Option<String>,
    prev_keycloak: Option<String>,
}

impl EnvGuard {
    fn capture() -> Self {
        Self {
            prev_db: std::env::var("TEST_DATABASE_URL").ok(),
            prev_keycloak: std::env::var("KEYCLOAK_URL").ok(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev_db.take() {
            Some(v) => unsafe { std::env::set_var("TEST_DATABASE_URL", v) },
            None => unsafe { std::env::remove_var("TEST_DATABASE_URL") },
        }
        match self.prev_keycloak.take() {
            Some(v) => unsafe { std::env::set_var("KEYCLOAK_URL", v) },
            None => unsafe { std::env::remove_var("KEYCLOAK_URL") },
        }
    }
}

/// Boot sqlite → build → artefact en base → download. Couvre : migrations
/// complètes sur sqlite (dont uuid_pk conditionnel), truncate portable
/// (`defer_foreign_keys`), upsert DbStore, proxy download.
#[tokio::test]
#[serial]
async fn sqlite_boot_build_download() {
    // Fichier sqlite temporaire (jamais :memory: — cf. development.yaml).
    let dir = tempfile::tempdir().expect("tmp");
    let db_path = dir.path().join("pnex_smoke.sqlite");
    let uri = format!("sqlite://{}?mode=rwc", db_path.display());

    let guard = EnvGuard::capture();
    unsafe { std::env::set_var("TEST_DATABASE_URL", &uri) };

    let base = common::spawn_mock_keycloak().await;
    unsafe { std::env::set_var("KEYCLOAK_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );

    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;

            // Org personnelle + device.
            let org: i64 = server
                .get("/api/v1/user-info")
                .add_header("Authorization", format!("Bearer {alice}"))
                .await
                .json::<serde_json::Value>()["orgs"][0]["id"]
                .as_i64()
                .expect("org personnelle");
            server
                .post("/api/v1/devices")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "device_id": "capteur-jardin",
                    "predefined_device_name": "soil_sensor",
                }))
                .await
                .assert_status(axum_test::http::StatusCode::CREATED);

            // Build inline (ForegroundBlocking) → succeeded.
            let res = server
                .post("/api/v1/build-firmware")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/json")
                .json(&serde_json::json!({
                    "wifi_ssid": "coloc",
                    "wifi_password": "pass-wifi",
                    "predefined_device_name": "soil_sensor",
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "capteur-jardin",
                }))
                .await;
            res.assert_status(axum_test::http::StatusCode::CREATED);
            let list: serde_json::Value = server
                .get("/api/v1/build-records")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await
                .json();
            assert_eq!(list["count"], 1, "{list}");
            assert_eq!(list["results"][0]["build_phase"], "succeeded", "{list}");
            assert_eq!(
                list["results"][0]["firmware_bin_s3_key"],
                format!("org_{org}/firmware/capteur-jardin-firmware.bin")
            );

            // L'artefact vit bien dans la table sqlite.
            use sea_orm::{ConnectionTrait, Statement};
            let backend = ctx.db.get_database_backend();
            let row = ctx
                .db
                .query_one_raw(Statement::from_string(
                    backend,
                    "SELECT COUNT(*) AS n FROM firmware_artifacts".to_string(),
                ))
                .await
                .expect("count artefacts")
                .expect("ligne");
            let n: i64 = row.try_get("", "n").expect("n");
            assert_eq!(n, 1, "un artefact en base");

            // Download proxifié : contenu de la fixture (secrets propagés).
            let dl = server
                .get("/api/v1/download/firmware/capteur-jardin")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            dl.assert_status(axum_test::http::StatusCode::OK);
            let content = dl.text();
            let ssid_b64 = base64::engine::general_purpose::STANDARD.encode("coloc");
            assert!(
                content.contains(&format!("fixture ssid={ssid_b64}")),
                "{content}"
            );
        },
    )
    .await;

    drop(guard);
}
