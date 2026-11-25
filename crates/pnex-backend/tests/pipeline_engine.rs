//! The pipeline engine on a real base (pipelines.md D937, D941): runs
//! write the org's table and keep the state of each row, so that a row done
//! is never asked again and an empty one waits its days.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::pipeline_engine::blob::Blob;
use pnex_backend::services::pipeline_engine::store::{fixture, run_for_org, shipped};
use pnex_backend::services::pipeline_engine::{Failure, Req, Transport};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let token = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, ctx.clone(), token).await;
        },
    )
    .await;
}

/// The fixtures of the pack's own test, counted.
struct Fixtures {
    calls: AtomicUsize,
}

impl Fixtures {
    fn answer(&self, url: &reqwest::Url) -> Option<Blob> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let base = "https://www.histdata.com/download-free-forex-historical-data/?/metatrader/1-minute-bar-quotes/eurusd/";
        let file = match url.as_str() {
            u if u == format!("{base}2024") => "histdata_page_2024.html",
            u if u == format!("{base}2025") => "histdata_page_2025.html",
            "https://www.histdata.com/get.php" => "histdata_eurusd_2024.zip",
            _ => return None,
        };
        fixture("markets", file).map(|b| Blob::Mem(b.to_vec()))
    }
}

impl Transport for Fixtures {
    async fn get(&self, url: &reqwest::Url, _: &Req) -> Result<Option<Blob>, Failure> {
        Ok(self.answer(url))
    }

    async fn post_form(
        &self,
        url: &reqwest::Url,
        _: Option<&reqwest::Url>,
        fields: &[(String, String)],
        _: &Req,
    ) -> Result<Option<Blob>, Failure> {
        assert!(
            fields.iter().any(|(k, v)| k == "tk" && !v.is_empty()),
            "{fields:?}"
        );
        Ok(self.answer(url))
    }
}

#[tokio::test]
#[serial]
async fn rows_done_are_kept_and_empty_ones_wait_their_days() {
    with_app(|server, ctx, token| async move {
        let auth = format!("Bearer {token}");
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", auth.clone())
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let call = |method: &'static str, path: String, body: Option<Value>| {
            let auth = auth.clone();
            let server = &server;
            async move {
                let req = match method {
                    "POST" => server.post(&path),
                    _ => server.patch(&path),
                }
                .add_header("Authorization", auth)
                .add_header("X-Org-Id", org.to_string());
                let r = match body {
                    Some(b) => req.json(&b).await,
                    None => req.await,
                };
                assert!(r.status_code().is_success(), "{path}: {}", r.text());
            }
        };
        call("POST", "/api/v1/ontology/packs/markets/install".into(), None).await;
        call(
            "PATCH",
            "/api/v1/ontology/packs/markets/settings".into(),
            Some(json!({ "values": { "histdata_pairs": "eurusd", "histdata_from": "2024" } })),
        )
        .await;
        let sp = shipped().iter().find(|p| p.id() == "markets/histdata_m1").unwrap();
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let net = Fixtures { calls: AtomicUsize::new(0) };
        let day = |d: u32| Utc.with_ymd_and_hms(2026, 2, d, 12, 0, 0).unwrap();
        let states = || async {
            ctx.db
                .query_all_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!(
                        "SELECT item || ':' || state || ':' || rows AS s FROM pipeline_items \
                         WHERE org_id = {org} AND pipeline = 'markets/histdata_m1' AND item <> '' ORDER BY item"
                    ),
                ))
                .await
                .unwrap()
                .iter()
                .map(|r| r.try_get::<String>("", "s").unwrap())
                .collect::<Vec<_>>()
        };

        // One row per run (per_run 1): the year with a file, then the empty one.
        for d in [10, 10] {
            let out = run_for_org(&ctx.db, &store, org, sp, &net, day(d), None).await.unwrap();
            assert!(out.failure.is_none(), "{:?}", out.failure);
        }
        assert_eq!(states().await, ["EURUSD/2024:done:3", "EURUSD/2025:empty:0"]);
        let calls = net.calls.load(Ordering::SeqCst);
        assert_eq!(calls, 3, "two pages and one file");

        // Nothing is due: a done year is never asked again, an empty one waits a week.
        let out = run_for_org(&ctx.db, &store, org, sp, &net, day(12), None).await.unwrap();
        assert!(out.items.is_empty());
        assert_eq!(net.calls.load(Ordering::SeqCst), calls);
        let out = run_for_org(&ctx.db, &store, org, sp, &net, day(18), None).await.unwrap();
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.items[0].0.key, "EURUSD/2025");

        // The table holds UTC instants of the New York wall clock (D939).
        let rows = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT id FROM datasets WHERE org_id = {org} AND name = 'histdata_m1'"),
            ))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "one table, made by the first write");
        let id: uuid::Uuid = rows[0].try_get("", "id").unwrap();
        let sample = server
            .get(&format!("/api/v1/datasets/{id}/sample"))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .text();
        assert!(sample.contains("2024-07-05T20:59:00"), "{sample}");
        assert!(sample.contains("2024-01-02T13:30:00"), "{sample}");
    })
    .await;
}

