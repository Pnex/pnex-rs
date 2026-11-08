//! Data governance (governance.md D798–D803): a marked table reads only with a
//! clearance for each marking, owner included; what is computed from it inherits
//! its markings; clearances end on revocation; declassification needs a reason.
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

async fn table(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let (s, d) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/datasets",
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(s, 201, "{d}");
    let id = d["id"].as_str().unwrap().to_string();
    let res = server
        .post(&format!("/api/v1/datasets/{id}/transactions/csv"))
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .text("name,salary\nAda,5000\nBob,4000\n".to_string())
        .await;
    assert!(res.status_code().is_success(), "{}", res.text());
    id
}

#[tokio::test]
#[serial]
async fn a_marked_table_reads_only_with_a_clearance() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_own = personal_org(&server, &env.bob).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "member").await;
        let (a, b) = (env.alice.as_str(), env.bob.as_str());
        let staff = table(&server, a, org, "staff").await;
        let sample = format!("/api/v1/datasets/{staff}/sample");

        let (s, m) = call(&server, a, org, "POST", "/api/v1/governance/markings",
            Some(json!({ "key": "hr", "name": "HR" }))).await;
        assert_eq!(s, 201, "{m}");
        let (s, _) = call(&server, b, org, "POST", "/api/v1/governance/markings",
            Some(json!({ "key": "x", "name": "X" }))).await;
        assert_eq!(s, 403, "a member does not set markings");
        let (s, d) = call(&server, a, org, "POST", &format!("/api/v1/datasets/{staff}/markings"),
            Some(json!({ "markings": ["hr"] }))).await;
        assert_eq!((s, d["markings"].clone()), (200, json!(["hr"])), "{d}");

        // Nobody reads it without a clearance, the owner neither.
        for t in [a, b] {
            let (s, e) = call(&server, t, org, "GET", &sample, None).await;
            assert_eq!((s, e["error"].as_str()), (403, Some("marking-required")), "{e}");
            assert_eq!(e["errors"]["args"]["markings"], json!(["hr"]), "{e}");
        }
        let (s, _) = call(&server, b, org, "GET", &format!("/api/v1/datasets/{staff}/profile"), None).await;
        assert_eq!(s, 403);
        // Its name and columns stay visible; markings are listed to anyone.
        let (s, d) = call(&server, b, org, "GET", &format!("/api/v1/datasets/{staff}"), None).await;
        assert_eq!((s, d["markings"].clone()), (200, json!(["hr"])));
        let (s, l) = call(&server, b, org, "GET", "/api/v1/governance/markings", None).await;
        assert_eq!((s, l["results"][0]["uses"].as_i64()), (200, Some(1)), "{l}");

        // A clearance for a purpose: bob reads; a stranger cannot be cleared.
        let bob_id: i64 = sql(&ctx.db, "SELECT id FROM users WHERE email = 'bob@example.com'").await[0]
            .try_get("", "id").unwrap();
        let (s, e) = call(&server, a, org, "POST", "/api/v1/governance/clearances",
            Some(json!({ "user_id": 999_999, "marking_key": "hr", "purpose": "payroll", "days": 30 }))).await;
        assert_eq!(s, 400, "{e}");
        let (s, c) = call(&server, a, org, "POST", "/api/v1/governance/clearances",
            Some(json!({ "user_id": bob_id, "marking_key": "hr", "purpose": "payroll 2026", "days": 30 }))).await;
        assert_eq!((s, c["active"].as_bool()), (201, Some(true)), "{c}");
        let (s, r) = call(&server, b, org, "GET", &sample, None).await;
        assert_eq!(s, 200, "{r}");
        assert!(r.to_string().contains("Ada"), "{r}");

        // From another org, the id names nothing: no marking leaks.
        let (s, e) = call(&server, b, bob_own, "GET", &sample, None).await;
        assert_eq!(s, 404, "{e}");

        // What a run computes from it inherits the marking (trigger on lineage).
        let derived = table(&server, a, org, "payroll").await;
        let p = sql(&ctx.db, &format!(
            "INSERT INTO pipelines (org_id, name, document) VALUES ({org}, 'p', '') RETURNING id")).await[0]
            .try_get::<uuid::Uuid>("", "id").unwrap();
        let tx = |d: &str| format!(
            "(SELECT id FROM dataset_transactions WHERE dataset_id = '{d}' AND state = 'committed' LIMIT 1)");
        sql(&ctx.db, &format!(
            "INSERT INTO dataset_transaction_inputs (transaction_id, input_transaction_id, pipeline_id, pipeline_version) \
             VALUES ({}, {}, '{p}', 1)", tx(&derived), tx(&staff))).await;
        let (_, d) = call(&server, a, org, "GET", &format!("/api/v1/datasets/{derived}"), None).await;
        assert_eq!(d["markings"], json!(["hr"]), "{d}");

        // Declassified only with a reason; a used marking is not deleted.
        let declassify = format!("/api/v1/datasets/{derived}/declassify");
        let (s, _) = call(&server, a, org, "POST", &declassify,
            Some(json!({ "marking": "hr", "justification": "no" }))).await;
        assert_eq!(s, 400);
        let (s, d) = call(&server, a, org, "POST", &declassify,
            Some(json!({ "marking": "hr", "justification": "salaries replaced by bands" }))).await;
        assert_eq!((s, d["markings"].clone()), (200, json!([])), "{d}");
        let mid = m["id"].as_str().unwrap();
        let (s, e) = call(&server, a, org, "DELETE", &format!("/api/v1/governance/markings/{mid}"), None).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("marking-in-use")), "{e}");

        // Revoked: bob is refused again at once.
        let cid = c["id"].as_str().unwrap();
        let (s, _) = call(&server, a, org, "POST", &format!("/api/v1/governance/clearances/{cid}/revoke"), None).await;
        assert_eq!(s, 204);
        let (s, _) = call(&server, b, org, "GET", &sample, None).await;
        assert_eq!(s, 403);
        let (_, list) = call(&server, a, org, "GET", "/api/v1/governance/clearances", None).await;
        assert_eq!(list["results"][0]["active"], false, "{list}");
    })
    .await;
}
