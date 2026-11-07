//! Connectors (data-pipelines.md D352): an import reads an outside database into a
//! table, read-only, with the password in the vault and rights checked on each run.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL, which also plays the outside database) and the
//! compose `rustfs` service.

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
            pnex_backend::services::markets::init(&ctx);
            f(server, env, ctx).await;
        },
    )
    .await;
}

struct Api<'a> {
    server: &'a axum_test::TestServer,
    token: &'a str,
    org: i64,
}

impl Api<'_> {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let req = match method {
            "GET" => self.server.get(path),
            "POST" => self.server.post(path),
            "PUT" => self.server.put(path),
            "DELETE" => self.server.delete(path),
            _ => unreachable!(),
        }
        .add_header("Authorization", format!("Bearer {}", self.token))
        .add_header("X-Org-Id", self.org.to_string());
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

    async fn run(&self, import: &str) -> Value {
        let (s, r) = self
            .call(
                "POST",
                &format!("/api/v1/ontology/imports/{import}/runs"),
                None,
            )
            .await;
        assert_eq!(s, 202, "{r}");
        let run = r["id"].as_str().unwrap().to_string();
        for _ in 0..300 {
            let (_, v) = self
                .call(
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
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .unwrap()
}

async fn sql(db: &DatabaseConnection, text: &str) -> Vec<QueryResult> {
    db.query_all_raw(Statement::from_string(DatabaseBackend::Postgres, text))
        .await
        .unwrap()
}

/// The test database, seen as an outside one.
fn outside() -> (String, i32, String, String, String) {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string());
    let u = reqwest::Url::parse(&url).unwrap();
    (
        u.host_str().unwrap().to_string(),
        u.port().unwrap_or(5432) as i32,
        u.path().trim_start_matches('/').to_string(),
        u.username().to_string(),
        u.password().unwrap_or_default().to_string(),
    )
}

fn connector_body(name: &str) -> Value {
    let (host, port, database, username, password) = outside();
    json!({ "name": name, "kind": "postgres", "host": host, "port": port, "database": database,
            "username": username, "tls": false, "password": { "value": password } })
}

fn mapping(query: &str) -> String {
    format!(
        "key: suppliers_table\nname: Suppliers\nformat: csv\nsteps:\n  - step: connector_query\n    connector: erp\n    query: {query:?}\nobjects:\n  - as: org\n    type: certification\n    key: code\n    title: {{ from: name }}\n    properties:\n      code: {{ from: code }}\n"
    )
}

async fn install_pack(api: &Api<'_>) {
    let (s, e) = api
        .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
        .await;
    assert_eq!(s, 200, "{e}");
}

#[tokio::test]
#[serial]
async fn an_import_reads_an_outside_database_into_a_table() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        sql(db, "DROP TABLE IF EXISTS outside_suppliers").await;
        sql(
            db,
            "CREATE TABLE outside_suppliers (code text, name text, note text)",
        )
        .await;
        sql(
            db,
            "INSERT INTO outside_suppliers VALUES ('A1', 'Acme, Inc', 'say \"hi\"'), ('B2', 'Beta', NULL)",
        )
        .await;
        install_pack(&api).await;

        // Created, password held in the vault and shown nowhere.
        let (s, c) = api
            .call("POST", "/api/v1/connectors", Some(connector_body("erp")))
            .await;
        assert_eq!(s, 201, "{c}");
        assert!(c.get("password").is_none() && c.get("secret_id").is_none(), "{c}");
        assert_eq!(c["has_password"], true);
        let id = c["id"].as_str().unwrap().to_string();
        let (s, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!((s, t["ok"].as_bool()), (200, Some(true)), "{t}");

        let import = |q: &str| {
            json!({ "name": "suppliers", "mapping": mapping(q), "source": "file",
                    "mode": "incremental" })
        };
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(import("SELECT code, name, note FROM outside_suppliers ORDER BY code")),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["counts"]["created"], 2, "{r}");
        // No host or user in the journal.
        let steps = r["steps"].to_string();
        assert!(steps.contains("connector_query"), "{steps}");
        assert!(!steps.contains(&outside().0), "{steps}");

        // The table holds the rows, quoting intact, and a pipeline could read it.
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "suppliers_table")
            .expect("the table")
            .clone();
        assert_eq!(table["rows"], 2, "{table}");
        let (s, sample) = api
            .call(
                "GET",
                &format!("/api/v1/datasets/{}/sample", table["id"].as_str().unwrap()),
                None,
            )
            .await;
        assert_eq!(s, 200, "{sample}");
        assert_eq!(sample["rows"][0][1], "Acme, Inc", "{sample}");
        assert_eq!(sample["rows"][0][2], "say \"hi\"", "{sample}");

        // Read-only, one statement, and nothing can get out of the parenthesis.
        let update = |q: &str| {
            let mut body = import(q);
            body["expected_version"] = json!(1);
            body
        };
        for hostile in [
            "DELETE FROM outside_suppliers RETURNING *",
            "UPDATE outside_suppliers SET name = 'x' RETURNING *",
            "SELECT 1; DELETE FROM outside_suppliers",
            "SELECT 1) TO STDOUT; DELETE FROM outside_suppliers; COPY (SELECT 1",
            "SELECT 1) TO STDOUT; COMMIT; DELETE FROM outside_suppliers; COPY (SELECT 1",
            "DROP TABLE outside_suppliers",
            "COPY outside_suppliers TO PROGRAM 'true'",
            "SELECT pg_read_file('/etc/passwd') /*",
        ] {
            let (_, read) = api
                .call("GET", &format!("/api/v1/ontology/imports/{imp}"), None)
                .await;
            let mut body = update(hostile);
            body["expected_version"] = read["version"].clone();
            let (s, e) = api
                .call("PUT", &format!("/api/v1/ontology/imports/{imp}"), Some(body))
                .await;
            assert_eq!(s, 200, "{e}");
            let r = api.run(&imp).await;
            assert_eq!(r["run"]["state"], "failed", "{hostile}: {r}");
            assert_eq!(r["run"]["state_code"], "connector-query-invalid", "{hostile}: {r}");
        }
        let left: i64 = sql(db, "SELECT count(*) AS n FROM outside_suppliers").await[0]
            .try_get("", "n")
            .unwrap();
        assert_eq!(left, 2, "nothing was written");

        // A connector of nobody, a wrong password: a code, never the server's text.
        let (s, c2) = api
            .call(
                "POST",
                "/api/v1/connectors",
                Some({
                    let mut b = connector_body("broken");
                    b["password"] = json!({ "value": "wrong" });
                    b["port"] = json!(1);
                    b
                }),
            )
            .await;
        assert_eq!(s, 201, "{c2}");
        let (_, t) = api
            .call(
                "POST",
                &format!("/api/v1/connectors/{}/test", c2["id"].as_str().unwrap()),
                None,
            )
            .await;
        assert_eq!(t["ok"], false, "{t}");
        assert_eq!(t["code"], "connector-unreachable", "{t}");

        // Moved elsewhere, the stored password does not follow.
        let mut moved = connector_body("erp");
        moved["host"] = json!("other.example.org");
        moved["expected_version"] = json!(1);
        moved.as_object_mut().unwrap().remove("password");
        let (s, e) = api
            .call("PUT", &format!("/api/v1/connectors/{id}"), Some(moved))
            .await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["password"], "required", "{e}");

        let (s, _) = api
            .call("DELETE", &format!("/api/v1/connectors/{id}"), None)
            .await;
        assert_eq!(s, 204);
        let (_, secrets) = api.call("GET", "/api/v1/secrets", None).await;
        assert!(
            !secrets.to_string().contains("connector/erp/"),
            "its secret went with it: {secrets}"
        );
        sql(db, "DROP TABLE outside_suppliers").await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn connectors_are_for_admins_of_the_org() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let alice = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let bob_outside = Api {
            server: &server,
            token: &env.bob,
            org: bob_org,
        };
        let bob = Api {
            server: &server,
            token: &env.bob,
            org,
        };
        let (s, c) = alice
            .call("POST", "/api/v1/connectors", Some(connector_body("erp")))
            .await;
        assert_eq!(s, 201, "{c}");
        let id = c["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/connectors/{id}");

        // Another org: it does not exist, and its name is free there.
        for (m, p) in [
            ("GET", path.clone()),
            ("POST", format!("{path}/test")),
            ("DELETE", path.clone()),
        ] {
            let (s, e) = bob_outside.call(m, &p, None).await;
            assert_eq!(
                (s, e["error"].as_str()),
                (404, Some("connector-not-found")),
                "{m} {p}"
            );
        }
        let (s, e) = bob_outside
            .call("POST", "/api/v1/connectors", Some(connector_body("erp")))
            .await;
        assert_eq!(s, 201, "{e}");

        // A member: no connector, and no import reading one.
        common::join_org(
            &server,
            alice.token,
            &env.bob,
            org,
            "bob@example.com",
            "member",
        )
        .await;
        for (method, p, body) in [
            ("GET", "/api/v1/connectors".to_string(), None),
            ("GET", path.clone(), None),
            ("POST", format!("{path}/test"), None),
            (
                "POST",
                "/api/v1/connectors".to_string(),
                Some(connector_body("x")),
            ),
            ("PUT", path.clone(), Some(connector_body("x"))),
            ("DELETE", path.clone(), None),
        ] {
            let (s, e) = bob.call(method, &p, body).await;
            assert_eq!(
                (s, e["error"].as_str()),
                (403, Some("access-denied")),
                "{method} {p}"
            );
        }
        install_pack(&alice).await;
        let body = |q: &str| {
            json!({ "name": "suppliers", "mapping": mapping(q), "source": "file",
                    "mode": "incremental" })
        };
        let (s, e) = bob
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(body("SELECT 1 AS code, 2 AS name")),
            )
            .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (403, Some("access-denied")),
            "{e}"
        );

        // An admin writes it, then loses the right: the next run reads nothing.
        let (s, m) = alice
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(body("SELECT 'a' AS code, 'b' AS name")),
            )
            .await;
        assert_eq!(s, 201, "{m}");
        let imp = m["id"].as_str().unwrap().to_string();
        let r = alice.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let bob_id: i64 = sql(
            &ctx.db,
            "SELECT id FROM users WHERE email = 'bob@example.com'",
        )
        .await[0]
            .try_get("", "id")
            .unwrap();
        sql(
            &ctx.db,
            &format!("UPDATE object_imports SET created_by = {bob_id} WHERE id = '{imp}'"),
        )
        .await;
        let r = alice.run(&imp).await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        assert_eq!(r["run"]["state_code"], "connector-forbidden", "{r}");

        // The private hosts belong to the platform admin only.
        let hosts = format!("/api/v1/system/orgs/{org}/connector-hosts");
        let (s, _) = bob_outside.call("GET", &hosts, None).await;
        assert_eq!(s, 403);
        let (s, _) = bob_outside
            .call("PUT", &hosts, Some(json!({ "hosts": ["db.lan"] })))
            .await;
        assert_eq!(s, 403);
    })
    .await;
}

