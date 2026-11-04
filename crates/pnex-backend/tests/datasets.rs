//! Datasets API (data-pipelines.md D569): load, view, time travel, refusals,
//! org isolation (R1), viewer read-only (R2), and a bench that a large file is
//! not held in memory.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and the compose `rustfs` service.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::datasets;
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

async fn get(server: &axum_test::TestServer, path: &str, token: &str, org: i64) -> (u16, Value) {
    let res = server
        .get(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .await;
    (res.status_code().as_u16(), res.json::<Value>())
}

async fn create(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> (u16, Value) {
    let res = server
        .post("/api/v1/datasets")
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&json!({ "name": name }))
        .await;
    (res.status_code().as_u16(), res.json::<Value>())
}

/// Loads `csv` with the query string `query`.
async fn load(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    query: &str,
    csv: &str,
) -> (u16, Value) {
    let res = server
        .post(&format!("/api/v1/datasets/{id}/transactions/csv{query}"))
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .text(csv.to_string())
        .await;
    (res.status_code().as_u16(), res.json::<Value>())
}

#[tokio::test]
#[serial]
async fn snapshot_append_and_time_travel() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (s, d) = create(&server, &env.alice, org, "partners").await;
        assert_eq!(s, 201, "{d}");
        let id = d["id"].as_str().unwrap().to_string();
        let (s, e) = create(&server, &env.alice, org, "partners").await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "dataset-name-taken");

        // A quoted newline in a middle column, like the real open data file.
        let first = "\"code\";\"title\";\"state\"\r\n\"A\";\"line one\nline two\";\"on\"\r\n\"B\";;\"off\"\r\n";
        let (s, t) = load(&server, &env.alice, org, &id, "", first).await;
        assert_eq!(s, 201, "{t}");
        assert_eq!(t["state"], "committed");
        assert_eq!(t["rows"], 2);
        let sample = format!("/api/v1/datasets/{id}/sample");
        let (_, v) = get(&server, &sample, &env.alice, org).await;
        assert_eq!(v["columns"], json!(["code", "title", "state"]));
        assert_eq!(v["rows"][0][1], "line one\nline two");
        assert_eq!(v["rows"][1][1], Value::Null);

        let (_, t) = load(&server, &env.alice, org, &id, "?kind=append", "code;title;state\nC;c;on\n").await;
        assert_eq!(t["state"], "committed");
        let (_, d) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(d["rows"], 3, "{d}");
        let before_snapshot = chrono::Utc::now();

        // An append must keep the columns; the refusal leaves the view alone.
        let (s, e) = load(&server, &env.alice, org, &id, "?kind=append", "other\nX\n").await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "dataset-schema-mismatch");
        assert_eq!(e["errors"]["args"]["columns"], json!(["code", "title", "state"]));

        // A snapshot replaces the view, its columns with it.
        let (s, t) = load(&server, &env.alice, org, &id, "?delimiter=,", "x,y\n1,2\n").await;
        assert_eq!(s, 201, "{t}");
        let (_, d) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(d["rows"], 1);
        assert_eq!(d["columns"][0]["name"], "x");

        // Reading in the past gives the old view, whole.
        let at = before_snapshot.to_rfc3339().replace('+', "%2B");
        let (s, v) = get(&server, &format!("{sample}?as_of={at}"), &env.alice, org).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["columns"][0], "code");
        assert_eq!(v["rows"].as_array().unwrap().len(), 3);
        // Before the dataset had any row: empty.
        let (s, v) = get(&server, &format!("{sample}?as_of=2000-01-01T00:00:00Z"), &env.alice, org).await;
        assert_eq!(s, 200);
        assert!(v["rows"].as_array().unwrap().is_empty());

        // The refused load is journaled with its code, and not counted.
        let (_, list) = get(&server, &format!("/api/v1/datasets/{id}/transactions"), &env.alice, org).await;
        assert_eq!(list["count"], 4);
        let aborted: Vec<&Value> = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["state"] == "aborted")
            .collect();
        assert_eq!(aborted.len(), 1);
        assert_eq!(aborted[0]["code"], "dataset-schema-mismatch");
        let (_, all) = get(&server, "/api/v1/datasets", &env.alice, org).await;
        assert_eq!(all["count"], 1);

        // Delete removes the files too.
        let store = datasets::store_of(&ctx.config).unwrap();
        let prefix = format!("org_{org}/datasets/{id}/");
        assert!(!store.list_prefix(&prefix).await.unwrap().is_empty());
        let res = server
            .delete(&format!("/api/v1/datasets/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code().as_u16(), 204);
        assert!(store.list_prefix(&prefix).await.unwrap().is_empty());
        let (s, _) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(s, 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_bad_file_is_refused_and_leaves_nothing_visible() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, d) = create(&server, &env.alice, org, "bad").await;
        let id = d["id"].as_str().unwrap().to_string();

        let (s, e) = load(&server, &env.alice, org, &id, "", "a;b\n1;2\n3\n").await;
        assert_eq!(s, 400);
        assert_eq!(e["error"], "dataset-csv-invalid");
        assert_eq!(e["errors"]["args"]["reason"], "field_count");
        assert_eq!(e["errors"]["args"]["expected"], 2);
        let (s, e) = load(&server, &env.alice, org, &id, "", "").await;
        assert_eq!(s, 400);
        assert_eq!(e["errors"]["args"]["reason"], "empty");
        let (s, e) = load(&server, &env.alice, org, &id, "?delimiter=ab", "a\n").await;
        assert_eq!(s, 400);
        assert_eq!(e["error"], "field-invalid");

        let (_, d) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(d["rows"], 0);

        // The aborted loads are removed by the purge, then the next load works:
        // a failed one does not hold the dataset.
        let store = datasets::store_of(&ctx.config).unwrap();
        datasets::purge(&ctx.db, &store).await.unwrap();
        let prefix = format!("org_{org}/datasets/{id}/");
        assert!(store.list_prefix(&prefix).await.unwrap().is_empty());
        let (s, t) = load(&server, &env.alice, org, &id, "", "a\n1\n").await;
        assert_eq!(s, 201, "{t}");
        // Committed files survive the purge.
        datasets::purge(&ctx.db, &store).await.unwrap();
        assert_eq!(store.list_prefix(&prefix).await.unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_between_orgs_and_viewer_read_only() {
    with_app(|server, env, _ctx| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, d) = create(&server, &env.alice, org_a, "private").await;
        let id = d["id"].as_str().unwrap().to_string();
        let (s, _) = load(&server, &env.alice, org_a, &id, "", "a\nsecret\n").await;
        assert_eq!(s, 201);

        // Another org sees nothing of it (R1), even with the right id.
        for path in [
            format!("/api/v1/datasets/{id}"),
            format!("/api/v1/datasets/{id}/sample"),
            format!("/api/v1/datasets/{id}/transactions"),
        ] {
            let (s, e) = get(&server, &path, &env.bob, org_b).await;
            assert_eq!(s, 404, "{path}");
            assert_eq!(e["error"], "dataset-not-found");
        }
        let (s, _) = load(&server, &env.bob, org_b, &id, "", "a\nx\n").await;
        assert_eq!(s, 404);

        // A viewer reads, never writes (R2).
        let res = server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        assert!(res.status_code().is_success());
        let (s, _) = get(
            &server,
            &format!("/api/v1/datasets/{id}/sample"),
            &env.bob,
            org_a,
        )
        .await;
        assert_eq!(s, 200);
        let (s, e) = create(&server, &env.bob, org_a, "nope").await;
        assert_eq!(s, 403);
        assert_eq!(e["error"], "access-denied");
        let (s, e) = load(&server, &env.bob, org_a, &id, "", "a\nx\n").await;
        assert_eq!(s, 403);
        assert_eq!(e["error"], "access-denied");
        let res = server
            .delete(&format!("/api/v1/datasets/{id}"))
            .add_header("Authorization", format!("Bearer {}", env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(res.status_code().as_u16(), 403);
    })
    .await;
}

fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|l| l.split_whitespace().next()?.parse().ok())
        .unwrap()
}

/// The OP1.1 acceptance: 500 MB of CSV loaded without being held in memory.
/// `cargo test -p pnex-backend --test datasets --release -- --ignored --nocapture`.
#[tokio::test]
#[serial]
#[ignore]
async fn half_a_gigabyte_is_not_held_in_memory() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, d) = create(&server, &env.alice, org, "large").await;
        let id: uuid::Uuid = d["id"].as_str().unwrap().parse().unwrap();
        let store = datasets::store_of(&ctx.config).unwrap();

        const CHUNKS: usize = 500;
        const CHUNK: usize = 1024 * 1024;
        let start_rss = peak_rss_kb();
        let mut n = 0u64;
        let chunks = (0..CHUNKS).map(move |c| {
            let mut s = String::with_capacity(CHUNK + 256);
            if c == 0 {
                s.push_str("id;siret;name;city\n");
            }
            while s.len() < CHUNK {
                n += 1;
                let h = n.wrapping_mul(0x9E3779B97F4A7C15);
                s.push_str(&format!(
                    "{n};{:014};Organisation {h:x};City {}\n",
                    h % 100_000_000,
                    h % 500
                ));
            }
            Ok::<_, std::io::Error>(bytes::Bytes::from(s))
        });
        let begin = std::time::Instant::now();
        let t = datasets::load_csv(
            &ctx.db,
            &store,
            org,
            id,
            pnex_core::datasets::DatasetTransactionKind::Snapshot,
            None,
            futures_util::stream::iter(chunks),
        )
        .await
        .expect("load");
        let grown_mb = (peak_rss_kb() - start_rss) / 1024;
        println!(
            "500 MB: {} rows, {} MB of Parquet, {:?}, peak memory grew by {grown_mb} MB",
            t.rows,
            t.bytes / 1024 / 1024,
            begin.elapsed()
        );
        assert!(grown_mb < 400, "held in memory: {grown_mb} MB");
        datasets::delete(&ctx.db, &store, org, id).await.unwrap();
    })
    .await;
}

