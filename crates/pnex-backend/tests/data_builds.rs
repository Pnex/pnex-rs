//! Chained runs (data-pipelines.md D575), stale tables (D576) and dataset events
//! (D578): imports really run; the worker's job is played in SQL, its own tests
//! cover what it computes.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), the compose `rustfs` and `valkey` services.

mod common;

use futures_util::StreamExt;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::artifact_store::{S3Config, S3Store};
use pnex_backend::services::{data_builds, ontology::imports};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde_json::{json, Value};
use serial_test::serial;
use std::time::Duration;

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
            "PATCH" => self.server.patch(path),
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

    async fn table(&self, name: &str, csv: &[u8]) -> String {
        let (s, d) = self
            .call("POST", "/api/v1/datasets", Some(json!({ "name": name })))
            .await;
        assert_eq!(s, 201, "{d}");
        let id = d["id"].as_str().unwrap().to_string();
        self.load(&id, csv).await;
        id
    }

    async fn load(&self, id: &str, csv: &[u8]) {
        let res = self
            .server
            .post(&format!("/api/v1/datasets/{id}/transactions/csv"))
            .add_header("Authorization", format!("Bearer {}", self.token))
            .add_header("X-Org-Id", self.org.to_string())
            .bytes(csv.to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
    }

    async fn pipeline(&self, name: &str, input: &str, output: &str) -> String {
        let (s, p) = self
            .call(
                "POST",
                "/api/v1/pipelines",
                Some(json!({ "name": name, "document": document(input, output) })),
            )
            .await;
        assert_eq!(s, 201, "{p}");
        p["id"].as_str().unwrap().to_string()
    }

    async fn import(&self, name: &str, table: &str, mapping: &str, extra: Value) -> String {
        let mut body = json!({ "name": name, "mapping": mapping, "source": "dataset",
                               "source_url": table, "mode": "incremental" });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (s, i) = self
            .call("POST", "/api/v1/ontology/imports", Some(body))
            .await;
        assert_eq!(s, 201, "{i}");
        i["id"].as_str().unwrap().to_string()
    }

    async fn build(&self, kind: &str, id: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/api/v1/data-builds",
            Some(json!({ "target_kind": kind, "target_id": id })),
        )
        .await
    }

    async fn get_build(&self, id: &str) -> Value {
        let (s, b) = self
            .call("GET", &format!("/api/v1/data-builds/{id}"), None)
            .await;
        assert_eq!(s, 200, "{b}");
        b
    }
}

fn document(input: &str, output: &str) -> String {
    format!("transforms:\n  - output: {output}\n    inputs: {{ i: {input} }}\n    sql: SELECT * FROM i\n")
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

fn store(ctx: &loco_rs::app::AppContext) -> S3Store {
    S3Config::from_config(&ctx.config).store().unwrap()
}

/// Plays the worker: the queued job of this pipeline ends in `state`.
async fn finish_job(db: &DatabaseConnection, pipeline: &str, state: &str, code: Option<&str>) {
    let code = code.map_or("NULL".to_string(), |c| format!("'{c}'"));
    let rows = sql(
        db,
        &format!(
            "UPDATE pipeline_jobs SET state = '{state}', code = {code}, finished_at = now() \
             WHERE pipeline_id = '{pipeline}' AND state IN ('queued', 'running') RETURNING id"
        ),
    )
    .await;
    assert_eq!(rows.len(), 1, "one job of {pipeline} was queued");
}

fn states(build: &Value) -> Vec<String> {
    build["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            format!(
                "{}={}",
                s["name"].as_str().unwrap(),
                s["state"].as_str().unwrap()
            )
        })
        .collect()
}