/// A paged JSON API: bearer `tok`, three records over two pages, a third page empty,
/// and a route whose next link leaves for another host.
async fn spawn_api() -> u16 {
    use axum::extract::Query;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use std::collections::HashMap;

    fn authorized(h: &HeaderMap) -> bool {
        h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer tok")
    }
    let app = axum::Router::new()
        .route(
            "/api",
            get(|h: HeaderMap| async move {
                if authorized(&h) {
                    StatusCode::OK
                } else {
                    StatusCode::UNAUTHORIZED
                }
            }),
        )
        .route(
            "/api/items",
            get(
                |h: HeaderMap, Query(q): Query<HashMap<String, String>>| async move {
                    if !authorized(&h) {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    let data = match q.get("page").map(String::as_str) {
                        Some("1") => json!([{ "code": "A1", "name": "Acme, Inc" },
                                            { "code": "B2", "name": "Beta" }]),
                        Some("2") => json!([{ "code": "C3", "name": "Gamma" }]),
                        _ => json!([]),
                    };
                    axum::Json(json!({ "data": data })).into_response()
                },
            ),
        )
        .route(
            "/api/away",
            get(|| async {
                (
                    [(
                        "link",
                        "<http://elsewhere.example.org/api/away?p=2>; rel=\"next\"",
                    )],
                    axum::Json(json!({ "data": [{ "code": "Z9", "name": "Zeta" }] })),
                )
            }),
        )
        .route("/api/html", get(|| async { "<html></html>" }))
        .route(
            "/api/feed",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let since: i64 = q.get("since").and_then(|v| v.parse().ok()).unwrap_or(0);
                let all = [("A1", "Acme", 1), ("B2", "Beta", 2), ("C3", "Gamma", 3)];
                let data: Vec<Value> = all
                    .iter()
                    .filter(|(_, _, v)| *v > since)
                    .map(|(c, n, v)| json!({ "code": c, "name": n, "v": v }))
                    .collect();
                axum::Json(json!({ "data": data }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

fn rest_mapping(path: &str, paging: &str) -> String {
    format!(
        "key: api_table\nname: Api\nformat: csv\nsteps:\n  - step: connector_query\n    connector: api\n    rest:\n      path: {path}\n      records: /data\n{paging}objects:\n  - as: org\n    type: certification\n    key: code\n    title: {{ from: name }}\n    properties:\n      code: {{ from: code }}\n"
    )
}

#[tokio::test]
#[serial]
async fn an_import_reads_a_paged_rest_api_into_a_table() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        install_pack(&api).await;
        let port = spawn_api().await;
        let body = |secret: &str| {
            json!({ "name": "api", "kind": "rest", "host": "127.0.0.1", "port": port,
                    "tls": false, "rest": { "base_path": "/api", "auth": "bearer" },
                    "password": { "value": secret } })
        };

        // Settings come back, the token never.
        let (s, c) = api
            .call("POST", "/api/v1/connectors", Some(body("tok")))
            .await;
        assert_eq!(s, 201, "{c}");
        assert_eq!(c["kind"], "rest", "{c}");
        assert_eq!(c["rest"]["base_path"], "/api", "{c}");
        assert!(!c.to_string().contains("tok\""), "{c}");
        let id = c["id"].as_str().unwrap().to_string();
        let (_, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!(t["ok"], true, "{t}");

        let paging = "      paging: page\n      page_param: page\n";
        let import = |path: &str, paging: &str| {
            json!({ "name": "api", "mapping": rest_mapping(path, paging), "source": "file",
                    "mode": "incremental" })
        };
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(import("/items", paging)),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["counts"]["created"], 3, "two pages read: {r}");
        assert!(!r["steps"].to_string().contains("127.0.0.1"), "{r}");

        // Each failure is a code: another host, not JSON, no records, a page limit.
        let mut version = 1;
        for (path, paging, code) in [
            (
                "/away",
                "      paging: link\n",
                "connector-rest-host-changed",
            ),
            ("/html", "", "connector-rest-not-json"),
            ("/", "", "connector-rest-status"),
            (
                "/items",
                "      paging: page\n      page_param: page\n      max_pages: 1\n",
                "connector-rest-page-limit",
            ),
        ] {
            let mut b = import(path, paging);
            b["expected_version"] = json!(version);
            let (s, e) = api
                .call("PUT", &format!("/api/v1/ontology/imports/{imp}"), Some(b))
                .await;
            assert_eq!(s, 200, "{e}");
            version = e["version"].as_i64().unwrap();
            let r = api.run(&imp).await;
            assert_eq!(r["run"]["state"], "failed", "{path}: {r}");
            assert_eq!(r["run"]["state_code"], code, "{path}: {r}");
        }

        // A wrong token: refused, and the test says so without the server's text.
        let mut wrong = body("nope");
        wrong["expected_version"] = json!(1);
        let (s, e) = api
            .call("PUT", &format!("/api/v1/connectors/{id}"), Some(wrong))
            .await;
        assert_eq!(s, 200, "{e}");
        let (_, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!(t["code"], "connector-rest-denied", "{t}");

        // A base path that smuggles a host is refused before anything is stored.
        let mut bad = body("tok");
        bad["name"] = json!("bad");
        bad["rest"]["base_path"] = json!("//evil.example.org/x");
        let (s, e) = api.call("POST", "/api/v1/connectors", Some(bad)).await;
        assert_eq!(s, 400, "{e}");
    })
    .await;
}

fn table_rows(list: &Value, name: &str) -> i64 {
    list["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == name)
        .map(|d| d["rows"].as_i64().unwrap_or(0))
        .unwrap_or(-1)
}

#[tokio::test]
#[serial]
async fn a_cursor_reads_only_what_is_new() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        sql(db, "DROP TABLE IF EXISTS outside_events").await;
        sql(
            db,
            "CREATE TABLE outside_events (code text, name text, updated int)",
        )
        .await;
        sql(
            db,
            "INSERT INTO outside_events VALUES ('A1', 'Acme', 1), ('B2', 'Beta', 2)",
        )
        .await;
        install_pack(&api).await;
        let (s, c) = api
            .call("POST", "/api/v1/connectors", Some(connector_body("erp")))
            .await;
        assert_eq!(s, 201, "{c}");
        let query = "SELECT code, name, updated FROM outside_events WHERE updated > {{cursor}}";
        let mapping = mapping(query).replace(
            "    query:",
            "    cursor:\n      column: updated\n      start: '0'\n    query:",
        );
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(
                    json!({ "name": "events", "mapping": mapping, "source": "file",
                             "mode": "incremental" }),
                ),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let cursor = || async {
            let rows = sql(
                db,
                &format!("SELECT cursor, cursor_runs FROM object_imports WHERE id = '{imp}'"),
            )
            .await;
            (
                rows[0].try_get::<Option<String>>("", "cursor").unwrap(),
                rows[0].try_get::<i32>("", "cursor_runs").unwrap(),
            )
        };

        // First run: everything, from the start value.
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["counts"]["created"], 2, "{r}");
        assert_eq!(cursor().await, (Some("2".into()), 0));

        // Second run: only the new row and the changed one, appended to the table.
        sql(db, "INSERT INTO outside_events VALUES ('C3', 'Gamma', 3)").await;
        sql(
            db,
            "UPDATE outside_events SET name = 'Beta two', updated = 4 WHERE code = 'B2'",
        )
        .await;
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["counts"]["created"], 1, "{r}");
        assert_eq!(r["run"]["counts"]["updated"], 1, "{r}");
        assert_eq!(cursor().await, (Some("4".into()), 1));
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        assert_eq!(table_rows(&list, "suppliers_table"), 4, "{list}");

        // Third run: nothing new, no empty version, the cursor stays.
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(cursor().await, (Some("4".into()), 1));
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        assert_eq!(table_rows(&list, "suppliers_table"), 4, "{list}");

        // A cursor value from the source is quoted by the source, never pasted.
        sql(
            db,
            &format!("UPDATE object_imports SET cursor = '0 OR true; --' WHERE id = '{imp}'"),
        )
        .await;
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        assert_eq!(r["run"]["state_code"], "connector-query-invalid", "{r}");
        sql(db, "DROP TABLE outside_events").await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_rest_cursor_sends_the_last_value() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        install_pack(&api).await;
        let port = spawn_api().await;
        let (s, c) = api
            .call(
                "POST",
                "/api/v1/connectors",
                Some(
                    json!({ "name": "api", "kind": "rest", "host": "127.0.0.1", "port": port,
                             "tls": false, "rest": { "base_path": "/api" } }),
                ),
            )
            .await;
        assert_eq!(s, 201, "{c}");
        let mapping = rest_mapping("/feed", "").replace(
            "    rest:",
            "    cursor:\n      column: /v\n      param: since\n    rest:",
        );
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(
                    json!({ "name": "feed", "mapping": mapping, "source": "file",
                             "mode": "incremental" }),
                ),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["counts"]["created"], 3, "{r}");
        // Without `since=3` the API would send the three records again.
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        assert_eq!(table_rows(&list, "api_table"), 3, "{list}");
    })
    .await;
}

