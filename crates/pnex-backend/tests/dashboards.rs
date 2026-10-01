//! Tests des dashboards du studio SCADA (D24/D40/D41) : CRUD + versions
//! append-only, save = live (pointeur), 409 concurrence optimiste,
//! restore = re-positionnement (école media), validation du layout
//! (400 `{"violations": [...]}`), isolation org (404 masqué), rôles
//! (viewer lecture seule), enveloppe D14.
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

/// Layout valide minimal : une jauge + un trait (couvre le chemin
/// heureux de validate_layout).
fn layout_json(widget_device: &str) -> serde_json::Value {
    serde_json::json!({
        "canvas": { "width": 1600, "height": 900, "background": "#f8fafc" },
        "widgets": [ {
            "id": "w1", "type": "gauge", "title": "Température",
            "x": 40, "y": 40, "w": 240, "h": 200,
            "source": [ {
                "role": "primary", "metric": "soil_moisture",
                "device_id": widget_device, "window": "1h"
            } ],
            "options": { "unit": "°C", "min": 0.0, "max": 50.0 }
        } ],
        "wires": []
    })
}

async fn create_dashboard(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
) -> serde_json::Value {
    server
        .post("/api/v1/dashboards")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "name": name }))
        .await
        .json::<serde_json::Value>()
}

#[tokio::test]
#[serial]
async fn cycle_creation_save_restore_et_isolation_org() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        // ── Création : v1 posée d'office, layout par défaut.
        let created = create_dashboard(&server, &env.alice, org_a, "Atelier").await;
        assert_eq!(created["current_version_number"], 1);
        assert_eq!(created["name"], "Atelier");
        assert_eq!(created["layout"]["canvas"]["width"], 1600);
        let id = created["id"].as_str().unwrap().to_string();

        // ── Save = nouvelle version = live (D24).
        let saved = server
            .patch(&format!("/api/v1/dashboards/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "layout": layout_json("soil-01"),
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(saved["current_version_number"], 2);
        assert_eq!(saved["layout"]["widgets"][0]["type"], "gauge");

        // ── Historique : 2 versions, v2 courante, append-only.
        let versions = server
            .get(&format!("/api/v1/dashboards/{id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await
            .json::<serde_json::Value>();
        let versions = versions.as_array().unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0]["version_number"], 2);
        assert_eq!(versions[0]["current"], true);
        assert_eq!(versions[1]["version_number"], 1);
        assert_eq!(versions[1]["current"], false);

        // ── Restore v1 (école media) : le pointeur recule, aucune v3.
        let restored = server
            .post(&format!("/api/v1/dashboards/{id}/versions/1/restore"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(restored["current_version_number"], 1);
        let versions = server
            .get(&format!("/api/v1/dashboards/{id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(versions.as_array().unwrap().len(), 2);

        // ── 409 : le save vise la version courante (le pointeur, pas le
        // dernier numéro) — attendu 1, courant 1, mais un save qui vise 2
        // (devenu faux après restore) doit être rejeté.
        let stale = server
            .patch(&format!("/api/v1/dashboards/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 2,
                "layout": layout_json("soil-01"),
            }))
            .await;
        assert_eq!(stale.status_code(), 409, "save périmé → 409");

        // ── Isolation org : bob (autre org) voit un 404 masqué.
        let cross = server
            .get(&format!("/api/v1/dashboards/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(cross.status_code(), 404);

        // ── Liste D14 : l'org de bob reste vide.
        let list = server
            .get("/api/v1/dashboards")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn violations_layout_et_roles_viewer() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let id = create_dashboard(&server, &env.alice, org, "Invalides").await["id"]
            .as_str()
            .unwrap()
            .to_string();

        // ── Layout invalide : widget hors canvas + fenêtre inconnue →
        // 400 violations (et PAS de version créée).
        let mut bad = layout_json("soil-01");
        bad["widgets"][0]["x"] = serde_json::json!(5000);
        bad["widgets"][0]["source"][0]["window"] = serde_json::json!("7j");
        let rejected = server
            .patch(&format!("/api/v1/dashboards/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "layout": bad,
            }))
            .await;
        assert_eq!(rejected.status_code(), 400);
        let body = rejected.json::<serde_json::Value>();
        let violations = body["violations"].as_array().expect("violations list");
        assert!(violations.iter().any(|v| v["code"] == "widget_overflow"));
        assert!(violations.iter().any(|v| v["code"] == "bad_window"));
        let versions = server
            .get(&format!("/api/v1/dashboards/{id}/versions"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(versions.as_array().unwrap().len(), 1, "aucune version");

        // ── Trait pendent (widget inconnu) rejeté aussi.
        let mut dangling = layout_json("soil-01");
        dangling["wires"] = serde_json::json!([ {
            "id": "t1",
            "from": { "widget_id": "w1", "side": "right" },
            "to": { "widget_id": "inconnu", "side": "left" }
        } ]);
        let rejected = server
            .patch(&format!("/api/v1/dashboards/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "layout": dangling,
            }))
            .await;
        assert_eq!(rejected.status_code(), 400);
        let body = rejected.json::<serde_json::Value>();
        assert!(body["violations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["code"] == "dangling_wire"));

        // ── Viewer : lecture OK, écriture 403.
        // (bob doit exister côté PNEX : le JIT provisioning tourne au
        // premier user-info — école sites.rs.)
        let _bob_org = personal_org(&server, &env.bob).await;
        server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let read = server
            .get(&format!("/api/v1/dashboards/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(read.status_code(), 200, "viewer lit le dashboard");
        let denied = server
            .patch(&format!("/api/v1/dashboards/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "layout": layout_json("soil-01"),
            }))
            .await;
        assert_eq!(denied.status_code(), 403, "viewer ne sauvegarde pas");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn save_sans_nom_valide_et_404_inconnu() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // ── Nom vide → 400 champ-par-champ.
        let bad = server
            .post("/api/v1/dashboards")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "name": "   " }))
            .await;
        assert_eq!(bad.status_code(), 400);
        assert_eq!(bad.json::<serde_json::Value>()["name"], "required");

        // ── 404 sur un UUID inconnu (même org).
        let missing = server
            .get("/api/v1/dashboards/00000000-0000-0000-0000-000000000001")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(missing.status_code(), 404);

        // ── 404 sur restore d'une version inexistante.
        let id = create_dashboard(&server, &env.alice, org, "Vide").await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let missing_version = server
            .post(&format!("/api/v1/dashboards/{id}/versions/9/restore"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(missing_version.status_code(), 404);

        // ── Suppression : 204 puis 404.
        let deleted = server
            .delete(&format!("/api/v1/dashboards/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204);
        let gone = server
            .get(&format!("/api/v1/dashboards/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(gone.status_code(), 404);
    })
    .await;
}