/// Ticks until the build ends: imports run in the background of this process.
async fn run_to_end(api: &Api<'_>, ctx: &loco_rs::app::AppContext, id: &str) -> Value {
    let store = store(ctx);
    for _ in 0..300 {
        data_builds::tick(&ctx.db, Some(&store)).await.unwrap();
        let b = api.get_build(id).await;
        if b["state"] != "running" {
            return b;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("build {id} never ended: {}", api.get_build(id).await);
}

const CERTS: &str = r#"
key: certs
name: Certifications
objects:
  - as: cert
    type: certification
    key: code
    title: { from: title }
    properties:
      code: { from: code, convert: [trim, upper] }
"#;

/// Waits for `certs`: its links need the certifications to exist.
const PARTNERS: &str = r#"
key: partners
name: Partners
after: [certs]
objects:
  - as: org
    type: organization
    key: siren
    title: { from: name, convert: [trim] }
    properties:
      siren: { from: siret, convert: [digits, "left:9", luhn] }
  - as: cert
    type: certification
    key: code
    create: false
    properties:
      code: { from: code, convert: [trim, upper] }
links:
  - type: accredited_for
    from: org
    to: cert
    attributes:
      scope: { from: scope, convert: [{ enum: { O: both, F: preparation, A: assessment } }] }
    on_conflict: { scope: both }
"#;
const CERTS_CSV: &[u8] = b"code;title\nRS1;First\n";
const PARTNERS_CSV: &[u8] = b"siret;name;code;scope\n73282932000074;Acme;RS1;F\n";

async fn links_of(db: &DatabaseConnection, org: i64) -> i64 {
    sql(
        db,
        &format!(
            "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} \
             AND valid_to IS NULL AND source_ref LIKE 'import:%'"
        ),
    )
    .await[0]
        .try_get("", "n")
        .unwrap()
}

#[tokio::test]
#[serial]
async fn a_cycle_is_refused_when_saved() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let a = api.pipeline("a", "raw", "clean").await;
        api.pipeline("b", "clean", "final").await;
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/pipelines/{a}"),
                Some(json!({ "document": document("final", "clean"), "expected_version": 1 })),
            )
            .await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["error"], "pipeline-document-invalid", "{e}");
        assert_eq!(e["errors"]["violations"][0]["token"], "cycle", "{e}");
        // A new document closing the same loop.
        let (s, _) = api
            .call(
                "POST",
                "/api/v1/pipelines",
                Some(json!({ "name": "c", "document": document("final", "raw") })),
            )
            .await;
        assert_eq!(s, 400);

        // Through imports.
        let (s, e) = api
            .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        assert_eq!(s, 200, "{e}");
        let certs = api.table("certs_table", CERTS_CSV).await;
        let partners = api.table("partners_table", PARTNERS_CSV).await;
        let one = api.import("certs", &certs, CERTS, json!({})).await;
        api.import("partners", &partners, PARTNERS, json!({})).await;
        let looped = format!("{CERTS}after: [partners]\n");
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/ontology/imports/{one}"),
                Some(
                    json!({ "name": "certs", "mapping": looped, "source": "dataset",
                             "source_url": certs, "mode": "incremental",
                             "expected_version": 1 }),
                ),
            )
            .await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["mapping.after"], "cycle", "{e}");
        let own = format!("{CERTS}after: [certs]\n");
        let (s, _) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(json!({ "name": "own", "mapping": own, "source": "dataset",
                             "source_url": certs, "mode": "incremental" })),
            )
            .await;
        assert_eq!(s, 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_failure_blocks_what_depends_on_it_and_nothing_else() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        let store = store(&ctx);
        api.table("raw", b"a\n1\n").await;
        let first = api.pipeline("first", "raw", "clean").await;
        let second = api.pipeline("second", "clean", "final").await;
        let third = api.pipeline("third", "final", "report").await;

        let plan = format!("/api/v1/data-builds/plan?target_kind=pipeline&target_id={third}");
        let (s, p) = api.call("GET", &plan, None).await;
        assert_eq!(s, 200, "{p}");
        let names: Vec<&str> = p["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["first", "second", "third"]);

        let (s, b) = api.build("pipeline", &third).await;
        assert_eq!(s, 202, "{b}");
        let id = b["id"].as_str().unwrap().to_string();

        // In order, one at a time.
        data_builds::tick(db, Some(&store)).await.unwrap();
        let b = api.get_build(&id).await;
        assert_eq!(
            states(&b),
            ["first=running", "second=pending", "third=pending"]
        );
        finish_job(db, &first, "done", None).await;
        data_builds::tick(db, Some(&store)).await.unwrap();
        let b = api.get_build(&id).await;
        assert_eq!(
            states(&b),
            ["first=done", "second=running", "third=pending"]
        );
        assert_eq!(
            sql(
                db,
                &format!("SELECT 1 AS x FROM pipeline_jobs WHERE pipeline_id = '{third}'")
            )
            .await
            .len(),
            0,
            "what follows has not started"
        );

        // Its input failed: the third never starts, the build says why.
        finish_job(db, &second, "failed", Some("pipeline-sql-invalid")).await;
        data_builds::tick(db, Some(&store)).await.unwrap();
        let b = api.get_build(&id).await;
        assert_eq!(states(&b), ["first=done", "second=failed", "third=blocked"]);
        assert_eq!(b["state"], "failed", "{b}");
        assert_eq!(b["code"], "pipeline-sql-invalid", "{b}");
        assert_eq!(b["args"]["step"], "second", "{b}");
        // The build's notice covers its runs; a failure outside a build has its own.
        let unnotified =
            "SELECT 1 AS x FROM pipeline_jobs WHERE state = 'failed' AND notified_at IS NULL";
        assert!(sql(db, unnotified).await.is_empty());
        let (s, r) = api
            .call("POST", &format!("/api/v1/pipelines/{first}/runs"), None)
            .await;
        assert_eq!(s, 202, "{r}");
        finish_job(db, &first, "failed", Some("pipeline-check-failed")).await;
        assert_eq!(sql(db, unnotified).await.len(), 1);
        data_builds::tick(db, Some(&store)).await.unwrap();
        assert!(sql(db, unnotified).await.is_empty());
        // Old finished builds go, a running or recent one stays.
        sql(
            db,
            &format!(
                "UPDATE data_builds SET finished_at = now() - interval '91 days' WHERE id = '{id}'"
            ),
        )
        .await;
        data_builds::tick(db, Some(&store)).await.unwrap();
        let (s, _) = api
            .call("GET", &format!("/api/v1/data-builds/{id}"), None)
            .await;
        assert_eq!(s, 404);
        assert_eq!(
            sql(
                db,
                &format!("SELECT 1 AS x FROM pipeline_jobs WHERE pipeline_id = '{third}'")
            )
            .await
            .len(),
            0
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn one_schedule_runs_an_import_and_what_it_waits_for() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        let (s, e) = api
            .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        assert_eq!(s, 200, "{e}");
        let certs_table = api.table("certs_table", CERTS_CSV).await;
        let partners_table = api.table("partners_table", PARTNERS_CSV).await;
        let every_day =
            json!({ "recurrence": { "start": "2026-01-01", "every": 1, "unit": "day" } });
        let certs = api.import("certs", &certs_table, CERTS, json!({})).await;
        // Declared second, scheduled alone: the only thing that orders them is `after`.
        let partners = api
            .import("partners", &partners_table, PARTNERS, every_day)
            .await;

        sql(
            db,
            &format!(
                "UPDATE object_imports SET next_run_at = now() - interval '1 minute' \
                 WHERE id = '{partners}'"
            ),
        )
        .await;
        imports::sweep(db).await.unwrap();
        let (_, list) = api.call("GET", "/api/v1/data-builds", None).await;
        assert_eq!(list["count"], 1, "{list}");
        let b = &list["results"][0];
        assert_eq!(b["trigger"], "schedule");
        assert_eq!(b["target_name"], "partners");
        let id = b["id"].as_str().unwrap().to_string();

        let b = run_to_end(&api, &ctx, &id).await;
        assert_eq!(states(&b), ["certs=done", "partners=done"], "{b}");
        assert_eq!(b["state"], "done", "{b}");
        assert_eq!(
            links_of(db, org).await,
            1,
            "the certification existed in time"
        );

        // A changed mapping stops its import at a preview: what waits for it stays.
        let (_, read) = api
            .call("GET", &format!("/api/v1/ontology/imports/{certs}"), None)
            .await;
        let changed = CERTS.replace("name: Certifications", "name: Other name");
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/ontology/imports/{certs}"),
                Some(
                    json!({ "name": "certs", "mapping": changed, "source": "dataset",
                             "source_url": certs_table, "mode": "incremental",
                             "expected_version": read["version"] }),
                ),
            )
            .await;
        assert_eq!(s, 200, "{e}");
        let (s, b) = api.build("import", &partners).await;
        assert_eq!(s, 202, "{b}");
        let b = run_to_end(&api, &ctx, b["id"].as_str().unwrap()).await;
        assert_eq!(states(&b), ["certs=failed", "partners=blocked"], "{b}");
        assert_eq!(b["code"], "import-preview-required", "{b}");
    })
    .await;
}

