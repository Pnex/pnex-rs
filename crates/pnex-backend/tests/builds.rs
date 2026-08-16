//! Tests de parité du domaine builds firmware (Phase 6) : POST build-firmware
//! (ordres de vérification Django : 404 device, 403 quota, 429 intervalle),
//! worker inline (ForegroundBlocking — le build est terminal au 201),
//! échec/timeout de toolchain via les fixtures, download proxifié — cf.
//! `docs/contracts/build.http`.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Toolchain remplacée par tests/fixtures/firmware (config test.yaml) :
//! `fail`/`sleep` en WIFI_SSID pilotent échec/timeout.

mod common;

use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

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

async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    device_id: &str,
) {
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "sensor_probe_v1",
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
}

async fn post_build(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    device_id: &str,
    wifi_ssid: &str,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/build-firmware")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "wifi_ssid": wifi_ssid,
            "wifi_password": "pass-wifi",
            "predefined_device_name": "sensor_probe_v1",
            "pnex_host": "dev1.pnex.io",
            "device_id": device_id,
        }))
        .await
}

async fn records(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/build-records{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
        .json()
}

/// Cycle complet : 201 → worker inline (ForegroundBlocking) → record
/// `succeeded` + artefact dans le magasin → download proxifié avec les
/// secrets propagés (WiFi clair, host base64 — matérialisés par la fixture).
#[tokio::test]
#[serial]
async fn build_reussi_chemin_complet() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "capteur-jardin").await;

        let res = post_build(&server, &env.alice, org, "capteur-jardin", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = res.json();
        assert_eq!(body["build_record_created"], true);
        assert_eq!(body["status"], "queued");
        let build_id = body["build_id"].as_i64().expect("build_id");

        // ForegroundBlocking : le build a tourné dans la requête — le
        // record est déjà terminal.
        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["count"], 1);
        let record = &list["results"][0];
        assert_eq!(record["id"], build_id);
        assert_eq!(record["success"], true);
        assert_eq!(record["build_phase"], "succeeded");
        assert_eq!(
            record["firmware_bin_s3_key"],
            format!("org_{org}/firmware/capteur-jardin-firmware.bin")
        );

        // Download : proxy + attachment + contenu de la fixture (les env du
        // sous-process y sont matérialisées ; HOST arrive en base64).
        let dl = server
            .get("/api/v1/download/firmware/capteur-jardin")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        dl.assert_status(axum_test::http::StatusCode::OK);
        let disposition = dl
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            disposition,
            "attachment; filename=\"capteur-jardin-firmware.bin\""
        );
        let content = dl.text();
        assert!(content.contains("fixture ssid=coloc"), "{content}");
        let host_b64 = base64::engine::general_purpose::STANDARD
            .encode("dev1.pnex.io");
        assert!(content.contains(&format!("host={host_b64}")), "{content}");
    })
    .await;
}

/// Intervalle min : un 2e build juste après un succès → 429 (string Django
/// exacte), aucun record supplémentaire.
#[tokio::test]
#[serial]
async fn build_intervalle_429() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-a").await;
        create_device(&server, &env.alice, org, "dev-b").await;

        let first = post_build(&server, &env.alice, org, "dev-a", "coloc").await;
        first.assert_status(axum_test::http::StatusCode::CREATED);

        // Tier Free : min_build_interval 300 s — l'intervalle compte les
        // builds RÉUSSIS, tous devices confondus.
        let second = post_build(&server, &env.alice, org, "dev-b", "coloc").await;
        second.assert_status(axum_test::http::StatusCode::TOO_MANY_REQUESTS);
        let body: serde_json::Value = second.json();
        assert_eq!(
            body["error"],
            "Build interval not met for your subscription tier. Please wait before next build"
        );

        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["count"], 1, "pas de record pour le build refusé");
    })
    .await;
}

/// Quota devices du type (Free : 3 sensors) → 403, string Django exacte.
#[tokio::test]
#[serial]
async fn build_quota_403() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        for i in 1..=3 {
            create_device(&server, &env.alice, org, &format!("dev-{i}")).await;
        }
        let res = post_build(&server, &env.alice, org, "dev-1", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::FORBIDDEN);
        let body: serde_json::Value = res.json();
        assert_eq!(
            body["error"],
            "Device limit reached for sensor devices in your subscription tier."
        );
    })
    .await;
}

/// Device introuvable dans l'org → 404, string Django exacte.
#[tokio::test]
#[serial]
async fn build_device_inconnu_404() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let res = post_build(&server, &env.alice, org, "fantome", "coloc").await;
        res.assert_status(axum_test::http::StatusCode::NOT_FOUND);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "Device with ID 'fantome' not found");
    })
    .await;
}

/// Validation champ-par-champ (forme DRF) : requis, longueur, modèle
/// inconnu.
#[tokio::test]
#[serial]
async fn validation_400() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-val").await;

        // Requis (chaîne vide) + trop long + modèle inconnu + hôte avec
        // espaces.
        let cases = [
            (
                serde_json::json!({
                    "wifi_ssid": "",
                    "wifi_password": "p",
                    "predefined_device_name": "sensor_probe_v1",
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "dev-val",
                }),
                "wifi_ssid",
                "This field is required.",
            ),
            (
                serde_json::json!({
                    "wifi_ssid": "x".repeat(101),
                    "wifi_password": "p",
                    "predefined_device_name": "sensor_probe_v1",
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "dev-val",
                }),
                "wifi_ssid",
                "Ensure this field has no more than 100 characters.",
            ),
            (
                serde_json::json!({
                    "wifi_ssid": "coloc",
                    "wifi_password": "p",
                    "predefined_device_name": "inconnu_modele",
                    "pnex_host": "dev1.pnex.io",
                    "device_id": "dev-val",
                }),
                "predefined_device_name",
                "PredefinedDevice with name inconnu_modele does not exist.",
            ),
        ];
        for (body, field, msg) in cases {
            let res = server
                .post("/api/v1/build-firmware")
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/json")
                .json(&body)
                .await;
            res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
            let out: serde_json::Value = res.json();
            assert_eq!(out[field], msg, "{out}");
        }
    })
    .await;
}

/// Échec de toolchain (fixture `fail`) : 201 quand même, record `failed`,
/// download 404.
#[tokio::test]
#[serial]
async fn build_echec_outil() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-fail").await;

        let res = post_build(&server, &env.alice, org, "dev-fail", "fail").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);

        let list = records(&server, &env.alice, org, "").await;
        let record = &list["results"][0];
        assert_eq!(record["build_phase"], "failed");
        assert_eq!(record["success"], false);

        let dl = server
            .get("/api/v1/download/firmware/dev-fail")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        dl.assert_status(axum_test::http::StatusCode::NOT_FOUND);
    })
    .await;
}

/// Timeout dur (fixture `sleep`, budget test 2 s) : build `failed` en
/// temps borné.
#[tokio::test]
#[serial]
async fn build_timeout() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "dev-slow").await;

        let started = std::time::Instant::now();
        let res = post_build(&server, &env.alice, org, "dev-slow", "sleep").await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        // Inline : le timeout a déjà couru dans la requête (2 s budget).
        assert!(started.elapsed().as_secs() >= 2, "le timeout doit avoir couru");

        let list = records(&server, &env.alice, org, "").await;
        assert_eq!(list["results"][0]["build_phase"], "failed");
        assert!(started.elapsed().as_secs() < 30, "test borné");
    })
    .await;
}
