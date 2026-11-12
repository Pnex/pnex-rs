//! The pipeline engine on a real base (pipeline-engine.md D937, D941): runs
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
