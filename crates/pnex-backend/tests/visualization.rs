//! Tests des endpoints `/api/v1/telemetry/*` de la page Visualisation
//! (2026-08-19) : sans OpenObserve configuré (section absente de
//! test.yaml), les deux endpoints répondent 200 dégradés
//! (`available: false`), jamais 500 ; les paramètres sont validés
//! (anti-injection PromQL) ; X-Org-Id requis.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Le parcours disponible (available:true) contre le mock O2 vit dans
//! tests/openobserve.rs.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
}

/// Boot l'app + seed du catalogue minimal (même harnais que dashboard.rs).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
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

async fn series(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    metric: &str,
    device_id: &str,
    window: &str,
) -> axum_test::TestResponse {
    server
        .get(&format!(
            "/api/v1/telemetry/series?metric={metric}&device_id={device_id}&window={window}"
        ))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
}

/// Sans O2 configuré : catalogue et série répondent 200 dégradés
/// (`available: false`, listes vides) — jamais 500 depuis la branche
/// télémétrie. Les paramètres hostiles sont rejetés en 400 AVANT toute
/// construction de requête PromQL (injection impossible), et X-Org-Id
/// reste requis.
#[tokio::test]
#[serial]
async fn telemetry_degrade_et_validation_sans_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Catalogue dégradé.
        let res = server
            .get("/api/v1/telemetry/catalog")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["available"], false);
        assert_eq!(body["series"].as_array().unwrap().len(), 0);

        // Série dégradée avec des paramètres valides.
        let res = series(&server, &env.alice, org, "soil_moisture", "esp-001", "24h").await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["available"], false);
        assert_eq!(body["metric"], "soil_moisture");
        assert_eq!(body["device_id"], "esp-001");
        assert_eq!(body["points"].as_array().unwrap().len(), 0);

        // Anti-injection PromQL : charset fermé métrique/device + fenêtre
        // preset — tout le reste est rejeté avant d'atteindre O2. Les
        // payloads hostiles sont percent-encodés (un client réel encode
        // forcément ces caractères, le serveur décode puis valide).
        let res = series(
            &server,
            &env.alice,
            org,
            "x%22%7Dor%20up%7B",
            "esp-001",
            "24h",
        )
        .await;
        assert_eq!(res.status_code(), 400, "métrique hostile rejetée");
        let res = series(
            &server,
            &env.alice,
            org,
            "soil_moisture",
            "zebra%22or%20up%28",
            "24h",
        )
        .await;
        assert_eq!(res.status_code(), 400, "device_id hostile rejeté");
        let res = series(&server, &env.alice, org, "soil_moisture", "esp-001", "7d").await;
        assert_eq!(res.status_code(), 400, "fenêtre non preset rejetée");
        let res = series(&server, &env.alice, org, "", "esp-001", "24h").await;
        assert_eq!(res.status_code(), 400, "métrique vide rejetée");

        // Paramètre manquant → 400 (désérialisation Query).
        let res = server
            .get("/api/v1/telemetry/series?metric=soil_moisture")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400);

        // Sans X-Org-Id → 400 (extracteur OrgContext).
        let res = server
            .get("/api/v1/telemetry/catalog")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// `POST /api/v1/telemetry/series-batch` (D31, studio SCADA) — sans O2
/// configuré : les items valides sont **dégradés au niveau item**
/// (`available: false`), une spec invalide est isolée (jamais de 400
/// globale), l'ordre de la réponse suit l'ordre des specs, et le cap de
/// specs est une vraie 400 (erreur d'appel, pas une donnée manquante).
#[tokio::test]
#[serial]
async fn series_batch_degrade_par_item_sans_o2() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Batch mixte : 2 specs valides + 1 invalide (device hostile +
        // fenêtre inconnue).
        let res = server
            .post("/api/v1/telemetry/series-batch")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "specs": [
                    { "metric": "soil_moisture", "device_id": "esp-001", "window": "5m" },
                    { "metric": "x\"}or up{", "device_id": "esp-001", "window": "1h" },
                    { "metric": "soil_temperature", "device_id": "esp-002", "window": "24h" },
                    // D171 label selectors: device-less valid, hostile value isolated.
                    { "metric": "media_capture_up", "device_id": "", "window": "1h",
                      "labels": { "stream": "inter" } },
                    { "metric": "media_capture_up", "device_id": "", "window": "1h",
                      "labels": { "stream": "x\"}or up{" } }
                ]
            }))
            .await;
        assert_eq!(res.status_code(), 200, "jamais de 400 sur spec invalide");
        let body: serde_json::Value = res.json();
        let results = body["results"].as_array().expect("results");
        assert_eq!(results.len(), 5, "un item par spec, même ordre");
        assert_eq!(results[0]["available"], false);
        assert_eq!(results[0]["metric"], "soil_moisture");
        assert_eq!(results[1]["available"], false, "spec hostile isolée");
        assert_eq!(results[2]["available"], false);
        assert_eq!(results[3]["metric"], "media_capture_up");
        assert_eq!(results[4]["available"], false, "hostile label isolated");

        // Sans specs : batch vide mais 200 (dashboard sans source).
        let res = server
            .post("/api/v1/telemetry/series-batch")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "specs": [] }))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["results"].as_array().unwrap().len(), 0);

        // Cap anti-abus : au-delà de 24 specs → 400 (forme d'appel
        // invalide, distincte d'une donnée dégradée).
        let mut specs = Vec::new();
        for i in 0..25 {
            specs.push(serde_json::json!({
                "metric": "soil_moisture", "device_id": format!("esp-{i:03}"), "window": "1h"
            }));
        }
        let res = server
            .post("/api/v1/telemetry/series-batch")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "specs": specs }))
            .await;
        assert_eq!(res.status_code(), 400, "cap specs dépassé → 400");

        // X-Org-Id reste requis sur le batch (comme le reste du domaine).
        let res = server
            .post("/api/v1/telemetry/series-batch")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "specs": [] }))
            .await;
        assert_eq!(res.status_code(), 400, "X-Org-Id absent → 400");
    })
    .await;
}