/// D941: a pipeline's `write: objects` updates the org's objects at once,
/// without preview, its provenance the pipeline.
#[tokio::test]
#[serial]
async fn a_pipeline_writes_objects_without_preview() {
    with_app(|server, ctx, token| async move {
        let worker = common::spawn_pipeline_worker();
        let auth = format!("Bearer {token}");
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", auth.clone())
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let post = |path: &'static str, body: Option<Value>| {
            let req = server
                .post(path)
                .add_header("Authorization", auth.clone())
                .add_header("X-Org-Id", org.to_string());
            async move {
                let r = match body {
                    Some(b) => req.json(&b).await,
                    None => req.await,
                };
                assert!(r.status_code().is_success(), "{path}: {}", r.text());
                r.json::<Value>()
            }
        };
        post("/api/v1/ontology/packs/companies/install", None).await;
        let acme = post(
            "/api/v1/ontology/objects",
            Some(json!({ "type_key": "organization", "title": "ACME", "properties": { "lei": "529900T8BM49AURSDO55" } })),
        )
        .await;
        let doc: pnex_core::pipeline::doc::Document = serde_saphyr::from_str(
            "key: t\nsteps:\n\
             - enumerate: { as: o, cross: [{ as: lei, objects: { type: organization, property: lei } }] }\n\
             - sql: { as: rows, sql: \"SELECT lei, 'FR' AS country FROM o\" }\n\
             - write: { from: rows, objects: { objects: [{ as: org, type: organization, key: lei, create: false, update: true, properties: { lei: lei, country: country } }] } }\n",
        )
        .unwrap();
        assert!(doc.violations().is_empty(), "{:?}", doc.violations());
        let sp = pnex_backend::services::pipeline_engine::store::Shipped {
            pack: "companies".into(),
            doc,
        };
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let net = Fixtures { calls: AtomicUsize::new(0) };
        let out = run_for_org(&ctx.db, &store, org, &sp, &net, Utc::now(), None).await.unwrap();
        assert!(out.failure.is_none(), "{:?}", out.failure);
        assert_eq!(out.written, 1);
        let read = server
            .get(&format!("/api/v1/ontology/objects/{}", acme["id"].as_str().unwrap()))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<Value>();
        assert_eq!(read["properties"]["country"], "FR", "{read}");
        worker.abort();
    })
    .await;
}

