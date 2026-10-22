//! Integration tests of the org web search providers and the `web_search`
//! node (web-presence.md D360–D361): CRUD with the key in the vault, viewer
//! refused on writes (R2), Test, and the node call against local mocks of
//! Serper and SearXNG (key added server-side only, exclusions applied, a
//! node absent from the deployed version refused).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use std::sync::Mutex;

use axum::extract::Query;
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::net::TcpListener;

struct Env {
    alice: String,
    bob: String,
    db: DatabaseConnection,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
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
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(
                server,
                Env {
                    alice,
                    bob,
                    db: ctx.db.clone(),
                },
            )
            .await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "DELETE" => server.delete(path),
        _ => server.get(path),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// What the mocks received: (kind, key header, query).
static SEEN: Mutex<Vec<(&'static str, Option<String>, String)>> = Mutex::new(Vec::new());

fn header(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(String::from)
}

/// Serper (`POST /search`) and SearXNG (`GET /search?format=json`) on one port.
async fn spawn_mock() -> String {
    async fn serper(h: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let q = body["q"].as_str().unwrap_or_default().to_string();
        SEEN.lock()
            .unwrap()
            .push(("serper", header(&h, "x-api-key"), q));
        Json(json!({ "organic": [
            {"title": "ACME | LinkedIn", "link": "https://fr.linkedin.com/company/acme", "snippet": "", "position": 1},
            {"title": "ACME Formation", "link": "https://www.acme-formation.fr/", "snippet": "Organisme de formation", "position": 2},
            {"title": "ACME sur Pappers", "link": "https://www.pappers.fr/entreprise/acme", "snippet": "", "position": 3},
            {"title": "Mentions légales", "link": "https://www.acme-formation.fr/mentions-legales", "snippet": "SIREN 123456789", "position": 4}
        ]}))
    }
    async fn searx(
        h: HeaderMap,
        Query(q): Query<std::collections::HashMap<String, String>>,
    ) -> Json<Value> {
        assert_eq!(q.get("format").map(String::as_str), Some("json"));
        let query = q.get("q").cloned().unwrap_or_default();
        SEEN.lock()
            .unwrap()
            .push(("searxng", header(&h, "authorization"), query));
        Json(json!({ "results": [
            {"url": "https://acme.example.org/", "title": "ACME", "content": "Accueil", "engine": "x"}
        ]}))
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = Router::new().route("/search", get(searx).post(serper));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock search");
    });
    format!("http://{addr}")
}

/// A deployed flow of `org` with web_search nodes; returns its id.
async fn deploy(db: &DatabaseConnection, org: i64, nodes: Value) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("search".into()),
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
        .post("/internal/flow/web-search")
        .add_header("x-pnex-flow-token", "tok-ws-1")
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
async fn providers_keep_their_key_and_viewers_only_read() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let _ = personal_org(&server, &env.bob).await;
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({"email": "bob@example.com", "role": "viewer"})),
        )
        .await;
        assert_eq!(st, 201);

        // Serper needs its key; SearXNG needs its address.
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/web-search/providers",
            &env.alice,
            org,
            Some(json!({"name": "s", "kind": "serper"})),
        )
        .await;
        assert_eq!(st, 400, "{e}");
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/web-search/providers",
            &env.alice,
            org,
            Some(json!({"name": "x", "kind": "searxng"})),
        )
        .await;
        assert_eq!(st, 400, "{e}");

        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/web-search/providers",
            &env.alice,
            org,
            Some(json!({"name": "s", "kind": "serper", "api_key": {"value": "serper-secret"}})),
        )
        .await;
        assert_eq!(st, 201, "{p}");
        assert!(!p.to_string().contains("serper-secret"), "{p}");
        assert_eq!(p["is_default"], true, "the first provider is the default");
        assert!(p["api_key"]["secret_id"].is_string());
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/web-search/providers",
            &env.alice,
            org,
            Some(json!({"name": "s", "kind": "serper", "api_key": {"value": "k"}})),
        )
        .await;
        assert_eq!(st, 409, "name taken");

        let id = p["id"].as_str().unwrap().to_string();
        let (st, list) = call(
            &server,
            "GET",
            "/api/v1/web-search/providers",
            &env.bob,
            org,
            None,
        )
        .await;
        assert_eq!((st, list["count"].clone()), (200, json!(1)));
        let body = json!({"name": "s2", "kind": "serper", "api_key": {"value": "k"}});
        for (method, path) in [
            ("POST", "/api/v1/web-search/providers".to_string()),
            ("PUT", format!("/api/v1/web-search/providers/{id}")),
            ("DELETE", format!("/api/v1/web-search/providers/{id}")),
            ("POST", format!("/api/v1/web-search/providers/{id}/test")),
        ] {
            let (st, e) = call(&server, method, &path, &env.bob, org, Some(body.clone())).await;
            assert_eq!(st, 403, "{method} {path}: {e}");
        }

        // An update without a key keeps the stored one.
        let (st, u) = call(
            &server,
            "PUT",
            &format!("/api/v1/web-search/providers/{id}"),
            &env.alice,
            org,
            Some(json!({"name": "serper", "kind": "serper", "is_default": true})),
        )
        .await;
        assert_eq!(st, 200, "{u}");
        assert_eq!(u["api_key"]["secret_id"], p["api_key"]["secret_id"]);
        let (st, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/web-search/providers/{id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 204);
        let (_, list) = call(
            &server,
            "GET",
            "/api/v1/web-search/providers",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(list["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn node_returns_candidates_from_the_org_provider() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let mock = spawn_mock().await;
        SEEN.lock().unwrap().clear();
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-ws-1") };
        let flow = deploy(&env.db, org, json!([
            {"id": "d", "kind": "web_search", "config": {"limit": 5, "country": "fr", "lang": "fr"}},
            {"id": "x", "kind": "web_search", "config": {"provider": "searx"}},
            {"id": "n", "kind": "web_search", "config": {"provider": "nope"}}
        ])).await;
        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": id});

        // No provider yet: a coded failure on port 1.
        let (st, r) = node_call(&server, json!({"node": node("d"), "query": "ACME formation Lyon"})).await;
        assert_eq!((st, r["port"].clone(), r["code"].clone()), (200, json!(1), json!("web-search-no-provider")), "{r}");

        let (st, _) = call(&server, "POST", "/api/v1/web-search/providers", &env.alice, org,
            Some(json!({"name": "serper", "kind": "serper", "base_url": mock, "api_key": {"value": "serper-secret"}}))).await;
        assert_eq!(st, 201);
        let (st, _) = call(&server, "POST", "/api/v1/web-search/providers", &env.alice, org,
            Some(json!({"name": "searx", "kind": "searxng", "base_url": mock}))).await;
        assert_eq!(st, 201);

        // Default provider (Serper): key sent by the server, directories dropped.
        let (st, r) = node_call(&server, json!({"node": node("d"), "query": "ACME formation Lyon"})).await;
        assert_eq!((st, r["port"].clone()), (200, json!(0)), "{r}");
        assert_eq!(r["provider"], "serper");
        let urls: Vec<&str> = r["results"].as_array().unwrap().iter().map(|h| h["url"].as_str().unwrap()).collect();
        assert_eq!(urls, ["https://www.acme-formation.fr/", "https://www.acme-formation.fr/mentions-legales"]);
        assert_eq!(r["results"][0]["rank"], 1);
        assert_eq!(r["results"][0]["domain"], "acme-formation.fr");
        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen.last().unwrap(), &("serper", Some("serper-secret".into()), "ACME formation Lyon".to_string()));

        // Named provider (SearXNG, no key).
        let (_, r) = node_call(&server, json!({"node": node("x"), "query": "ACME"})).await;
        assert_eq!((r["port"].clone(), r["provider"].clone()), (json!(0), json!("searx")), "{r}");
        assert_eq!(r["results"][0]["domain"], "acme.example.org");
        assert_eq!(SEEN.lock().unwrap().last().unwrap().1, None);

        let (_, r) = node_call(&server, json!({"node": node("n"), "query": "ACME"})).await;
        assert_eq!((r["code"].clone(), r["args"]["provider"].clone()), (json!("web-search-no-provider"), json!("nope")));
        let (_, r) = node_call(&server, json!({"node": node("d"), "query": "  "})).await;
        assert_eq!(r["code"], "web-search-query-invalid");

        // The node's options come from the deployed version only.
        let (st, e) = node_call(&server, json!({"node": node("ghost"), "query": "ACME"})).await;
        assert_eq!((st, e["code"].as_str()), (403, Some("flow-node-not-deployed")));
        let res = server
            .post("/internal/flow/web-search")
            .json(&json!({"node": node("d"), "query": "ACME"}))
            .await;
        assert_eq!(res.status_code().as_u16(), 401, "no flow token");

        // Test: one real query on the mock.
        let (_, list) = call(&server, "GET", "/api/v1/web-search/providers", &env.alice, org, None).await;
        let id = list["results"][0]["id"].as_str().unwrap().to_string();
        let (st, t) = call(&server, "POST", &format!("/api/v1/web-search/providers/{id}/test"), &env.alice, org, None).await;
        assert_eq!((st, t["ok"].clone()), (200, json!(true)), "{t}");
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