/// Waits for the run of the only step to end, then marks it lost with its server.
async fn lose_run_of_step(api: &Api<'_>, ctx: &loco_rs::app::AppContext, id: &str) -> String {
    let store = store(ctx);
    for _ in 0..300 {
        let b = api.get_build(id).await;
        if let Some(run) = b["steps"][0]["run_id"].as_str() {
            let state = sql(
                &ctx.db,
                &format!("SELECT state FROM object_import_runs WHERE id = '{run}'"),
            )
            .await;
            if state[0].try_get::<String>("", "state").unwrap() != "running" {
                sql(
                    &ctx.db,
                    &format!(
                        "UPDATE object_import_runs \
                         SET state = 'failed', state_code = 'import-run-interrupted' \
                         WHERE id = '{run}'"
                    ),
                )
                .await;
                return run.to_string();
            }
        } else {
            data_builds::tick(&ctx.db, Some(&store)).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no run ended: {}", api.get_build(id).await);
}

#[tokio::test]
#[serial]
async fn a_run_lost_with_its_server_starts_again_once() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let (s, e) = api
            .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        assert_eq!(s, 200, "{e}");
        let table = api.table("certs_table", CERTS_CSV).await;
        let import = api.import("certs", &table, CERTS, json!({})).await;

        // A deployment during the run: the build carries on with a new run.
        let (s, b) = api.build("import", &import).await;
        assert_eq!(s, 202, "{b}");
        let id = b["id"].as_str().unwrap().to_string();
        let lost = lose_run_of_step(&api, &ctx, &id).await;
        let done = run_to_end(&api, &ctx, &id).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["steps"][0]["retries"], 1, "{done}");
        assert_ne!(done["steps"][0]["run_id"], lost.as_str(), "{done}");

        // Lost twice: it stops there and says why.
        let (s, b) = api.build("import", &import).await;
        assert_eq!(s, 202, "{b}");
        let id = b["id"].as_str().unwrap().to_string();
        lose_run_of_step(&api, &ctx, &id).await;
        let store = store(&ctx);
        data_builds::tick(&ctx.db, Some(&store)).await.unwrap();
        let again = api.get_build(&id).await;
        assert_eq!(again["steps"][0]["retries"], 1, "{again}");
        lose_run_of_step(&api, &ctx, &id).await;
        let failed = run_to_end(&api, &ctx, &id).await;
        assert_eq!(failed["state"], "failed", "{failed}");
        assert_eq!(
            failed["steps"][0]["code"], "import-run-interrupted",
            "{failed}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_request_during_a_build_is_one_more_pass() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        let store = store(&ctx);
        api.table("raw", b"a\n1\n").await;
        let p = api.pipeline("only", "raw", "clean").await;
        let (_, first) = api.build("pipeline", &p).await;
        let (s, again) = api.build("pipeline", &p).await;
        assert_eq!(s, 202);
        assert_eq!(again["id"], first["id"], "merged");
        assert_eq!(again["again"], true);
        let (_, third) = api.build("pipeline", &p).await;
        assert_eq!(third["id"], first["id"], "merged once");

        data_builds::tick(db, Some(&store)).await.unwrap();
        finish_job(db, &p, "done", None).await;
        data_builds::tick(db, Some(&store)).await.unwrap();
        data_builds::tick(db, Some(&store)).await.unwrap();
        let (_, list) = api.call("GET", "/api/v1/data-builds", None).await;
        assert_eq!(list["count"], 2, "exactly one more: {list}");
        assert_eq!(list["results"][1]["state"], "done", "{list}");
        assert_eq!(list["results"][0]["state"], "running", "{list}");
        assert_eq!(list["results"][0]["again"], false, "{list}");
    })
    .await;
}