fn signature(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

async fn call_hook(
    server: &axum_test::TestServer,
    id: &str,
    signature: Option<String>,
    body: &str,
) -> (u16, Value) {
    let mut req = server
        .post(&format!("/api/v1/hooks/{id}"))
        .content_type("application/json");
    if let Some(s) = signature {
        req = req.add_header("X-Signature", s);
    }
    let res = req.bytes(body.to_string().into()).await;
    let status = res.status_code().as_u16();
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

#[tokio::test]
#[serial]
async fn a_webhook_keeps_signed_calls_for_an_import() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        install_pack(&api).await;
        let (s, c) = api
            .call(
                "POST",
                "/api/v1/connectors",
                Some(json!({ "name": "hook", "kind": "webhook" })),
            )
            .await;
        assert_eq!(s, 201, "{c}");
        assert_eq!(c["webhook"]["signature_header"], "X-Signature", "{c}");
        let id = c["id"].as_str().unwrap().to_string();
        let (_, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!(t["code"], "hook-secret-missing", "{t}");

        // No secret yet: nothing is kept.
        let one = r#"{"code":"A1","name":"Acme"}"#;
        let (s, e) = call_hook(&server, &id, Some(signature("x", one.as_bytes())), one).await;
        assert_eq!((s, e["error"].as_str()), (401, Some("hook-signature-invalid")), "{e}");

        let (s, sec) = api
            .call("POST", &format!("/api/v1/connectors/{id}/secret"), None)
            .await;
        assert_eq!(s, 200, "{sec}");
        let secret = sec["secret"].as_str().unwrap().to_string();
        assert_eq!(secret.len(), 64);
        let (_, c) = api.call("GET", &format!("/api/v1/connectors/{id}"), None).await;
        assert!(!c.to_string().contains(&secret), "never read back: {c}");
        let (_, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!(t["ok"], true, "{t}");

        // Unsigned, wrongly signed, unknown: one answer.
        let unknown = uuid::Uuid::new_v4().to_string();
        for (target, sig) in [
            (id.as_str(), None),
            (id.as_str(), Some(signature("wrong", one.as_bytes()))),
            (id.as_str(), Some("sha256=zz".to_string())),
            (unknown.as_str(), Some(signature(&secret, one.as_bytes()))),
            ("not-a-uuid", Some(signature(&secret, one.as_bytes()))),
        ] {
            let (s, e) = call_hook(&server, target, sig, one).await;
            assert_eq!((s, e["error"].as_str()), (401, Some("hook-signature-invalid")), "{e}");
        }
        let (s, e) = call_hook(&server, &id, Some(signature(&secret, b"nope")), "nope").await;
        assert_eq!((s, e["error"].as_str()), (400, Some("hook-body-invalid")), "{e}");

        let two = r#"[{"code":"B2","name":"Beta"},{"code":"C3","name":"Gamma"}]"#;
        for body in [one, two] {
            let (s, e) = call_hook(&server, &id, Some(signature(&secret, body.as_bytes())), body)
                .await;
            assert_eq!(s, 202, "{e}");
        }
        let kept = sql(
            &ctx.db,
            &format!("SELECT count(*) AS n FROM connector_inbox WHERE org_id = {org}"),
        )
        .await[0]
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(kept, 2, "kept in the connector's org");

        let mapping = "key: hook_table\nname: Hook\nformat: csv\nsteps:\n  - step: connector_query\n    connector: hook\nobjects:\n  - as: org\n    type: certification\n    key: code\n    title: { from: name }\n    properties:\n      code: { from: code }\n";
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(json!({ "name": "hook", "mapping": mapping, "source": "file",
                             "mode": "incremental" })),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["counts"]["created"], 3, "{r}");

        // A later call is the newer state of its object; only it is read.
        let again = r#"{"code":"A1","name":"Acme two"}"#;
        let (s, _) = call_hook(&server, &id, Some(signature(&secret, again.as_bytes())), again)
            .await;
        assert_eq!(s, 202);
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["counts"]["updated"], 1, "{r}");
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        assert_eq!(table_rows(&list, "hook_table"), 4, "{list}");

        // A new secret: the old one stops at once.
        let (_, _) = api
            .call("POST", &format!("/api/v1/connectors/{id}/secret"), None)
            .await;
        let (s, _) = call_hook(&server, &id, Some(signature(&secret, one.as_bytes())), one).await;
        assert_eq!(s, 401);
    })
    .await;
}

