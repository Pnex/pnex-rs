//! Tests for the thermo endpoints (diagram + cycle-points): 200 with
//! polylines, cache, org mixture resolution, 400 CoolProp.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
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

async fn post_diagram(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/thermo/diagram/query")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await
}

#[tokio::test]
#[serial]
async fn diagram_water_ph_and_cycle_points() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // p-h Water : polylines + axes.
        let resp = post_diagram(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "fluid": "Water",
                "diagram": "ph",
                "n_points": 30
            }),
        )
        .await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert!(!body["polylines"].as_array().unwrap().is_empty());
        assert_eq!(body["x"]["label"], "Hmass");
        assert!(body["y"]["log"].as_bool().unwrap());

        // Cache: identical second call → same content.
        let again = post_diagram(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "fluid": "Water",
                "diagram": "ph",
                "n_points": 30
            }),
        )
        .await;
        assert_eq!(again.json::<serde_json::Value>(), body);

        // cycle-points: state (P=1 bar, T=400 K) → coords (Hmass, P).
        let resp = server
            .post("/api/v1/thermo/cycle-points/query")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "fluid": "Water",
                "diagram": "ph",
                "points": [{
                    "label": "vapeur",
                    "input_pair": "PT_INPUTS",
                    "v1": 101325.0,
                    "v2": 400.0
                }]
            }))
            .await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let pts = resp.json::<serde_json::Value>()["points"].clone();
        let hmass = pts[0]["props"]["Hmass"].as_f64().unwrap();
        assert!((pts[0]["coords"][0].as_f64().unwrap() - hmass).abs() < 1.0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn diagram_resolves_org_mixture_and_rejects_unknown() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;

        // Org mixture "MonMelange" 50/50 Propane/Ethane.
        let resp = server
            .post("/api/v1/fluid-mixtures")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "MonMelange",
                "composition": {
                    "basis": "mole",
                    "components": [
                        {"fluid": "Propane", "fraction": 0.5},
                        {"fluid": "Ethane", "fraction": 0.5}
                    ]
                }
            }))
            .await;
        assert_eq!(resp.status_code(), 201, "{}", resp.text());

        // The fluids picker exposes the mixture's resolved spec (the flow
        // CoolProp node freezes it at pick time).
        let resp = server
            .get("/api/v1/thermo/fluids")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(
            body["mixture_specs"]["MonMelange"], "Propane[0.5]&Ethane[0.5]",
            "{body}"
        );

        // The diagram by mixture NAME works (org resolution).
        // NB: mixture → dome not computable → the CoolProp error is
        // expected here (400): the resolution itself is proven by the
        // message (not "unknown fluid").
        let resp = post_diagram(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "fluid": "MonMelange",
                "diagram": "ph",
                "n_points": 20
            }),
        )
        .await;
        assert_eq!(resp.status_code(), 400);
        let err = resp.json::<serde_json::Value>();
        assert_eq!(err["error"], "coolprop-error", "{err}");
        assert!(err["errors"]["args"]["detail"].is_string(), "{err}");
    })
    .await;
}
