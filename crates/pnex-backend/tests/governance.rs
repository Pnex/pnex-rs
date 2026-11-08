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

#[tokio::test]
#[serial]
async fn a_marked_object_is_masked_and_cannot_be_written() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "member").await;
        let (a, b) = (env.alice.as_str(), env.bob.as_str());
        let (s, t) = call(&server, a, org, "POST", "/api/v1/ontology/types", Some(json!({
            "def": { "key": "employee", "name": "Employee",
                     "properties": [{ "key": "salary", "name": "Salary", "kind": "text", "max_len": 16 }] } }))).await;
        assert_eq!(s, 201, "{t}");
        let mk = |title: &str, salary: &str| json!({ "type_key": "employee", "title": title,
                                                    "properties": { "salary": salary } });
        let (_, ada) = call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(mk("Ada", "5000"))).await;
        let (_, joe) = call(&server, a, org, "POST", "/api/v1/ontology/objects", Some(mk("Joe", "3000"))).await;
        let ada_id = ada["id"].as_str().unwrap().to_string();
        call(&server, a, org, "POST", "/api/v1/governance/markings",
            Some(json!({ "key": "hr", "name": "HR" }))).await;
        let (s, m) = call(&server, a, org, "POST", &format!("/api/v1/ontology/objects/{ada_id}/markings"),
            Some(json!({ "markings": ["hr"] }))).await;
        assert_eq!((s, m["markings"].clone()), (200, json!(["hr"])), "{m}");

        // Masked whole for everyone not cleared, owner included.
        let one = format!("/api/v1/ontology/objects/{ada_id}");
        for t in [a, b] {
            let (s, o) = call(&server, t, org, "GET", &one, None).await;
            assert_eq!(s, 200, "{o}");
            assert!(o["title"].as_str().unwrap().starts_with("••••"), "{o}");
            assert!(o["properties"].as_object().unwrap().is_empty(), "{o}");
        }
        // A query neither lists it nor counts it: a filter on salary would tell.
        let (s, q) = call(&server, b, org, "POST", "/api/v1/ontology/query",
            Some(json!({ "type_key": "employee" }))).await;
        assert_eq!(s, 200, "{q}");
        assert_eq!(q["count"], 1, "{q}");
        assert_eq!(q["rows"][0]["object"]["id"], joe["id"], "{q}");
        // No write on what one cannot read.
        let (s, e) = call(&server, b, org, "DELETE", &one, None).await;
        assert_eq!((s, e["error"].as_str()), (403, Some("marking-required")), "{e}");

        // Cleared, bob reads it as it is.
        let bob_id: i64 = sql(&ctx.db, "SELECT id FROM users WHERE email = 'bob@example.com'").await[0]
            .try_get("", "id").unwrap();
        call(&server, a, org, "POST", "/api/v1/governance/clearances",
            Some(json!({ "user_id": bob_id, "marking_key": "hr", "purpose": "payroll", "days": 1 }))).await;
        let (_, o) = call(&server, b, org, "GET", &one, None).await;
        assert_eq!((o["title"].as_str(), o["properties"]["salary"].as_str()), (Some("Ada"), Some("5000")), "{o}");
        let (_, q) = call(&server, b, org, "POST", "/api/v1/ontology/query",
            Some(json!({ "type_key": "employee" }))).await;
        assert_eq!(q["count"], 2, "{q}");

        // Declassified with a reason: alice reads it too.
        let (s, m) = call(&server, a, org, "POST", &format!("/api/v1/ontology/objects/{ada_id}/declassify"),
            Some(json!({ "marking": "hr", "justification": "salary moved to the payroll table" }))).await;
        assert_eq!((s, m["markings"].clone()), (200, json!([])), "{m}");
        let (_, o) = call(&server, a, org, "GET", &one, None).await;
        assert_eq!(o["title"], "Ada", "{o}");
    })
    .await;
}

