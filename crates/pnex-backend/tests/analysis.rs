//! Read-only SQL (analysis.md D825–D828): what a query may read is fixed when it is
//! sent, under its author's rights and markings; the worker part is tested in
//! `pnex-pipeline-worker/tests/run.rs`.
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
async fn a_query_fixes_what_its_author_may_read() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "viewer").await;
        let a = env.alice.as_str();
        sql(&ctx.db, "DELETE FROM analysis_queries").await;
        let staff = table(&server, a, org, "staff").await;
        let secret = table(&server, a, org, "secret").await;
        call(&server, a, org, "POST", "/api/v1/governance/markings", Some(json!({ "key": "hr", "name": "HR" }))).await;
        call(&server, a, org, "POST", &format!("/api/v1/datasets/{secret}/markings"),
            Some(json!({ "markings": ["hr"] }))).await;
        let (s, t) = call(&server, a, org, "POST", "/api/v1/ontology/types", Some(json!({
            "def": { "key": "employee", "name": "Employee",
                     "properties": [{ "key": "salary", "name": "Salary", "kind": "text", "max_len": 16 }] } }))).await;
        assert_eq!(s, 201, "{t}");
        for (title, salary) in [("Ada", "5000"), ("Bob", "4000")] {
            call(&server, a, org, "POST", "/api/v1/ontology/objects",
                Some(json!({ "type_key": "employee", "title": title, "properties": { "salary": salary } }))).await;
        }
        let (_, hidden) = call(&server, a, org, "POST", "/api/v1/ontology/objects",
            Some(json!({ "type_key": "employee", "title": "Cy", "properties": { "salary": "9000" } }))).await;
        call(&server, a, org, "POST", &format!("/api/v1/ontology/objects/{}/markings", hidden["id"].as_str().unwrap()),
            Some(json!({ "markings": ["hr"] }))).await;

        // A viewer may not run queries.
        let body = json!({ "sql": "SELECT * FROM staff JOIN objects.employee e ON true" });
        let (s, _) = call(&server, &env.bob, org, "POST", "/api/v1/analysis/queries", Some(body.clone())).await;
        assert_eq!(s, 403);

        let (s, q) = call(&server, a, org, "POST", "/api/v1/analysis/queries", Some(body.clone())).await;
        assert_eq!((s, q["state"].as_str()), (202, Some("queued")), "{q}");
        let id = q["id"].as_str().unwrap().to_string();
        let row = &sql(&ctx.db, &format!("SELECT tables, refused, objects FROM analysis_queries WHERE id = '{id}'")).await[0];
        let tables: Value = row.try_get("", "tables").unwrap();
        let refused: Value = row.try_get("", "refused").unwrap();
        let objects: Value = row.try_get("", "objects").unwrap();
        assert_eq!(tables["staff"], json!(staff), "{tables}");
        assert!(tables.get("secret").is_none() && refused == json!(["secret"]), "{tables} {refused}");
        // The objects as alice sees them: the marked one is absent.
        let key = objects["employee"].as_str().unwrap();
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let bytes = store.get_bytes(key).await.unwrap().unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Ada") && text.contains("Bob") && !text.contains("Cy"), "frozen view");

        // One at a time; a query of someone else does not exist for them.
        let (s, e) = call(&server, a, org, "POST", "/api/v1/analysis/queries", Some(body.clone())).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("analysis-busy")), "{e}");
        // A live block asks for reuse: the same text answers the running query.
        let mut reuse = body.clone();
        reuse["max_age_secs"] = json!(300);
        let (s, again) = call(&server, a, org, "POST", "/api/v1/analysis/queries", Some(reuse.clone())).await;
        assert_eq!((s, again["id"].as_str()), (202, Some(id.as_str())), "{again}");
        // Done ten minutes ago: too old, a new run.
        sql(&ctx.db, &format!("UPDATE analysis_queries SET state = 'done', finished_at = now() - interval '10 minutes' WHERE id = '{id}'")).await;
        let (s, fresh) = call(&server, a, org, "POST", "/api/v1/analysis/queries", Some(reuse)).await;
        assert_eq!(s, 202, "{fresh}");
        assert_ne!(fresh["id"].as_str(), Some(id.as_str()), "not reused past 5 minutes");
        let (s, _) = call(&server, a, org, "GET", &format!("/api/v1/analysis/queries/{id}"), None).await;
        assert_eq!(s, 200);
        let (s, _) = call(&server, &env.bob, org, "GET", &format!("/api/v1/analysis/queries/{id}"), None).await;
        assert_eq!(s, 403, "a viewer has no analysis:run");
        let (s, e) = call(&server, a, org, "POST", "/api/v1/analysis/queries", Some(json!({ "sql": " " }))).await;
        assert_eq!(s, 400, "{e}");
        sql(&ctx.db, "DELETE FROM analysis_queries").await;
    })
    .await;
}
