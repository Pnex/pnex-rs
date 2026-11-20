//! Rules (analysis.md D829–D830): permissions, a pass that records then notifies
//! only what starts matching, replay, optimistic writes.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and the compose `rustfs` service.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde_json::{json, Value};
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
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
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
    token: &str,
    org: i64,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "DELETE" => server.delete(path),
        "PUT" => server.put(path),
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

async fn sql(db: &DatabaseConnection, text: &str) -> Vec<QueryResult> {
    db.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, text))
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_rule_notifies_what_starts_matching() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "viewer").await;
        let a = env.alice.as_str();
        sql(&ctx.db, "DELETE FROM rules").await;
        let (s, t) = call(&server, a, org, "POST", "/api/v1/ontology/types", Some(json!({
            "def": { "key": "deal", "name": "Deal",
                     "properties": [{ "key": "amount", "name": "Amount", "kind": "number" }] } }))).await;
        assert_eq!(s, 201, "{t}");
        let deal = |title: &str, amount: i64| json!({ "type_key": "deal", "title": title, "properties": { "amount": amount } });
        call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(deal("small", 10))).await;
        call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(deal("big", 5000))).await;

        let body = json!({ "name": "Big deals", "condition": { "kind": "objects", "type_key": "deal",
            "filters": [{ "key": "amount", "op": "gt", "value": 1000 }] } });
        let (s, _) = call(&server, &env.bob, org, "POST", "/api/v1/rules", Some(body.clone())).await;
        assert_eq!(s, 403, "a viewer reads rules, never writes them");
        let (s, r) = call(&server, a, org, "POST", "/api/v1/rules", Some(body.clone())).await;
        assert_eq!(s, 201, "{r}");
        let id = r["id"].as_str().unwrap().to_string();
        let (s, e) = call(&server, a, org, "POST", "/api/v1/rules", Some(body)).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("rule-name-taken")), "{e}");
        let (s, l) = call(&server, &env.bob, org, "GET", "/api/v1/rules", None).await;
        assert_eq!((s, l["count"].as_i64()), (200, Some(1)), "{l}");

        // First pass: what matches already is recorded, not announced.
        pnex_backend::services::rules::pass(&ctx).await.unwrap();
        let (_, r) = call(&server, a, org, "GET", &format!("/api/v1/rules/{id}"), None).await;
        assert_eq!((r["firing"].as_bool(), r["last_fired_at"].is_null()), (Some(true), true), "{r}");
        // A new big deal: the next pass fires.
        call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(deal("huge", 90000))).await;
        let fired = pnex_backend::services::rules::pass(&ctx).await.unwrap();
        assert!(fired >= 1);
        let (_, r) = call(&server, a, org, "GET", &format!("/api/v1/rules/{id}"), None).await;
        assert!(!r["last_fired_at"].is_null(), "{r}");
        // Nothing new: no second notification.
        assert_eq!(pnex_backend::services::rules::pass(&ctx).await.unwrap(), 0);

        // The replay answers, complete or not depending on the history store.
        let (s, p) = call(&server, a, org, "POST", &format!("/api/v1/rules/{id}/replay"),
            Some(json!({ "window": "24h" }))).await;
        assert_eq!(s, 200, "{p}");
        assert!(p["events"].is_array() && p["complete"].is_boolean(), "{p}");
        let (s, _) = call(&server, a, org, "POST", &format!("/api/v1/rules/{id}/replay"),
            Some(json!({ "window": "3y" }))).await;
        assert_eq!(s, 400);

        // A stale version is refused; a deleted rule is gone.
        let (s, e) = call(&server, a, org, "PUT", &format!("/api/v1/rules/{id}"), Some(json!({
            "name": "Big deals", "condition": { "kind": "objects", "type_key": "deal" },
            "expected_version": 99 }))).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("rule-version-conflict")), "{e}");
        let (s, _) = call(&server, a, org, "DELETE", &format!("/api/v1/rules/{id}"), None).await;
        assert_eq!(s, 204);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_rule_proposes_a_linked_object() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = env.alice.as_str();
        sql(&ctx.db, "DELETE FROM rules").await;
        let (s, r) = call(&server, a, org, "POST", "/api/v1/ontology/packs/maintenance/install", None).await;
        assert!(s < 300, "{r}");
        let machine = |serial: &str| json!({ "type_key": "machine", "title": serial, "properties": { "serial": serial } });
        call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(machine("P-1"))).await;
        let propose = json!({ "type_key": "work_order", "title": "Inspect the new machine",
            "link_type": "work_order_on", "values": { "kind": "inspection", "state": "requested" } });
        let rule = |p: Value| json!({ "name": "New machines", "propose": p,
            "condition": { "kind": "objects", "type_key": "machine" } });

        // Unknown types are refused before anything runs.
        let (s, e) = call(&server, a, org, "POST", "/api/v1/rules",
            Some(rule(json!({ "type_key": "nope", "title": "x", "link_type": "work_order_on" })))).await;
        assert_eq!(s, 400, "{e}");
        assert!(e["errors"]["fields"].get("propose.type_key").is_some(), "{e}");
        let (s, e) = call(&server, a, org, "POST", "/api/v1/rules",
            Some(rule(json!({ "type_key": "work_order", "title": "x", "link_type": "nope" })))).await;
        assert!(s == 400 && e["errors"]["fields"].get("propose.link_type").is_some(), "{e}");
        let (s, r) = call(&server, a, org, "POST", "/api/v1/rules", Some(rule(propose))).await;
        assert_eq!(s, 201, "{r}");
        assert_eq!(r["propose"]["values"]["kind"], "inspection", "{r}");

        // First pass records; a new machine then fires and proposes a work order on it.
        pnex_backend::services::rules::pass(&ctx).await.unwrap();
        let (_, m) = call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(machine("P-2"))).await;
        let m = m["id"].as_str().unwrap().to_string();
        pnex_backend::services::rules::pass(&ctx).await.unwrap();
        let rows = sql(&ctx.db, "SELECT id::text AS id, origin, link_target_id, title FROM proposals \
            WHERE type_key = 'work_order' AND status = 'proposed'").await;
        assert_eq!(rows.len(), 1, "one proposal for the one new machine");
        let row = &rows[0];
        let get = |k: &str| row.try_get::<String>("", k).unwrap();
        assert_eq!((get("origin"), get("link_target_id"), get("title")),
            ("rule".into(), m.clone(), "Inspect the new machine".into()));

        // Nobody filed it: the rule's author accepts it like any member.
        let (s, v) = call(&server, a, org, "POST", &format!("/api/v1/proposals/{}/decision", get("id")),
            Some(json!({ "decision": "accept" }))).await;
        assert_eq!((s, v["status"].as_str()), (200, Some("accepted")), "{v}");
        let wo = v["applied"].as_str().unwrap();
        let links = sql(&ctx.db, &format!("SELECT target_object_id::text AS t FROM resource_edges \
            WHERE source_object_id = '{wo}' AND relation = 'work_order_on'")).await;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].try_get::<String>("", "t").unwrap(), m);
    })
    .await;
}
