//! The sales follow-up pack (crm.md D448–D452): installed with what it
//! builds on, its flows as drafts, opportunities with their account, people
//! and qualification, the opportunity sheet and the account's tree.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{json, Value};
use serial_test::serial;

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
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
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

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn me(server: &axum_test::TestServer, token: &str) -> Value {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
    props: Value,
) -> String {
    let (st, o) = call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": props})),
    )
    .await;
    assert_eq!(st, 201, "{o}");
    o["id"].as_str().unwrap().to_string()
}

async fn link(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    link_type: &str,
    from: &str,
    to: &str,
    attributes: Value,
) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/ontology/links",
        token,
        org,
        Some(json!({"link_type": link_type, "source_id": from, "target_id": to, "attributes": attributes})),
    )
    .await
}

#[tokio::test]
#[serial]
async fn an_opportunity_lives_in_the_account_graph() {
    with_app(|server, env| async move {
        let _ = &env.bob;
        let org = me(&server, &env.alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (st, e) = call(&server, "POST", "/api/v1/ontology/packs/crm/install", &env.alice, org, None).await;
        assert_eq!(st, 200, "{e}");

        // What it builds on is installed first; its flows wait as drafts.
        let count = |sql: String| {
            let db = env.db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<i64>("", "n")
                    .unwrap()
            }
        };
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM packs WHERE org_id = {org} AND key IN ('prospection_b2b', 'crm')")).await,
            2
        );
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org} AND name LIKE 'CRM · %' AND status = 'draft'")).await,
            7
        );

        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({"statut_compte": "prospect"})).await;
        let ann = object(&server, &env.alice, org, "person", "Ann Smith", json!({})).await;
        let budget = object(&server, &env.alice, org, "qualification_criterion", "Budget confirmed", json!({"stage": "qualification", "active": true})).await;
        let deal = object(&server, &env.alice, org, "opportunity", "Acme 2027", json!({
            "stage": "qualification", "source": "trade_show", "source_detail": "Global Industrie",
            "amount": {"amount": 48000, "currency": "EUR"}
        })).await;
        // A stage outside the list is refused.
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org,
            Some(json!({"type_key": "opportunity", "title": "X", "properties": {"stage": "signed"}}))).await;
        assert_eq!(st, 400);

        let (st, e) = link(&server, &env.alice, org, "opportunity_with", &deal, &acme, json!({})).await;
        assert_eq!(st, 201, "{e}");
        let beta = object(&server, &env.alice, org, "organization", "Beta", json!({})).await;
        let (st, _) = link(&server, &env.alice, org, "opportunity_with", &deal, &beta, json!({})).await;
        assert_eq!(st, 409, "one account per opportunity");
        let (st, e) = link(&server, &env.alice, org, "involved_in", &ann, &deal, json!({"role": "decision_maker"})).await;
        assert_eq!(st, 201, "{e}");
        let (st, e) = link(&server, &env.alice, org, "qualified_on", &deal, &budget, json!({"status": "partial", "note": "being decided"})).await;
        assert_eq!(st, 201, "{e}");

        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{acme}/hierarchy?key=opportunities"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let titles: Vec<&str> = h["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Acme", "Acme 2027"]);
        assert!(h["counts"].to_string().contains("\"qualification\""), "{h}");

        let (st, page) = call(&server, "POST", &format!("/api/v1/ontology/objects/{deal}/pages"), &env.alice, org,
            Some(json!({"template": "opportunity_sheet"}))).await;
        assert_eq!(st, 201, "{page}");
        let snap = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT snapshot FROM docs WHERE id = '{}'", page["id"].as_str().unwrap()))).await.unwrap().unwrap()
            .try_get::<Vec<u8>>("", "snapshot").unwrap();
        let text = String::from_utf8_lossy(&snap);
        for want in ["Acme 2027", "Acme", "Ann Smith", "Budget confirmed", "Global Industrie"] {
            assert!(text.contains(want), "{want} missing");
        }
    })
    .await;
}

/// « elodie » finds « Élodie » (query and global search), and an objection
/// recorded on « Élodie » blocks « ELODIE » at the same account (crm.md D447).
#[tokio::test]
#[serial]
async fn names_match_without_accents_or_case() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/crm/install", &env.alice, org, None).await;
        assert_eq!(st, 200);
        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({})).await;
        let elodie = object(&server, &env.alice, org, "person", "Élodie Martin", json!({"notes": "Rencontrée à Lyon"})).await;
        object(&server, &env.alice, org, "person", "Bob Durand", json!({})).await;

        let (_, q) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org,
            Some(json!({"type_key": "person", "text": "ELODIE"}))).await;
        assert_eq!(q["count"], 1, "{q}");
        assert_eq!(q["rows"][0]["object"]["title"], "Élodie Martin");
        let (_, q) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org,
            Some(json!({"type_key": "person", "filters": [{"key": "notes", "op": "contains", "value": "rencontree a lyon"}]}))).await;
        assert_eq!(q["count"], 1, "{q}");

        let (st, s) = call(&server, "GET", "/api/v1/search?q=elodie", &env.alice, org, None).await;
        assert_eq!(st, 200, "{s}");
        assert!(s.to_string().contains("Élodie Martin"), "{s}");

        let (st, _) = link(&server, &env.alice, org, "employs", &acme, &elodie, json!({})).await;
        assert_eq!(st, 201);
        let (st, o) = call(&server, "POST", &format!("/api/v1/extension/people/{elodie}/opposition"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{o}");
        let twin = object(&server, &env.alice, org, "person", "ELODIE  martin", json!({})).await;
        let (st, e) = link(&server, &env.alice, org, "employs", &acme, &twin, json!({})).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("prospection-opposed")), "{e}");
    })
    .await;
}
