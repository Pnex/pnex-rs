//! The `org_resolve` node (account-mapping.md D332) against a local mock of
//! the company register: a clear match on port 0, namesakes to review on
//! port 1, a wrong postcode retried without it, a forged node refused.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use std::sync::Mutex;

use axum::extract::Query;
use axum::routing::get;
use axum::{Json, Router};
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::net::TcpListener;

/// Query strings the register mock received.
static SEEN: Mutex<Vec<std::collections::HashMap<String, String>>> = Mutex::new(Vec::new());

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

fn hit(siren: &str, name: &str, postcode: &str, city: &str, state: &str) -> Value {
    json!({"siren": siren, "nom_complet": name, "nom_raison_sociale": name,
           "etat_administratif": state, "activite_principale": "25.62B",
           "tranche_effectif_salarie": "42",
           "siege": {"code_postal": postcode, "libelle_commune": city},
           "matching_etablissements": []})
}

async fn spawn_register() -> String {
    async fn search(Query(q): Query<std::collections::HashMap<String, String>>) -> Json<Value> {
        SEEN.lock().unwrap().push(q.clone());
        let results = match (
            q.get("q").map(String::as_str),
            q.get("code_postal").map(String::as_str),
        ) {
            (_, Some("99999")) => vec![],
            (Some(n), _) if n.to_lowercase().contains("chrome") => vec![
                hit("429853351", "MECACHROME FRANCE", "37530", "AMBOISE", "A"),
                hit("329936793", "MECACHROME TOULOUSE", "31200", "TOULOUSE", "A"),
                hit("542093166", "SOC MECACHROME", "37530", "AMBOISE", "C"),
            ],
            _ => vec![],
        };
        Json(json!({"results": results, "total_results": results.len()}))
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/search", get(search)))
            .await
            .unwrap();
    });
    format!("http://{addr}/search")
}

/// A deployed flow of `org` with these nodes; returns its id.
async fn deploy(db: &DatabaseConnection, org: i64, nodes: Value) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("resolve".into()),
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
        .post("/internal/flow/org-resolve")
        .add_header("x-pnex-flow-token", "tok-or-1")
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
async fn node_scores_register_candidates() {
    with_app(|server, alice, db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let register = spawn_register().await;
        unsafe { std::env::set_var("PNEX_COMPANY_SEARCH_URL", &register) };
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-or-1") };
        SEEN.lock().unwrap().clear();
        let flow = deploy(&db, org, json!([
            {"id": "r", "kind": "org_resolve", "config": {"limit": 2}}
        ])).await;
        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": id});

        // Name and city: one clear match, candidates best first.
        let (st, r) = node_call(&server, json!({"node": node("r"), "query": {"name": "Mécachrome France SAS", "city": "Amboise"}})).await;
        assert_eq!((st, r["port"].clone(), r["siren"].clone()), (200, json!(0), json!("429853351")), "{r}");
        assert_eq!(r["candidates"].as_array().unwrap().len(), 2, "limit: {r}");
        assert_eq!(r["candidates"][0]["score"], 95);
        assert_eq!(r["candidates"][0]["headcount_band"], "42");

        // A name alone: 80 at most, the ceased namesake too close → review.
        let (_, r) = node_call(&server, json!({"node": node("r"), "query": {"name": "Mecachrome"}})).await;
        assert_eq!((r["port"].clone(), r["siren"].clone()), (json!(1), Value::Null), "{r}");

        // A wrong postcode is retried without it; the exact name alone,
        // with no close namesake, still reaches the threshold.
        SEEN.lock().unwrap().clear();
        let (_, r) = node_call(&server, json!({"node": node("r"), "query": {"name": "Mecachrome France", "postcode": "99999"}})).await;
        assert_eq!((r["port"].clone(), r["candidates"][0]["score"].clone()), (json!(0), json!(80)), "{r}");
        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].get("code_postal").map(String::as_str), Some("99999"));
        assert_eq!(seen[1].get("code_postal"), None);

        let (_, r) = node_call(&server, json!({"node": node("r"), "query": {"name": " "}})).await;
        assert_eq!((r["port"].clone(), r["code"].clone()), (json!(2), json!("org-resolve-query-invalid")));

        // Options come from the deployed version only; no token, no call.
        let (st, e) = node_call(&server, json!({"node": node("ghost"), "query": {"name": "x"}})).await;
        assert_eq!((st, e["code"].as_str()), (403, Some("flow-node-not-deployed")));
        let res = server
            .post("/internal/flow/org-resolve")
            .json(&json!({"node": node("r"), "query": {"name": "x"}}))
            .await;
        assert_eq!(res.status_code().as_u16(), 401);

        // The register down: a coded failure.
        unsafe { std::env::set_var("PNEX_COMPANY_SEARCH_URL", "http://127.0.0.1:9/search") };
        let (_, r) = node_call(&server, json!({"node": node("r"), "query": {"name": "Mecachrome"}})).await;
        assert_eq!((r["port"].clone(), r["code"].clone()), (json!(2), json!("org-resolve-unavailable")));
        unsafe { std::env::remove_var("PNEX_COMPANY_SEARCH_URL") };
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