/// D934 parity with a transform (D571): a table write keeps the org tables
/// its `sql` steps read (D574), sets its retention, and its checks compare
/// with the current version (D576).
#[tokio::test]
#[serial]
async fn a_table_write_keeps_lineage_retention_and_checks() {
    with_app(|server, ctx, token| async move {
        let worker = common::spawn_pipeline_worker();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let net = Fixtures { calls: AtomicUsize::new(0) };
        let run = |steps: &str| {
            let doc: pnex_core::pipeline::doc::Document =
                serde_saphyr::from_str(&format!("key: t\nsteps:\n{steps}")).unwrap();
            assert!(doc.violations().is_empty(), "{:?}", doc.violations());
            let sp = pnex_backend::services::pipeline_engine::store::Shipped {
                pack: "companies".into(),
                doc,
            };
            let (db, store, net) = (&ctx.db, &store, &net);
            async move { run_for_org(db, store, org, &sp, net, Utc::now(), None).await.unwrap() }
        };
        let out = run(
            "- sql: { as: rows, sql: \"SELECT 'a' AS k UNION ALL SELECT 'b' AS k\" }\n\
             - write: { from: rows, table: { name: src, replace: true } }\n",
        )
        .await;
        assert!(out.failure.is_none(), "{:?}", out.failure);
        let derived = |filter: &str| {
            format!(
                "- sql: {{ as: rows, tables: {{ s: src }}, sql: \"SELECT k FROM s {filter}\" }}\n\
                 - write: {{ from: rows, table: {{ name: derived, replace: true, retain_days: 30, \
                   checks: {{ max_drop_pct: 10, same_columns: true }} }} }}\n"
            )
        };
        let out = run(&derived("")).await;
        assert!(out.failure.is_none(), "{:?}", out.failure);
        assert_eq!(out.written, 2);
        let row = ctx
            .db
            .query_one_raw(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT d.retain_days, \
                   (SELECT count(*) FROM dataset_transaction_inputs i \
                    JOIN dataset_transactions o ON o.id = i.transaction_id \
                    JOIN dataset_transactions it ON it.id = i.input_transaction_id \
                    JOIN datasets s ON s.id = it.dataset_id \
                    WHERE o.dataset_id = d.id AND s.name = 'src' AND i.pipeline_id IS NULL) AS inputs \
                 FROM datasets d WHERE d.org_id = $1 AND d.name = 'derived'",
                vec![org.into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<Option<i32>>("", "retain_days").unwrap(), Some(30));
        assert_eq!(row.try_get::<i64>("", "inputs").unwrap(), 1);
        // Half the rows gone: the current version stays.
        let out = run(&derived("WHERE k = 'a'")).await;
        let f = out.failure.expect("the drop is refused");
        assert_eq!(f.code, pnex_core::err_codes::PIPELINE_CHECK_FAILED);
        assert_eq!(
            f.args,
            json!({ "output": "derived", "check": "max_drop_pct", "column": null, "value": "50", "limit": "10" })
        );
        worker.abort();
    })
    .await;
}

/// Runs `steps` as a pipeline of the org, its `sql` steps on the worker.
async fn run_steps(
    ctx: &loco_rs::app::AppContext,
    org: i64,
    steps: &str,
) -> pnex_backend::services::pipeline_engine::RunOutcome {
    let doc: pnex_core::pipeline::doc::Document =
        serde_saphyr::from_str(&format!("key: t\nsteps:\n{steps}")).unwrap();
    assert!(doc.violations().is_empty(), "{:?}", doc.violations());
    let sp = pnex_backend::services::pipeline_engine::store::Shipped {
        pack: "companies".into(),
        doc,
    };
    let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
    let net = Fixtures {
        calls: AtomicUsize::new(0),
    };
    run_for_org(&ctx.db, &store, org, &sp, &net, Utc::now(), None)
        .await
        .unwrap()
}

async fn first_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .unwrap()
}