/// An edit moves the schedule to its editor (SEC-58), and a person who left is no longer
/// named in the history.
#[tokio::test]
#[serial]
async fn an_edit_takes_the_schedule_and_a_former_member_stays_unnamed() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        personal_org(&server, &env.bob).await;
        let alice = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        alice.table("raw", b"a\n1\n").await;
        let p = alice.pipeline("only", "raw", "clean").await;
        let every_day =
            json!({ "recurrence": { "start": "2026-01-01", "every": 1, "unit": "day" } });
        let (s, e) = alice
            .call("PUT", &format!("/api/v1/pipelines/{p}/schedule"), Some(every_day))
            .await;
        assert_eq!(s, 200, "{e}");
        let (s, m) = alice
            .call(
                "POST",
                &format!("/api/v1/orgs/{org}/members"),
                Some(json!({ "email": "bob@example.com", "role": "admin" })),
            )
            .await;
        assert!((200..300).contains(&s), "{m}");
        let bob = Api {
            server: &server,
            token: &env.bob,
            org,
        };
        let path = format!("/api/v1/pipelines/{p}");
        let (_, current) = bob.call("GET", &path, None).await;
        let (s, e) = bob
            .call(
                "PUT",
                &path,
                Some(json!({ "document": current["document"], "expected_version": current["version"] })),
            )
            .await;
        assert_eq!(s, 200, "{e}");
        let by = sql(
            &ctx.db,
            &format!(
                "SELECT u.email FROM pipelines p JOIN users u ON u.id = p.scheduled_by \
                 WHERE p.id = '{p}'"
            ),
        )
        .await;
        assert_eq!(by[0].try_get::<String>("", "email").unwrap(), "bob@example.com");

        sql(
            &ctx.db,
            &format!(
                "DELETE FROM organization_members WHERE org_id = {org} AND user_id = \
                 (SELECT id FROM users WHERE email = 'bob@example.com')"
            ),
        )
        .await;
        let (_, v) = alice.call("GET", &format!("{path}/versions"), None).await;
        assert_eq!(v["results"][0]["author"], Value::Null, "{v}");
        assert_eq!(v["results"][1]["author"], "alice@example.com", "{v}");
    })
    .await;
}