async fn run_import(server: &axum_test::TestServer, token: &str, org: i64, import: &str) -> Value {
    let (s, r) = call(
        server,
        token,
        org,
        "POST",
        &format!("/api/v1/ontology/imports/{import}/runs"),
        None,
    )
    .await;
    assert_eq!(s, 202, "{r}");
    let run = r["id"].as_str().unwrap().to_string();
    for _ in 0..300 {
        let (_, v) = call(
            server,
            token,
            org,
            "GET",
            &format!("/api/v1/ontology/imports/{import}/runs/{run}"),
            None,
        )
        .await;
        if v["run"]["state"] != "running" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("run {run} never settled");
}

#[tokio::test]
#[serial]
async fn a_sync_needs_a_cleared_author_and_marks_what_it_writes() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = env.alice.as_str();
        let (s, t) = call(&server, a, org, "POST", "/api/v1/ontology/types", Some(json!({
            "def": { "key": "employee", "name": "Employee",
                     "properties": [{ "key": "code", "name": "Code", "kind": "text", "max_len": 16,
                                      "unique": true }] } }))).await;
        assert_eq!(s, 201, "{t}");
        let (s, d) = call(&server, a, org, "POST", "/api/v1/datasets", Some(json!({ "name": "staff" }))).await;
        assert_eq!(s, 201, "{d}");
        let staff = d["id"].as_str().unwrap().to_string();
        let res = server
            .post(&format!("/api/v1/datasets/{staff}/transactions/csv"))
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .text("code,name\nA1,Ada\nB2,Bob\n".to_string())
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        call(&server, a, org, "POST", "/api/v1/governance/markings", Some(json!({ "key": "hr", "name": "HR" }))).await;
        call(&server, a, org, "POST", &format!("/api/v1/datasets/{staff}/markings"),
            Some(json!({ "markings": ["hr"] }))).await;

        let mapping = "key: staff_sync\nname: Staff\nformat: csv\nobjects:\n  - as: e\n    type: employee\n    key: code\n    title: { from: name }\n    properties:\n      code: { from: code }\n";
        let (s, imp) = call(&server, a, org, "POST", "/api/v1/ontology/imports", Some(json!({
            "name": "staff", "mapping": mapping, "source": "dataset", "source_url": staff,
            "mode": "snapshot" }))).await;
        assert_eq!(s, 201, "{imp}");
        let id = imp["id"].as_str().unwrap().to_string();

        // Its author is not cleared: the run reads nothing.
        let r = run_import(&server, a, org, &id).await;
        assert_eq!(r["run"]["state_code"], "marking-required", "{r}");

        let alice: i64 = sql(&ctx.db, "SELECT id FROM users WHERE email = 'alice@example.com'").await[0]
            .try_get("", "id").unwrap();
        call(&server, a, org, "POST", "/api/v1/governance/clearances",
            Some(json!({ "user_id": alice, "marking_key": "hr", "purpose": "staff sync", "days": 1 }))).await;
        let r = run_import(&server, a, org, &id).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let marked: i64 = sql(&ctx.db, &format!(
            "SELECT count(*) AS n FROM object_markings om JOIN objects o ON o.id = om.object_id \
             WHERE o.org_id = {org} AND o.type_key = 'employee'")).await[0]
            .try_get("", "n").unwrap();
        assert_eq!(marked, 2, "what the sync wrote carries the table's marking: {r}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_journal_exports_to_the_admin_only() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "member",
        )
        .await;
        let (s, _) = call(
            &server,
            &env.bob,
            org,
            "GET",
            "/api/v1/governance/journal",
            None,
        )
        .await;
        assert_eq!(s, 403);
        let (s, e) = call(
            &server,
            &env.alice,
            org,
            "GET",
            "/api/v1/governance/journal?from=yesterday",
            None,
        )
        .await;
        assert_eq!(s, 400, "{e}");
        let res = server
            .get("/api/v1/governance/journal")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let status = res.status_code().as_u16();
        let text = res.text();
        // OpenObserve may be absent where the tests run: then a coded refusal.
        match status {
            200 => assert!(text.starts_with("_timestamp,action,user_id"), "{text}"),
            503 => assert!(text.contains("governance-journal-unavailable"), "{text}"),
            other => panic!("{other}: {text}"),
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn personal_data_in_a_table_is_proposed_not_marked() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "member",
        )
        .await;
        let (a, b) = (env.alice.as_str(), env.bob.as_str());
        let (_, d) = call(
            &server,
            a,
            org,
            "POST",
            "/api/v1/datasets",
            Some(json!({ "name": "contacts" })),
        )
        .await;
        let id = d["id"].as_str().unwrap().to_string();
        let csv = "name,email,city\nAda,ada@example.org,Lyon\nBob,bob@example.org,Nantes\n\
                   Cy,cy@example.net,Brest\nDi,di@example.com,Paris\nEd,ed@example.fr,Lille\n";
        let res = server
            .post(&format!("/api/v1/datasets/{id}/transactions/csv"))
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .text(csv.to_string())
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());

        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let n = pnex_backend::services::governance::detect_pass(&ctx.db, &store)
            .await
            .unwrap();
        assert!(n >= 1, "one column of e-mails");
        // Read once: a second pass on the same version proposes nothing.
        let again = pnex_backend::services::governance::detect_pass(&ctx.db, &store)
            .await
            .unwrap();
        assert_eq!(again, 0);

        // Nothing changed by itself.
        let (_, t) = call(
            &server,
            a,
            org,
            "GET",
            &format!("/api/v1/datasets/{id}"),
            None,
        )
        .await;
        assert_eq!(t["markings"], json!([]), "{t}");
        let (_, l) = call(
            &server,
            a,
            org,
            "GET",
            "/api/v1/proposals?status=proposed",
            None,
        )
        .await;
        let p = l["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["kind"] == "dataset_mark")
            .cloned()
            .expect("a proposal");
        assert_eq!(
            (
                p["values"]["column"].as_str(),
                p["values"]["category"].as_str()
            ),
            (Some("email"), Some("email")),
            "{p}"
        );
        let pid = p["id"].as_str().unwrap();
        let decide = format!("/api/v1/proposals/{pid}/decision");

        // A member may decide proposals, not set a marking.
        let (s, e) = call(
            &server,
            b,
            org,
            "POST",
            &decide,
            Some(json!({ "decision": "accept" })),
        )
        .await;
        assert_eq!(s, 403, "{e}");
        let (s, v) = call(
            &server,
            a,
            org,
            "POST",
            &decide,
            Some(json!({ "decision": "accept" })),
        )
        .await;
        assert_eq!((s, v["status"].as_str()), (200, Some("accepted")), "{v}");
        let (_, t) = call(
            &server,
            a,
            org,
            "GET",
            &format!("/api/v1/datasets/{id}"),
            None,
        )
        .await;
        assert_eq!(t["markings"], json!(["personal"]), "{t}");
        let (s, _) = call(
            &server,
            a,
            org,
            "GET",
            &format!("/api/v1/datasets/{id}/sample"),
            None,
        )
        .await;
        assert_eq!(s, 403, "marked now: a clearance is needed");
    })
    .await;
}
