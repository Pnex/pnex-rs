//! Tests for the custom fluid mixtures CRUD: validation
//! (pnex-core + CoolProp instantiation), stored mass→molar conversion,
//! name uniqueness, org isolation (masked 404).
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

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
        .expect("personal org")
}

/// Valid molar composition (CoolProp README example).
fn composition_mole() -> serde_json::Value {
    serde_json::json!({
        "basis": "mole",
        "components": [
            {"fluid": "Propane", "fraction": 0.5},
            {"fluid": "Ethane", "fraction": 0.5}
        ]
    })
}

async fn create_mixture(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
    composition: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/fluid-mixtures")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "name": name,
            "composition": composition
        }))
        .await
}

/// Creates the mixture then returns its id (used by detail/update/delete).
async fn seed_mixture(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> i64 {
    let resp = create_mixture(server, token, org, name, composition_mole()).await;
    assert_eq!(resp.status_code(), 201, "seed: {}", resp.text());
    resp.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("integer id")
}

/// Full CRUD + org isolation: create → list → detail → cross-org 404
/// → update (stored mass conversion) → delete → 404 after delete.
#[tokio::test]
#[serial]
async fn crud_and_org_isolation() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        let id = seed_mixture(&server, &env.alice, org_a, "Boucle secondaire").await;

        // List: paginated envelope, the mixture is present.
        let list = server
            .get("/api/v1/fluid-mixtures")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 1, "{list}");
        assert_eq!(list["results"][0]["name"], "Boucle secondaire");
        // The inline CoolProp spec is exposed and correct.
        assert_eq!(list["results"][0]["spec"], "Propane[0.5]&Ethane[0.5]");

        // Detail + cross-org isolation (masked 404, never 403).
        let resp = server
            .get(&format!("/api/v1/fluid-mixtures/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(resp.status_code(), 200);
        let resp = server
            .get(&format!("/api/v1/fluid-mixtures/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(resp.status_code(), 404);

        // Update: switch to a 50/50 water/ethanol mass basis — the
        // response carries the converted molar fractions.
        let resp = server
            .put(&format!("/api/v1/fluid-mixtures/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({
                "name": "Boucle secondaire",
                "composition": {
                    "basis": "mass",
                    "components": [
                        {"fluid": "Water", "fraction": 0.5},
                        {"fluid": "Ethanol", "fraction": 0.5}
                    ]
                }
            }))
            .await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        let comp = &body["composition"];
        assert_eq!(comp["basis"], "mass");
        let x0 = comp["mole_fractions"][0].as_f64().expect("x0");
        assert!(
            (x0 - 0.7187).abs() < 1e-3,
            "x_water expected ≈0.7187, got {x0}"
        );
        assert!(comp["molar_mass"].as_f64().is_some());

        // Delete then 404.
        let resp = server
            .delete(&format!("/api/v1/fluid-mixtures/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(resp.status_code(), 204);
        let resp = server
            .get(&format!("/api/v1/fluid-mixtures/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(resp.status_code(), 404);
    })
    .await;
}

/// Rejections: sum ≠ 1 (pnex-core violations), unknown fluid (CoolProp
/// message), duplicate name, write without can_write.
#[tokio::test]
#[serial]
async fn validation_rejections_and_uniqueness() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;

        // Somme ≠ 1 → 400 {"violations": [...]}.
        let resp = create_mixture(
            &server,
            &env.alice,
            org_a,
            "Somme fausse",
            serde_json::json!({
                "basis": "mole",
                "components": [
                    {"fluid": "Propane", "fraction": 0.5},
                    {"fluid": "Ethane", "fraction": 0.2}
                ]
            }),
        )
        .await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        assert!(
            resp.json::<serde_json::Value>()["errors"]["violations"]
                .as_array()
                .expect("violations")
                .iter()
                .any(|v| v["code"] == "mixture-fraction-sum"),
            "{}",
            resp.text()
        );

        // Unknown fluid → 400 composition with the CoolProp message.
        let resp = create_mixture(
            &server,
            &env.alice,
            org_a,
            "Fluide fantôme",
            serde_json::json!({
                "basis": "mole",
                "components": [
                    {"fluid": "Propane", "fraction": 0.5},
                    {"fluid": "NOT_A_FLUID", "fraction": 0.5}
                ]
            }),
        )
        .await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["error"], "coolprop-error", "{body}");
        let detail = body["errors"]["args"]["detail"]
            .as_str()
            .unwrap_or_default();
        assert!(!detail.is_empty(), "CoolProp detail expected: {body}");

        // Duplicate name in the same org → 400 name.
        seed_mixture(&server, &env.alice, org_a, "Double").await;
        let resp = create_mixture(&server, &env.alice, org_a, "Double", composition_mole()).await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        assert_eq!(
            resp.json::<serde_json::Value>()["errors"]["fields"]["name"],
            "unique"
        );
    })
    .await;
}
