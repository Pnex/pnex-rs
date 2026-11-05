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
        let (s, m) = alice
            .call(
                "POST",
                &format!("/api/v1/orgs/{org}/members"),
                Some(json!({ "email": "bob@example.com", "role": "member" })),
            )
            .await;
        assert!(s == 200 || s == 201, "{m}");
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
