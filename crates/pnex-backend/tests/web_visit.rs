//! The `web_visit` node (web-presence.md D364) against a local site: the
//! page and its legal notice read, robots.txt obeyed, a private address
//! refused, a forged node refused.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use axum::response::Html;
use axum::routing::get;
use axum::Router;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::net::TcpListener;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx.db.clone()).await;
        },
    )
    .await;
}

async fn spawn_site() -> String {
    let app = Router::new()
        .route("/robots.txt", get(|| async { "User-agent: *\nDisallow: /prive\n" }))
        .route(
            "/",
            get(|| async {
                Html(
                    "<html><head><title>ACME Formation</title><script>evil()</script></head>\
                     <body><h1>Bienvenue</h1><p>Organisme de formation à Lyon.</p>\
                     <a href=\"https://ailleurs.fr/mentions-legales\">autre</a>\
                     <a href=\"/Mentions-Legales\">Mentions légales</a></body></html>",
                )
            }),
        )
        .route(
            "/Mentions-Legales",
            get(|| async {
                Html("<html><body><p>ACME SAS, SIREN 123 456 789, NDA 84691234569.</p></body></html>")
            }),
        )
        .route("/prive", get(|| async { Html("<p>secret</p>") }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// A deployed flow of `org` with these nodes; returns its id.
async fn deploy(db: &DatabaseConnection, org: i64, nodes: Value) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("visit".into()),
        status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
        org_id: Set(org),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(json!({ "nodes": nodes })),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let id = flow.id;
    let mut am: flows::ActiveModel = flow.into();
    am.deployed_version_id = Set(Some(version.id));
    am.update(db).await.unwrap();
    id
}

async fn node_call(server: &axum_test::TestServer, body: Value) -> (u16, Value) {
    let res = server
        .post("/internal/flow/web-visit")
        .add_header("x-pnex-flow-token", "tok-wv-1")
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial]
async fn node_reads_a_page_and_its_legal_notice() {
    with_app(|server, alice, db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let site = spawn_site().await;
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-wv-1") };
        let flow = deploy(
            &db,
            org,
            json!([
                {"id": "v", "kind": "web_visit"},
                {"id": "h", "kind": "web_visit", "config": {"legal_page": false}}
            ]),
        )
        .await;
        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": id});

        let (st, r) = node_call(
            &server,
            json!({"node": node("v"), "url": format!("{site}/")}),
        )
        .await;
        assert_eq!(
            (st, r["port"].clone(), r["status"].clone()),
            (200, json!(0), json!(200)),
            "{r}"
        );
        assert_eq!(r["title"], "ACME Formation");
        let text = r["text"].as_str().unwrap();
        assert!(
            text.contains("Organisme de formation") && !text.contains("evil"),
            "{text}"
        );
        assert_eq!(
            r["legal_url"],
            format!("{site}/Mentions-Legales"),
            "the off-site link is skipped"
        );
        assert!(
            r["legal_text"]
                .as_str()
                .unwrap()
                .contains("SIREN 123 456 789"),
            "{r}"
        );

        let (_, r) = node_call(
            &server,
            json!({"node": node("h"), "url": format!("{site}/")}),
        )
        .await;
        assert_eq!(
            (r["port"].clone(), r["legal_url"].clone()),
            (json!(0), Value::Null),
            "{r}"
        );

        let (_, r) = node_call(
            &server,
            json!({"node": node("v"), "url": format!("{site}/prive")}),
        )
        .await;
        assert_eq!(
            (r["port"].clone(), r["code"].clone()),
            (json!(1), json!("web-visit-blocked")),
            "{r}"
        );
        let (_, r) = node_call(&server, json!({"node": node("v"), "url": "ftp://acme.fr/"})).await;
        assert_eq!(r["code"], "web-visit-url-invalid", "{r}");
        let (_, r) = node_call(
            &server,
            json!({"node": node("v"), "url": format!("{site}/absent")}),
        )
        .await;
        assert_eq!(r["code"], "web-visit-unreachable", "{r}");

        let (st, e) = node_call(
            &server,
            json!({"node": node("ghost"), "url": format!("{site}/")}),
        )
        .await;
        assert_eq!(
            (st, e["code"].as_str()),
            (403, Some("flow-node-not-deployed"))
        );
        let res = server
            .post("/internal/flow/web-visit")
            .json(&json!({"node": node("v"), "url": format!("{site}/")}))
            .await;
        assert_eq!(res.status_code().as_u16(), 401);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