/// Committed versions of a table of the org: (rows) in order.
async fn committed(ctx: &loco_rs::app::AppContext, org: i64, table: &str) -> Vec<i64> {
    ctx.db
        .query_all_raw(Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT t.rows FROM dataset_transactions t JOIN datasets d ON d.id = t.dataset_id \
             WHERE d.org_id = $1 AND d.name = $2 AND t.state = 'committed' ORDER BY t.created_at",
            vec![org.into(), table.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|r| r.try_get::<i64>("", "rows").unwrap())
        .collect()
}

/// D576 (ported from the worker's transforms): each check alone refuses with
/// what it measured, and the committed version stays the only one.
#[tokio::test]
#[serial]
async fn each_table_check_alone_refuses_and_keeps_the_version() {
    with_app(|server, ctx, token| async move {
        let worker = common::spawn_pipeline_worker();
        let org = first_org(&server, &token).await;
        let seed = "- sql: { as: rows, sql: \"SELECT 'a' AS k, '1' AS v UNION ALL SELECT 'b', '2' \
                    UNION ALL SELECT 'c', '3' UNION ALL SELECT 'd', '4'\" }\n\
                    - write: { from: rows, table: { name: raw, replace: true } }\n";
        assert!(run_steps(&ctx, org, seed).await.failure.is_none());
        let clean = |checks: &str, sql: &str| {
            format!(
                "- sql: {{ as: rows, tables: {{ r: raw }}, sql: \"{sql}\" }}\n\
                 - write: {{ from: rows, table: {{ name: clean, replace: true, checks: {checks} }} }}\n"
            )
        };
        let all = "SELECT k, v FROM r";
        let out = run_steps(
            &ctx,
            org,
            &clean("{ min_rows: 1, unique: [k], max_empty_pct: { v: 0 } }", all),
        )
        .await;
        assert!(out.failure.is_none(), "{:?}", out.failure);
        assert_eq!(committed(&ctx, org, "clean").await, [4]);

        let doubled = "SELECT k, v FROM r UNION ALL SELECT k, v FROM r";
        let blanks = "SELECT k, CASE WHEN k = 'a' THEN v ELSE ' ' END AS v FROM r";
        let renamed = "SELECT k, v AS other FROM r";
        let few = "SELECT k, v FROM r WHERE k = 'a'";
        for (checks, sql, check, value) in [
            ("{ min_rows: 5 }", all, "min_rows", "4"),
            ("{ max_rows: 3 }", all, "max_rows", "4"),
            ("{ unique: [k] }", doubled, "unique", "4"),
            ("{ unique: [gone] }", all, "unique", "-1"),
            ("{ max_empty_pct: { v: 50 } }", blanks, "max_empty_pct", "75"),
            ("{ max_empty_pct: { gone: 100 } }", all, "max_empty_pct", "100"),
            ("{ max_drop_pct: 50 }", few, "max_drop_pct", "75"),
            ("{ same_columns: true }", renamed, "same_columns", "k, other"),
        ] {
            let f = run_steps(&ctx, org, &clean(checks, sql))
                .await
                .failure
                .unwrap_or_else(|| panic!("{checks} passed"));
            assert_eq!(f.code, pnex_core::err_codes::PIPELINE_CHECK_FAILED, "{checks}");
            assert_eq!(f.args["check"], check, "{checks}: {}", f.args);
            assert_eq!(f.args["value"], value, "{checks}: {}", f.args);
            assert_eq!(committed(&ctx, org, "clean").await, [4], "{checks}");
        }
        worker.abort();
    })
    .await;
}

/// The table of an import is not written by a pipeline, and a refused run
/// leaves it as it is. A value declared erased never comes back in a table a
/// pipeline writes. A failed `sql` step does not repeat a cell (R4).
#[tokio::test]
#[serial]
async fn a_table_write_spares_import_tables_erasures_and_cells() {
    with_app(|server, ctx, token| async move {
        let worker = common::spawn_pipeline_worker();
        let org = first_org(&server, &token).await;
        let exec = |sql: &str, values: Vec<sea_orm::Value>| {
            let db = ctx.db.clone();
            let st = Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values);
            async move { db.execute_raw(st).await.unwrap() }
        };
        // An import left, until its conversion at boot (D1050).
        exec(
            "INSERT INTO datasets (org_id, name, import_id, retain_days) \
             VALUES ($1, 'imported', gen_random_uuid(), 30)",
            vec![org.into()],
        )
        .await;
        // An org's own pipeline: a pack's may feed its import.
        let yaml = "key: t\nsteps:\n- sql: { as: rows, sql: \"SELECT 'a' AS k\" }\n\
                    - write: { from: rows, table: { name: imported, replace: true } }\n";
        let doc = pnex_backend::services::pipelines::checked(yaml).unwrap();
        let net = Fixtures { calls: AtomicUsize::new(0) };
        let out = pnex_backend::services::pipeline_engine::store::run_own(
            &ctx,
            org,
            uuid::Uuid::new_v4(),
            1,
            &doc,
            None,
            Vec::new(),
            &net,
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.failure.map(|f| f.code), Some("table_reserved"));
        assert!(committed(&ctx, org, "imported").await.is_empty());

        let partners = "- sql: { as: rows, sql: \"SELECT '732829320' AS siren UNION ALL SELECT '552081317'\" }\n\
                        - write: { from: rows, table: { name: partners, replace: true } }\n";
        assert!(run_steps(&ctx, org, partners).await.failure.is_none());
        exec(
            "INSERT INTO dataset_erasures (org_id, dataset_id, column_name, value, type_key, done_at) \
             SELECT $1, id, 'siren', $2, 't', now() FROM datasets WHERE org_id = $1 AND name = 'partners'",
            vec![
                org.into(),
                pnex_core::datasets::fingerprint("732829320").unwrap().into(),
            ],
        )
        .await;
        assert!(run_steps(&ctx, org, partners).await.failure.is_none());
        assert_eq!(committed(&ctx, org, "partners").await, [2, 1]);

        let f = run_steps(
            &ctx,
            org,
            "- sql: { as: rows, tables: { p: partners }, sql: \"SELECT CAST(siren || 'x' AS BIGINT) AS n FROM p\" }\n\
             - write: { from: rows, table: { name: cast, replace: true } }\n",
        )
        .await
        .failure
        .expect("the cast fails");
        assert!(!f.args.to_string().contains("552081317"), "{}", f.args);
        worker.abort();
    })
    .await;
}