/// Needs a MySQL server with TLS (any MySQL 8 generates its certificates):
/// `docker run -d -p 127.0.0.1:33306:3306 -e MYSQL_ROOT_PASSWORD=pnex
/// -e MYSQL_DATABASE=outside -e MYSQL_USER=reader -e MYSQL_PASSWORD=reader mysql:8.4`,
/// then `TEST_MYSQL_URL=mysql://root:pnex@127.0.0.1:33306/outside`.
#[tokio::test]
#[serial]
#[ignore = "needs a MySQL server: TEST_MYSQL_URL"]
async fn an_import_reads_a_mysql_database_under_tls() {
    use sqlx::Connection;
    let url = std::env::var("TEST_MYSQL_URL").expect("TEST_MYSQL_URL");
    let mut root = sqlx::MySqlConnection::connect(&url).await.unwrap();
    for q in [
        "DROP TABLE IF EXISTS suppliers",
        "CREATE TABLE suppliers (code varchar(10), name varchar(50), updated int, price decimal(6,2))",
        "INSERT INTO suppliers VALUES ('A1', 'Acme, Inc', 1, 9.50), ('B2', NULL, 2, 3.00)",
        "GRANT SELECT, INSERT, UPDATE, DELETE, DROP ON outside.* TO 'reader'@'%'",
    ] {
        sqlx::raw_sql(q).execute(&mut root).await.unwrap();
    }
    let u = reqwest::Url::parse(&url).unwrap();
    let port = u.port().unwrap();
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        install_pack(&api).await;
        let body = json!({ "name": "erp", "kind": "mysql", "host": "127.0.0.1", "port": port,
                           "database": "outside", "username": "reader", "tls": true,
                           "password": { "value": "reader" } });
        let mut clear = body.clone();
        clear["tls"] = json!(false);
        let (s, e) = api.call("POST", "/api/v1/connectors", Some(clear)).await;
        assert_eq!(s, 400, "TLS is required: {e}");
        let (s, c) = api.call("POST", "/api/v1/connectors", Some(body)).await;
        assert_eq!(s, 201, "{c}");
        let id = c["id"].as_str().unwrap().to_string();
        let (_, t) = api
            .call("POST", &format!("/api/v1/connectors/{id}/test"), None)
            .await;
        assert_eq!(t["ok"], true, "{t}");

        let import = |q: &str| {
            json!({ "name": "suppliers", "mapping": mapping(q), "source": "file",
                    "mode": "incremental" })
        };
        let (s, i) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(import("SELECT code, COALESCE(name, code) AS name, price FROM suppliers ORDER BY code")),
            )
            .await;
        assert_eq!(s, 201, "{i}");
        let imp = i["id"].as_str().unwrap().to_string();
        let r = api.run(&imp).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["counts"]["created"], 2, "{r}");
        let (_, list) = api.call("GET", "/api/v1/datasets", None).await;
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "suppliers_table")
            .unwrap()
            .clone();
        let (_, sample) = api
            .call(
                "GET",
                &format!("/api/v1/datasets/{}/sample", table["id"].as_str().unwrap()),
                None,
            )
            .await;
        assert_eq!(sample["rows"][0][1], "Acme, Inc", "{sample}");
        assert_eq!(sample["rows"][0][2], "9.50", "{sample}");

        // Rows only, one statement, read only.
        for hostile in [
            "DELETE FROM suppliers",
            "SELECT 1; DELETE FROM suppliers",
            "SET SESSION TRANSACTION READ WRITE",
            "DROP TABLE suppliers",
        ] {
            let (_, read) = api
                .call("GET", &format!("/api/v1/ontology/imports/{imp}"), None)
                .await;
            let mut b = import(hostile);
            b["expected_version"] = read["version"].clone();
            let (s, e) = api
                .call("PUT", &format!("/api/v1/ontology/imports/{imp}"), Some(b))
                .await;
            assert_eq!(s, 200, "{e}");
            let r = api.run(&imp).await;
            assert_eq!(r["run"]["state"], "failed", "{hostile}: {r}");
            assert_eq!(r["run"]["state_code"], "connector-query-invalid", "{hostile}: {r}");
        }

        // A cursor reads past the highest value, quoted by MySQL itself.
        let q = "SELECT code, COALESCE(name, code) AS name, updated FROM suppliers WHERE updated > {{cursor}}";
        let cursor = mapping(q).replace(
            "    query:",
            "    cursor:\n      column: updated\n      start: '0'\n    query:",
        );
        let (_, read) = api
            .call("GET", &format!("/api/v1/ontology/imports/{imp}"), None)
            .await;
        let b = json!({ "name": "suppliers", "mapping": cursor, "source": "file",
                        "mode": "incremental", "expected_version": read["version"] });
        let (s, e) = api
            .call("PUT", &format!("/api/v1/ontology/imports/{imp}"), Some(b))
            .await;
        assert_eq!(s, 200, "{e}");
        // The changed mapping stops the sync at a preview; the read itself is what
        // this checks, in the step's journal.
        let manifest = |r: &Value| r["steps"][0]["manifest"].clone();
        let r = api.run(&imp).await;
        let m = manifest(&r);
        assert_eq!((m["full_read"].as_bool(), m["cursor"].as_str()), (Some(true), Some("2")), "{r}");
        let r = api.run(&imp).await;
        let m = manifest(&r);
        assert_eq!((m["full_read"].as_bool(), m["read_rows"].as_i64()), (Some(false), Some(0)), "{r}");
    })
    .await;
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM suppliers")
        .fetch_one(&mut root)
        .await
        .unwrap();
    assert_eq!(left, 2, "nothing was written");
    sqlx::raw_sql("DROP TABLE suppliers")
        .execute(&mut root)
        .await
        .unwrap();
}