async fn send(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Value,
) -> (u16, Value) {
    let req = match method {
        "PUT" => server.put(path),
        _ => server.post(path),
    };
    let res = req
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

async fn person(server: &axum_test::TestServer, token: &str, org: i64, title: &str) -> String {
    let (s, o) = send(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        json!({ "type_key": "person", "title": title, "properties": {} }),
    )
    .await;
    assert_eq!(s, 201, "{o}");
    o["id"].as_str().unwrap().to_string()
}

async fn sql(db: &sea_orm::DatabaseConnection, text: &str) -> Vec<sea_orm::QueryResult> {
    use sea_orm::ConnectionTrait;
    db.query_all_raw(sea_orm::Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        text.to_string(),
    ))
    .await
    .unwrap()
}

/// A person leaves every readable version, under any case or accent; a file
/// without them is left alone; what their rows fed is purged and queued again.
#[tokio::test]
#[serial]
async fn erasing_an_object_removes_its_rows_from_every_version() {
    with_app(|server, env, ctx| async move {
        let store = datasets::store_of(&ctx.config).expect("store");
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let (s, e) = send(
            &server,
            "POST",
            "/api/v1/ontology/packs/prospecting_b2b/install",
            &env.alice,
            org,
            json!({}),
        )
        .await;
        assert_eq!(s, 200, "{e}");

        let (_, d) = create(&server, &env.alice, org, "people").await;
        let id = d["id"].as_str().unwrap().to_string();
        assert_eq!(d["identity"], json!([]));
        load(&server, &env.alice, org, &id, "", "name;city\nAnn Smith;Lyon\nBob Jones;Nice\n").await;
        let old = chrono::Utc::now().to_rfc3339().replace('+', "%2B");
        load(&server, &env.alice, org, &id, "", "name;city\nANN  SMITH;Lyon\nCara Lee;Metz\nBob Jones;Nice\n").await;
        load(&server, &env.alice, org, &id, "?kind=append", "name;city\nÀnn Smith;Pau\n").await;

        // Refused: a column it does not have, an unknown type, an unknown property.
        let path = format!("/api/v1/datasets/{id}/identity");
        let declare = |column: &str, type_key: &str, property: &str| {
            json!({ "columns": [{ "column": column, "type_key": type_key, "property": property }] })
        };
        for (body, field) in [
            (declare("nope", "person", "$title"), "columns.0.column"),
            (declare("name", "nope", "$title"), "columns.0.type_key"),
            (declare("name", "person", "nope"), "columns.0.property"),
        ] {
            let (s, e) = send(&server, "PUT", &path, &env.alice, org, body).await;
            assert_eq!(s, 400, "{e}");
            assert_eq!(e["errors"]["fields"][field], "invalid", "{e}");
        }
        // Another org does not see it (R1).
        let body = declare("name", "person", "$title");
        let (s, _) = send(&server, "PUT", &path, &env.bob, bob_org, body.clone()).await;
        assert_eq!(s, 404);
        let (s, d) = send(&server, "PUT", &path, &env.alice, org, body).await;
        assert_eq!(s, 200, "{d}");
        assert_eq!(d["identity"][0]["column"], "name");

        // A dataset computed from it, and one that is not.
        let (_, derived) = create(&server, &env.alice, org, "report").await;
        let derived = derived["id"].as_str().unwrap().to_string();
        load(&server, &env.alice, org, &derived, "", "who\nann\n").await;
        let (_, other) = create(&server, &env.alice, org, "unrelated").await;
        let other = other["id"].as_str().unwrap().to_string();
        load(&server, &env.alice, org, &other, "", "name\nAnn Smith\n").await;
        let (s, p) = send(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            json!({ "name": "report", "document": "transforms:\n  - output: report\n    inputs: { p: people }\n    sql: SELECT name AS who FROM p\n" }),
        )
        .await;
        assert_eq!(s, 201, "{p}");
        let pipeline = p["id"].as_str().unwrap();
        sql(&ctx.db, &format!(
            "INSERT INTO dataset_transaction_inputs (transaction_id, input_transaction_id, pipeline_id, pipeline_version) \
             SELECT o.id, i.id, '{pipeline}', 1 FROM dataset_transactions o, dataset_transactions i \
             WHERE o.dataset_id = '{derived}' AND i.dataset_id = '{id}'"
        )).await;

        // The same through a pipeline deleted since: its output is still reached.
        let (_, gone) = create(&server, &env.alice, org, "left over").await;
        let gone = gone["id"].as_str().unwrap().to_string();
        load(&server, &env.alice, org, &gone, "", "who\nann\n").await;
        let (s, p2) = send(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            json!({ "name": "gone", "document": "transforms:\n  - output: left over\n    inputs: { p: people }\n    sql: SELECT name AS who FROM p\n" }),
        )
        .await;
        assert_eq!(s, 201, "{p2}");
        let gone_pipeline = p2["id"].as_str().unwrap();
        sql(&ctx.db, &format!(
            "INSERT INTO dataset_transaction_inputs (transaction_id, input_transaction_id, pipeline_id, pipeline_version) \
             SELECT o.id, i.id, '{gone_pipeline}', 1 FROM dataset_transactions o, dataset_transactions i \
             WHERE o.dataset_id = '{gone}' AND i.dataset_id = '{id}'"
        )).await;
        let res = server
            .delete(&format!("/api/v1/pipelines/{gone_pipeline}"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code().as_u16(), 204);

        // A huge cell is cut when sampled.
        let (_, wide) = create(&server, &env.alice, org, "wide").await;
        let wide = wide["id"].as_str().unwrap().to_string();
        let cell = "x".repeat(2 * 1024 * 1024);
        load(&server, &env.alice, org, &wide, "", &format!("a\n{cell}\n{cell}\n")).await;
        let (s, v) = get(&server, &format!("/api/v1/datasets/{wide}/sample"), &env.alice, org).await;
        assert_eq!(s, 200);
        assert_eq!(v["rows"][1][0].as_str().unwrap().len(), 1000);

        // A person with no row: nothing rewritten, nothing recomputed.
        let nobody = person(&server, &env.alice, org, "Nobody Here").await;
        let erase = |object: String| format!("/api/v1/ontology/objects/{object}/erase");
        let (s, e) = send(&server, "POST", &erase(nobody), &env.alice, org, json!({})).await;
        assert_eq!(s, 200, "{e}");
        assert_eq!(datasets::erasure::erase_pass(&ctx.db, &store).await.unwrap(), 1);
        let rewritten = sql(&ctx.db, &format!(
            "SELECT f.key FROM dataset_files f JOIN dataset_transactions t ON t.id = f.transaction_id \
             WHERE t.dataset_id = '{id}' AND f.key LIKE '%erased-%'"
        )).await;
        assert!(rewritten.is_empty());
        let (s, _) = get(&server, &format!("/api/v1/datasets/{derived}/sample"), &env.alice, org).await;
        assert_eq!(s, 200);

        // Queued with the object; a run in progress makes it wait.
        let ann = person(&server, &env.alice, org, "Ann Smith").await;
        let (s, e) = send(&server, "POST", &erase(ann), &env.alice, org, json!({})).await;
        assert_eq!(s, 200, "{e}");
        let queued = sql(&ctx.db, "SELECT value FROM dataset_erasures WHERE done_at IS NULL").await;
        assert_eq!(queued.len(), 1);
        // Not the value: it stays in this table for good.
        assert_eq!(queued[0].try_get::<String>("", "value").unwrap().len(), 64);
        sql(&ctx.db, &format!(
            "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger, state, heartbeat_at) \
             VALUES ({org}, '{pipeline}', 1, 'manual', 'running', now())"
        )).await;
        assert_eq!(datasets::erasure::erase_pass(&ctx.db, &store).await.unwrap(), 0);
        sql(&ctx.db, "DELETE FROM pipeline_jobs").await;
        assert_eq!(datasets::erasure::erase_pass(&ctx.db, &store).await.unwrap(), 1);
        assert!(sql(&ctx.db, "SELECT 1 FROM dataset_erasures WHERE done_at IS NULL").await.is_empty());

        // Gone from the current view and from the past, the counts follow.
        let sample = format!("/api/v1/datasets/{id}/sample");
        let (_, v) = get(&server, &sample, &env.alice, org).await;
        assert_eq!(v["rows"], json!([["Cara Lee", "Metz"], ["Bob Jones", "Nice"]]));
        let (_, d) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(d["rows"], 2, "{d}");
        let (s, v) = get(&server, &format!("{sample}?as_of={old}"), &env.alice, org).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["rows"], json!([["Bob Jones", "Nice"]]));

        // The store holds no file the catalogue does not name.
        let listed = store.list_prefix(&format!("org_{org}/datasets/{id}/")).await.unwrap();
        let named = sql(&ctx.db, &format!(
            "SELECT f.key FROM dataset_files f JOIN dataset_transactions t ON t.id = f.transaction_id \
             WHERE t.dataset_id = '{id}'"
        )).await;
        assert_eq!(listed.len(), 3);
        assert_eq!(named.len(), 3);
        for row in &named {
            let key: String = row.try_get("", "key").unwrap();
            assert!(listed.iter().any(|(k, _, _)| *k == key), "{key}");
        }

        // What it fed is gone and queued; what it did not feed is untouched.
        let (s, e) = get(&server, &format!("/api/v1/datasets/{derived}/sample"), &env.alice, org).await;
        assert_eq!(s, 410, "{e}");
        let (s, e) = get(&server, &format!("/api/v1/datasets/{gone}/sample"), &env.alice, org).await;
        assert_eq!(s, 410, "{e}");
        let jobs = sql(&ctx.db, &format!(
            "SELECT 1 FROM pipeline_jobs WHERE pipeline_id = '{pipeline}' AND state = 'queued' AND trigger = 'erasure'"
        )).await;
        assert_eq!(jobs.len(), 1);
        let (_, v) = get(&server, &format!("/api/v1/datasets/{other}/sample"), &env.alice, org).await;
        assert_eq!(v["rows"], json!([["Ann Smith"]]));

        // A version whose last row goes stays readable, empty.
        sql(&ctx.db, "DELETE FROM pipeline_jobs").await;
        let bob = person(&server, &env.alice, org, "Bob Jones").await;
        send(&server, "POST", &erase(bob), &env.alice, org, json!({})).await;
        // The old file a pass left when it died after its commit goes too.
        let first_key: String = named[0].try_get("", "key").unwrap();
        let orphan = format!("{}part-0.parquet", first_key.rsplit_once('/').unwrap().0.to_string() + "/");
        let mut upload = store.writer(&orphan).await.unwrap();
        upload.write(bytes::Bytes::from_static(b"left over")).await.unwrap();
        upload.close().await.unwrap();
        assert_eq!(datasets::erasure::erase_pass(&ctx.db, &store).await.unwrap(), 1);
        let (s, v) = get(&server, &format!("{sample}?as_of={old}"), &env.alice, org).await;
        assert_eq!(s, 200, "{v}");
        assert_eq!(v["rows"], json!([]));
        assert_eq!(v["columns"], json!(["name", "city"]));
        let listed = store.list_prefix(&format!("org_{org}/datasets/{id}/")).await.unwrap();
        assert!(!listed.iter().any(|(k, _, _)| *k == orphan), "{orphan}");
        assert_eq!(listed.len(), 3);

        // A source that still holds the person does not bring them back.
        load(&server, &env.alice, org, &id, "", "name;city\nann smith;Lyon\nDan Roe;Caen\n").await;
        let (_, v) = get(&server, &sample, &env.alice, org).await;
        assert_eq!(v["rows"], json!([["Dan Roe", "Caen"]]));
        let (_, d) = get(&server, &format!("/api/v1/datasets/{id}"), &env.alice, org).await;
        assert_eq!(d["rows"], 1, "{d}");

        for dataset in [id, derived, other, gone, wide] {
            let dataset = dataset.parse().unwrap();
            datasets::delete(&ctx.db, &store, org, dataset).await.unwrap();
        }
    })
    .await;
}