/// Demoted between two recurrences: the chained run stops before writing anything.
#[tokio::test]
#[serial]
async fn a_scheduled_build_reads_the_rights_of_its_author_before_each_step() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let store = store(&ctx);
        api.table("raw", b"a\n1\n").await;
        let p = api.pipeline("only", "raw", "clean").await;
        let author: i64 = sql(
            &ctx.db,
            "SELECT id FROM users WHERE email = 'alice@example.com'",
        )
        .await[0]
            .try_get("", "id")
            .unwrap();
        let build = data_builds::request(
            &ctx.db,
            org,
            pnex_core::pipelines::BuildTargetKind::Pipeline,
            p.parse().unwrap(),
            "schedule",
            Some(author),
        )
        .await
        .unwrap();
        sql(
            &ctx.db,
            &format!("UPDATE organization_members SET role = 'viewer' WHERE org_id = {org}"),
        )
        .await;
        data_builds::tick(&ctx.db, Some(&store)).await.unwrap();
        let queued = sql(&ctx.db, "SELECT id FROM pipeline_jobs").await;
        assert!(queued.is_empty(), "a job was queued for a viewer");
        let done = api.get_build(&build.id).await;
        assert_eq!(done["steps"][0]["code"], "build-author-forbidden", "{done}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_schedule_answers_to_who_set_it() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let alice = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let bob = Api {
            server: &server,
            token: &env.bob,
            org,
        };
        let db = &ctx.db;
        let store = store(&ctx);
        alice.table("raw", b"a\n1\n").await;
        let p = alice.pipeline("only", "raw", "clean").await;
        let schedule = format!("/api/v1/pipelines/{p}/schedule");
        let every_day =
            json!({ "recurrence": { "start": "2026-01-01", "every": 1, "unit": "day" } });

        // Another org sees nothing.
        let other = Api {
            server: &server,
            token: &env.bob,
            org: bob_org,
        };
        let (s, e) = other.call("PUT", &schedule, Some(every_day.clone())).await;
        assert_eq!(s, 404, "{e}");
        let (s, e) = other.build("pipeline", &p).await;
        assert_eq!(s, 404, "{e}");
        assert_eq!(e["error"], "build-not-found");
        let (_, list) = alice.call("GET", "/api/v1/data-builds", None).await;
        let id = list["results"].as_array().unwrap().first().cloned();
        assert!(id.is_none());

        // A viewer reads, and starts nothing.
        let members = format!("/api/v1/orgs/{org}/members");
        let (s, m) = alice
            .call(
                "POST",
                &members,
                Some(json!({ "email": "bob@example.com", "role": "viewer" })),
            )
            .await;
        assert!(s == 200 || s == 201, "{m}");
        let (s, _) = bob.call("GET", "/api/v1/data-builds", None).await;
        assert_eq!(s, 200);
        let (s, e) = bob.call("PUT", &schedule, Some(every_day.clone())).await;
        assert_eq!(
            (s, e["error"].as_str()),
            (403, Some("access-denied")),
            "{e}"
        );
        let (s, e) = bob.build("pipeline", &p).await;
        assert_eq!(
            (s, e["error"].as_str()),
            (403, Some("access-denied")),
            "{e}"
        );

        // Bad recurrence.
        let (s, e) = alice
            .call(
                "PUT",
                &schedule,
                Some(json!({ "recurrence": { "every": "often" } })),
            )
            .await;
        assert_eq!(s, 400, "{e}");

        let (s, set) = alice.call("PUT", &schedule, Some(every_day)).await;
        assert_eq!(s, 200, "{set}");
        assert!(set["next_run_at"].is_string(), "{set}");

        // Still a member with the right: it runs.
        let due = format!(
            "UPDATE pipelines SET next_run_at = now() - interval '1 minute' WHERE id = '{p}'"
        );
        sql(db, &due).await;
        data_builds::tick(db, Some(&store)).await.unwrap();
        let (_, list) = alice.call("GET", "/api/v1/data-builds", None).await;
        assert_eq!(list["results"][0]["state"], "running", "{list}");
        assert_eq!(list["results"][0]["trigger"], "schedule", "{list}");
        let next = alice
            .call("GET", &format!("/api/v1/pipelines/{p}"), None)
            .await
            .1;
        assert!(next["next_run_at"].is_string(), "moved ahead: {next}");
        finish_job(db, &p, "done", None).await;
        data_builds::tick(db, Some(&store)).await.unwrap();

        // Who set it is no longer allowed: nothing runs, and it says why.
        sql(
            db,
            &format!(
                "UPDATE pipelines SET scheduled_by = (SELECT id FROM users \
                 WHERE email = 'bob@example.com') WHERE id = '{p}'"
            ),
        )
        .await;
        sql(db, &due).await;
        let jobs_before = sql(db, "SELECT id FROM pipeline_jobs").await.len();
        data_builds::tick(db, Some(&store)).await.unwrap();
        let (_, list) = alice.call("GET", "/api/v1/data-builds", None).await;
        assert_eq!(list["results"][0]["state"], "failed", "{list}");
        assert_eq!(
            list["results"][0]["code"], "build-author-forbidden",
            "{list}"
        );
        assert_eq!(
            sql(db, "SELECT id FROM pipeline_jobs").await.len(),
            jobs_before
        );

        // `null` stops it.
        let (s, off) = alice
            .call("PUT", &schedule, Some(json!({ "recurrence": null })))
            .await;
        assert_eq!(s, 200, "{off}");
        assert!(
            off["next_run_at"].is_null() && off["recurrence"].is_null(),
            "{off}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_table_without_a_new_version_warns_once() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        let id = api.table("feed", b"a\n1\n").await;
        let path = format!("/api/v1/datasets/{id}/freshness");
        for bad in [0, -1, 100_000] {
            let (s, e) = api
                .call("PUT", &path, Some(json!({ "max_age_hours": bad })))
                .await;
            assert_eq!(s, 400, "{e}");
        }
        let (s, d) = api
            .call("PUT", &path, Some(json!({ "max_age_hours": 24 })))
            .await;
        assert_eq!(s, 200, "{d}");
        assert_eq!(
            (d["max_age_hours"].as_i64(), d["stale"].as_bool()),
            (Some(24), Some(false))
        );

        let notified = |db: &DatabaseConnection, id: String| {
            let db = db.clone();
            async move {
                sql(
                    &db,
                    &format!(
                        "SELECT to_char(stale_notified_at, 'HH24:MI:SS.US') AS at \
                         FROM datasets WHERE id = '{id}'"
                    ),
                )
                .await[0]
                    .try_get::<Option<String>>("", "at")
                    .unwrap()
            }
        };
        data_builds::tick(db, None).await.unwrap();
        assert!(
            notified(db, id.clone()).await.is_none(),
            "fresh: no warning"
        );

        sql(
            db,
            &format!(
                "UPDATE dataset_transactions SET committed_at = now() - interval '2 days' \
                 WHERE dataset_id = '{id}'"
            ),
        )
        .await;
        let (_, d) = api
            .call("GET", &format!("/api/v1/datasets/{id}"), None)
            .await;
        assert_eq!(d["stale"], true, "{d}");
        data_builds::tick(db, None).await.unwrap();
        let first = notified(db, id.clone()).await.expect("warned");
        data_builds::tick(db, None).await.unwrap();
        assert_eq!(notified(db, id.clone()).await.unwrap(), first, "once");

        // A new version ends it, and a later lapse warns again.
        api.load(&id, b"a\n2\n").await;
        let (_, d) = api
            .call("GET", &format!("/api/v1/datasets/{id}"), None)
            .await;
        assert_eq!(d["stale"], false, "{d}");
        sql(
            db,
            &format!(
                "UPDATE dataset_transactions SET committed_at = now() - interval '2 days' \
                 WHERE dataset_id = '{id}'"
            ),
        )
        .await;
        sql(
            db,
            &format!(
                "UPDATE datasets SET stale_notified_at = now() - interval '3 days' \
                 WHERE id = '{id}'"
            ),
        )
        .await;
        data_builds::tick(db, None).await.unwrap();
        assert_ne!(
            notified(db, id.clone()).await.unwrap(),
            first,
            "warned again"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn commits_and_syncs_reach_the_channel_of_their_org_once() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let db = &ctx.db;
        let url = std::env::var("PNEX_TEST_APP_VALKEY_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379/14".to_string());
        let client = redis::Client::open(url).unwrap();
        let mut sub = client.get_async_pubsub().await.unwrap();
        sub.subscribe(pnex_core::pipelines::dataset_channel(org))
            .await
            .unwrap();
        let mut conn = pnex_backend::services::shared_valkey::conn(&ctx.config)
            .await
            .expect("valkey is configured for tests");

        let (s, e) = api
            .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        assert_eq!(s, 200, "{e}");
        let table = api.table("certs_table", CERTS_CSV).await;
        // An old commit is not announced late.
        let old = api.table("old_table", b"a\n1\n").await;
        sql(
            db,
            &format!(
                "UPDATE dataset_transactions SET committed_at = now() - interval '2 hours' \
                 WHERE dataset_id = '{old}'"
            ),
        )
        .await;
        let certs = api.import("certs", &table, CERTS, json!({})).await;
        let (s, r) = api
            .call(
                "POST",
                &format!("/api/v1/ontology/imports/{certs}/runs"),
                None,
            )
            .await;
        assert_eq!(s, 202, "{r}");
        let run = r["id"].as_str().unwrap().to_string();
        for _ in 0..200 {
            let (_, v) = api
                .call(
                    "GET",
                    &format!("/api/v1/ontology/imports/{certs}/runs/{run}"),
                    None,
                )
                .await;
            if v["run"]["state"] != "running" {
                assert_eq!(v["run"]["state"], "applied", "{v}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        data_builds::publish_events(db, &mut conn).await.unwrap();
        let mut stream = sub.on_message();
        let mut next = async || {
            let msg = tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .expect("an event")
                .unwrap();
            serde_json::from_str::<Value>(&msg.get_payload::<String>().unwrap()).unwrap()
        };
        let commit = next().await;
        assert_eq!(commit["event"], "dataset.committed", "{commit}");
        assert_eq!(commit["dataset"], "certs_table", "{commit}");
        assert_eq!(commit["counts"]["rows"], 1, "{commit}");
        let synced = next().await;
        assert_eq!(synced["event"], "dataset.synced", "{synced}");
        assert_eq!(synced["dataset"], "certs_table", "{synced}");
        assert_eq!(synced["reference"], format!("{certs}@1"), "{synced}");
        assert_eq!(synced["changed"].as_array().unwrap().len(), 1, "{synced}");

        // Already sent: nothing more comes.
        data_builds::publish_events(db, &mut conn).await.unwrap();
        let quiet = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        assert!(quiet.is_err(), "sent once");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_trigger_needs_a_table_of_the_org_to_deploy() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let api = Api {
            server: &server,
            token: &env.alice,
            org,
        };
        let graph = |dataset: &str| {
            json!({ "nodes": [
                { "id": "dt", "kind": "dataset_trigger", "config": { "dataset": dataset },
                  "outputs": [{ "port": 0, "targets": ["dbg"] }] },
                { "id": "dbg", "kind": "debug" }
            ] })
        };
        let (s, f) = api
            .call(
                "POST",
                "/api/v1/flows",
                Some(json!({ "name": "On table", "graph": graph("not_there") })),
            )
            .await;
        assert_eq!(s, 201, "{f}");
        let id = f["id"].as_i64().unwrap();
        let deploy = format!("/api/v1/flows/{id}/deploy");
        let (s, d) = api.call("POST", &deploy, Some(json!({}))).await;
        assert_eq!(s, 400, "{d}");
        assert_eq!(
            d["errors"]["violations"][0]["code"], "dataset-not-found",
            "{d}"
        );

        let save = |dataset: &'static str, expected: i64| {
            let path = format!("/api/v1/flows/{id}");
            let body = json!({ "graph": graph(dataset), "expected_version_number": expected });
            let api = &api;
            async move { api.call("PATCH", &path, Some(body)).await }
        };
        let (s, d) = save("", 1).await;
        assert_eq!(s, 200, "{d}");
        let (s, d) = api.call("POST", &deploy, Some(json!({}))).await;
        assert_eq!(s, 400, "{d}");
        assert_eq!(
            d["errors"]["violations"][0]["code"], "dataset_trigger_unset",
            "{d}"
        );

        api.table("there", b"a\n1\n").await;
        let (s, d) = save("there", 2).await;
        assert_eq!(s, 200, "{d}");
        // The gate lets it through; the engine is off in tests.
        let (s, d) = api.call("POST", &deploy, Some(json!({}))).await;
        assert_ne!(s, 400, "{d}");
    })
    .await;
}
