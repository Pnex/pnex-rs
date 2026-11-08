//! Primary market sources (markets-research.md D540-D542): imports of the
//! `markets` pack against a mock of SEC EDGAR and FRED, and the reading of a
//! value "as known at a date".
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.
//! The real sources: `markets_real_sources`, ignored by default.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::{json, Value};
use serial_test::serial;

const CONTACT: &str = "Test Lab test@example.com";

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

struct Api<'a> {
    server: &'a axum_test::TestServer,
    token: String,
    org: i64,
}

impl Api<'_> {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let req = match method {
            "GET" => self.server.get(path),
            "POST" => self.server.post(path),
            _ => self.server.patch(path),
        }
        .add_header("Authorization", format!("Bearer {}", self.token))
        .add_header("X-Org-Id", self.org.to_string());
        let res = match body {
            Some(b) => req.json(&b).await,
            None => req.await,
        };
        let (st, text) = (res.status_code().as_u16(), res.text());
        (
            st,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn run(&self, name: &str) -> Value {
        let (st, list) = self
            .call("GET", "/api/v1/ontology/imports?limit=50", None)
            .await;
        assert_eq!(st, 200, "{list}");
        let id = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {list}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, r) = self
            .call("POST", &format!("/api/v1/ontology/imports/{id}/runs"), None)
            .await;
        assert_eq!(st, 202, "{r}");
        let run = r["id"].as_str().unwrap().to_string();
        for _ in 0..1200 {
            let path = format!("/api/v1/ontology/imports/{id}/runs/{run}");
            let (_, r) = self.call("GET", &path, None).await;
            if r["run"]["state"] != "running" {
                return r;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("{name} never settled");
    }

    /// `run` with a long wait: a real download can take minutes.
    async fn run_long(&self, name: &str) -> (Value, std::time::Duration) {
        let started = std::time::Instant::now();
        let (_, list) = self
            .call("GET", "/api/v1/ontology/imports?limit=50", None)
            .await;
        let id = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {list}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, r) = self
            .call("POST", &format!("/api/v1/ontology/imports/{id}/runs"), None)
            .await;
        assert_eq!(st, 202, "{r}");
        let run = r["id"].as_str().unwrap().to_string();
        loop {
            let path = format!("/api/v1/ontology/imports/{id}/runs/{run}");
            let (_, r) = self.call("GET", &path, None).await;
            if r["run"]["state"] != "running" {
                return (r, started.elapsed());
            }
            assert!(started.elapsed().as_secs() < 3600, "{name} never settled");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    async fn settings(&self, values: Value) {
        let (st, r) = self
            .call(
                "PATCH",
                "/api/v1/ontology/packs/markets/settings",
                Some(json!({ "values": values })),
            )
            .await;
        assert_eq!(st, 200, "{r}");
    }

    async fn observations(&self, id: &str, query: &str) -> Vec<Value> {
        let path = format!("/api/v1/ontology/objects/{id}/observations?limit=500&{query}");
        let (st, r) = self.call("GET", &path, None).await;
        assert_eq!(st, 200, "{r}");
        r["results"].as_array().unwrap().clone()
    }
}

async fn object_id(
    db: &DatabaseConnection,
    org: i64,
    type_key: &str,
    prop: &str,
    v: &str,
) -> Option<String> {
    db.query_one_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!(
            "SELECT id::text AS id FROM objects WHERE org_id = {org} AND type_key = '{type_key}' \
             AND valid_to IS NULL AND properties ->> '{prop}' = '{v}'"
        ),
    ))
    .await
    .unwrap()
    .map(|r| r.try_get::<String>("", "id").unwrap())
}

/// EDGAR and FRED as they answer; the macro value is revised on the second
/// read. Records the `User-Agent` of each request.
async fn mock_sources(agents: Arc<Mutex<Vec<String>>>) -> String {
    use axum::http::HeaderMap;
    use axum::routing::get;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = move |h: &HeaderMap, a: &Arc<Mutex<Vec<String>>>| {
        let ua = h
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        a.lock().unwrap().push(ua.to_string());
    };
    let (a1, a2, a3, a4) = (agents.clone(), agents.clone(), agents.clone(), agents);
    let app = axum::Router::new()
        .route(
            "/files/company_tickers.json",
            get(move |h: HeaderMap| async move {
                seen(&h, &a1);
                axum::Json(json!({
                    "0": {"cik_str": 42, "ticker": "ACME", "title": "ACME CORP"},
                    "1": {"cik_str": 43, "ticker": "OTHER", "title": "OTHER CORP"}
                }))
            }),
        )
        .route(
            "/submissions/{file}",
            get(move |h: HeaderMap| async move {
                seen(&h, &a2);
                axum::Json(json!({
                    "cik": "0000000042",
                    "filings": {"recent": {
                        "accessionNumber": ["0000000042-26-000001", "0000000042-26-000002"],
                        "filingDate": ["2026-02-01", "2026-02-02"],
                        "reportDate": ["2025-12-31", ""],
                        "acceptanceDateTime": ["2026-02-01T21:00:00.000Z", ""],
                        "form": ["10-K", "4"],
                        "items": ["", ""],
                        "primaryDocument": ["acme-2025.htm", "x.xml"]
                    }}
                }))
            }),
        )
        .route(
            "/api/xbrl/companyfacts/{file}",
            get(move |h: HeaderMap| async move {
                seen(&h, &a3);
                axum::Json(
                    json!({"cik": 42, "facts": {"us-gaap": {"Assets": {"units": {"USD": [
                        {"end": "2025-12-31", "val": 100, "filed": "2026-02-01", "accn": "a1"},
                        {"end": "2025-12-31", "val": 100, "filed": "2026-05-01", "accn": "a2"},
                        {"end": "2025-12-31", "val": 120, "filed": "2026-08-01", "accn": "a3"}
                    ]}}}}}),
                )
            }),
        )
        .route(
            "/graph/fredgraph.csv",
            get(move |h: HeaderMap| {
                let reads = reads.clone();
                async move {
                    seen(&h, &a4);
                    let v = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                        "2.5"
                    } else {
                        "2.7"
                    };
                    format!("observation_date,TESTCPI\n2026-01-01,{v}\n2026-02-01,.\n")
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
#[serial]
async fn primary_sources_keep_what_was_known_when() {
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        assert_eq!(st, 200, "{r}");

        // No declared contact: refused, nothing read.
        let r = api.run("Companies followed (SEC)").await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        assert_eq!(r["run"]["state_code"], "import-contact-required", "{r}");

        api.settings(json!({
            "source_contact": CONTACT, "covered_tickers": "acme", "macro_series": "TESTCPI"
        }))
        .await;
        let agents = Arc::new(Mutex::new(Vec::new()));
        let base = mock_sources(agents.clone()).await;
        for host in ["https://www.sec.gov", "https://data.sec.gov", "https://fred.stlouisfed.org"] {
            db.execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "UPDATE object_imports SET source_url = replace(source_url, '{host}', '{base}') \
                     WHERE org_id = {org}"
                ),
            ))
            .await
            .unwrap();
        }

        // Only the followed ticker becomes a company.
        let r = api.run("Companies followed (SEC)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let acme = object_id(&db, org, "organization", "cik", "0000000042").await.expect("followed");
        assert!(object_id(&db, org, "organization", "cik", "0000000043").await.is_none());

        // Its result filing, with the document address; the insider form stays out.
        let r = api.run("Filings (SEC EDGAR)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let filing = object_id(&db, org, "filing", "accession", "0000000042-26-000001").await.expect("10-K");
        assert!(object_id(&db, org, "filing", "accession", "0000000042-26-000002").await.is_none());
        let (_, links) = api.call("GET", &format!("/api/v1/ontology/objects/{filing}/links"), None).await;
        assert!(links.to_string().contains("filed_by"), "{links}");

        // A figure restated in a later filing: each date reads its own truth.
        for _ in 0..2 {
            let r = api.run("Reported figures (SEC EDGAR)").await;
            assert_eq!(r["run"]["state"], "applied", "{r}");
        }
        let value = |rows: Vec<Value>| rows[0]["value"].as_f64();
        assert_eq!(value(api.observations(&acme, "").await), Some(120.0));
        let before = api.observations(&acme, "as_of=2026-03-01T00:00:00Z").await;
        assert_eq!(before.len(), 1, "{before:?}");
        assert_eq!(value(before), Some(100.0));
        // Not yet known during the day it was filed.
        assert!(api.observations(&acme, "as_of=2026-02-01T20:00:00Z").await.is_empty());
        let stored = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT count(*) AS n FROM market_observations WHERE org_id = {org}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(stored, 2, "the repeat and the second run add nothing");

        // A macro series has no publication date: flagged on first sight,
        // kept out of a past-dated reading; its revision is a real one.
        let r = api.run("Macroeconomic series (FRED)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let series = object_id(&db, org, "macro_series", "code", "TESTCPI").await.expect("created");
        let first = api.observations(&series, "").await;
        assert_eq!(first[0]["backfilled"], true, "{first:?}");
        let later = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(api.observations(&series, &format!("as_of={later}")).await.is_empty());
        api.run("Macroeconomic series (FRED)").await;
        let revised = api.observations(&series, &format!("as_of={later}")).await;
        assert_eq!(revised.len(), 1, "{revised:?}");
        assert_eq!(revised[0]["value"].as_f64(), Some(2.7));
        assert_eq!(revised[0]["backfilled"], false);

        // Every request declared the contact.
        let agents = agents.lock().unwrap().clone();
        assert!(!agents.is_empty() && agents.iter().all(|a| a.contains(CONTACT)), "{agents:?}");

        // Unknown object: not found.
        let (st, _) = api
            .call("GET", "/api/v1/ontology/objects/00000000-0000-0000-0000-000000000000/observations", None)
            .await;
        assert_eq!(st, 404);
    })
    .await;
}

/// Day `i` of the mock's prices.
fn price_day(i: usize) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap() + chrono::Duration::days(i as i64)
}

/// A daily market return, the same for every call.
fn market_return(i: usize) -> f64 {
    0.01 * (i as f64 * 1.7).sin()
}

/// The prices of a ticker: ACME moves twice the benchmark and jumps 5% on
/// day 300. Records the `Authorization` and the `startDate` it receives.
fn mock_prices(ticker: &str) -> Value {
    let mut close = 100.0;
    let bars: Vec<Value> = (0..340)
        .map(|i| {
            let mut r = market_return(i);
            if ticker.eq_ignore_ascii_case("ACME") {
                r = 2.0 * r + if i == 300 { 0.05 } else { 0.0 };
            }
            close *= 1.0 + r;
            // ACME trades six times its usual volume on the day of its jump.
            let volume = if ticker.eq_ignore_ascii_case("ACME") && i == 300 { 6000 } else { 1000 + 40 * (i % 5) };
            json!({"date": format!("{}T00:00:00.000Z", price_day(i)), "close": close, "volume": volume,
                   "divCash": 0.0, "splitFactor": 1.0, "adjClose": close / 2.0})
        })
        .collect();
    Value::Array(bars)
}

async fn mock_keyed_sources(seen: Arc<Mutex<Vec<(String, String)>>>) -> String {
    use axum::extract::{Path, RawQuery};
    use axum::http::HeaderMap;
    use axum::routing::get;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let filed = price_day(300).to_string();
    let app = axum::Router::new()
        .route(
            "/tiingo/daily/{ticker}/prices",
            get(move |Path(t): Path<String>, h: HeaderMap, RawQuery(q): RawQuery| async move {
                let auth = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
                seen.lock().unwrap().push((auth.to_string(), q.unwrap_or_default()));
                axum::Json(mock_prices(&t))
            }),
        )
        .route(
            "/files/company_tickers.json",
            get(|| async { axum::Json(json!({"0": {"cik_str": 42, "ticker": "ACME", "title": "ACME CORP"}})) }),
        )
        .route(
            "/submissions/{file}",
            get(move || {
                let filed = filed.clone();
                async move {
                    axum::Json(json!({"cik": "42", "filings": {"recent": {
                        "accessionNumber": ["0000000042-25-000001", "0000000042-25-000002"],
                        "filingDate": [filed, "2025-01-05"],
                        "reportDate": ["", ""],
                        "acceptanceDateTime": ["", ""],
                        "form": ["8-K", "8-K"],
                        "items": ["2.02,9.01", "5.02"],
                        "primaryDocument": ["a.htm", "b.htm"]
                    }}}))
                }
            }),
        )
        .route(
            "/service/data/FM/{key}",
            get(|Path(key): Path<String>| async move {
                format!("KEY,TIME_PERIOD,OBS_VALUE\nFM.{key},2026-09,2.15\n")
            }),
        )
        .route(
            "/api/filings",
            get(|| async {
                axum::Json(json!({
                    "data": [{"attributes": {"fxo_id": "LEITEST-2025", "published": "2026-03-01 10:00:00",
                        "period_end": "2025-12-31", "report_url": "/x/report.xhtml"}}],
                    "included": [{"type": "entity", "attributes": {"name": "EURO SA"}}]
                }))
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
#[serial]
async fn prices_with_a_vault_key_and_the_reaction_to_a_filing() {
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        pnex_backend::services::markets::init(&ctx);
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({
            "source_contact": CONTACT, "covered_tickers": "ACME", "benchmark_ticker": "SPY",
            "ecb_series": "FM.D.U2.EUR", "covered_leis": "LEITEST",
            "move_tickers": "ACME:volume:equity_index"
        }))
        .await;

        // A keyed import pointed at another host never gets the key.
        let r = api.run("Daily prices (Tiingo)").await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_keyed_sources(seen.clone()).await;
        let sql = |q: String| {
            db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, q))
        };
        for host in ["https://www.sec.gov", "https://data.sec.gov", "https://api.tiingo.com",
                     "https://data-api.ecb.europa.eu", "https://filings.xbrl.org"] {
            sql(format!("UPDATE object_imports SET source_url = replace(source_url, '{host}', '{base}') WHERE org_id = {org}"))
                .await
                .unwrap();
        }
        let r = api.run("Daily prices (Tiingo)").await;
        assert_eq!(r["run"]["state_code"], "import-key-host", "a mock host is not the key's: {r}");
        assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
        pnex_backend::services::markets::allow_test_host(&base);

        // No key in the vault: refused, with a code the UI can explain.
        let r = api.run("Benchmark prices (Tiingo)").await;
        assert_eq!(r["run"]["state_code"], "import-key-required", "{r}");
        let (st, r) = api
            .call("POST", "/api/v1/secrets", Some(json!({"name": "markets_tiingo_key", "value": "k-123"})))
            .await;
        assert_eq!(st, 201, "{r}");

        for name in ["Companies followed (SEC)", "Filings (SEC EDGAR)", "Benchmark prices (Tiingo)", "Daily prices (Tiingo)"] {
            let r = api.run(name).await;
            assert_eq!(r["run"]["state"], "applied", "{name}: {r}");
        }
        let calls = seen.lock().unwrap().clone();
        assert!(calls.iter().all(|(a, _)| a == "Token k-123"), "{calls:?}");
        // The benchmark is an instrument too: read in turn with the others, only
        // the new one from the start.
        let whole = calls.iter().filter(|(_, q)| q == "startDate=2000-01-01").count();
        assert_eq!(whole, 2, "first read of each instrument: {calls:?}");
        let last = "ACME";
        let acme = object_id(&db, org, "instrument", "ticker", last).await.expect("instrument");
        // D810: the volume spike is a move of measure `volume`, the only one.
        let spike = format!("move:ACME:{}:volume", price_day(300));
        let volume_moves = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event' \
                 AND properties ->> 'mode' = 'volume'"
            ),
        )
        .await;
        assert_eq!(volume_moves, 1, "one volume move");
        assert!(object_id(&db, org, "market_event", "ref", &spike).await.is_some(), "{spike}");

        // A daily read asks only for the recent days.
        api.run("Daily prices (Tiingo)").await;
        let (_, q) = seen.lock().unwrap().last().unwrap().clone();
        assert_eq!(q, format!("startDate={}", price_day(339) - chrono::Duration::days(7)));

        // The raw close is stored, not the adjusted one.
        let (_, page) = api
            .call("GET", &format!("/api/v1/ontology/objects/{acme}/observations?metric=close"), None)
            .await;
        assert_eq!(page["count"], 340, "{page}");
        let closes = page["results"].as_array().unwrap();
        assert_eq!(closes[0]["known_at"], format!("{}T23:59:59Z", price_day(339)));

        // The known jump and beta come back, the day after the jump's return.
        let date = price_day(300);
        let (st, study) = api
            .call("GET", &format!("/api/v1/markets/instruments/{acme}/event-study?date={date}"), None)
            .await;
        assert_eq!(st, 200, "{study}");
        assert!((study["beta"].as_f64().unwrap() - 2.0).abs() < 1e-6, "{study}");
        let window = study["windows"].as_array().unwrap().iter().find(|w| w["from"] == -1).unwrap();
        assert!((window["abnormal"].as_f64().unwrap() - 0.05).abs() < 1e-6, "{window}");

        // A rule: results filings, not the management change, against drawn dates.
        let (st, rule) = api.call("GET", "/api/v1/markets/rule-test?form=8-K&item=2.02", None).await;
        assert_eq!(st, 200, "{rule}");
        assert_eq!(rule["filings"], 1, "{rule}");
        assert_eq!(rule["instruments"], 1, "{rule}");
        let w = &rule["windows"][0];
        assert_eq!(w["events"]["count"], 1, "{rule}");
        assert!(w["baseline"]["count"].as_u64().unwrap() > 1, "{rule}");
        assert!(w["events"]["mean"].as_f64().unwrap() > 0.04, "{rule}");
        let (_, none) = api.call("GET", "/api/v1/markets/rule-test?form=8-K&item=1.01", None).await;
        assert_eq!(none["filings"], 0, "{none}");
        let (st, _) = api.call("GET", "/api/v1/markets/rule-test?form=8-K&from=soon", None).await;
        assert_eq!(st, 400);

        // A long first read stays flagged past the first write batch.
        let long: Vec<_> = (0..2500)
            .map(|i| pnex_core::markets::PlannedObservation {
                key: "FM.D.U2.EUR".into(),
                metric: "LONG".into(),
                unit: String::new(),
                period_start: (price_day(0) + chrono::Duration::days(i)).to_string(),
                period_end: (price_day(0) + chrono::Duration::days(i)).to_string(),
                known_on: None,
                value: 1.0,
                source: None,
            })
            .collect();
        api.run("European series (ECB)").await;
        let target = pnex_core::markets::ObservationTarget {
            type_key: "macro_series".into(),
            property: "code".into(),
        };
        pnex_backend::services::markets::store(&db, org, &target, &long, "test").await.unwrap();
        let flagged = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT count(*) FILTER (WHERE backfilled) AS f, count(*) AS n FROM market_observations WHERE org_id = {org} AND metric = 'LONG'"),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(flagged.try_get::<i64>("", "n").unwrap(), 2500);
        assert_eq!(flagged.try_get::<i64>("", "f").unwrap(), 2500, "the whole first read");

        // A thesis: a fictitious position is measured on the closes.
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/investment_thesis/install", None).await;
        assert_eq!(st, 200, "{r}");
        let (st, pos) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "paper_position", "title": "ACME long",
                    "properties": {"side": "long", "quantity": 10, "entry_on": price_day(200).to_string()}})),
            )
            .await;
        assert_eq!(st, 201, "{pos}");
        let pid = pos["id"].as_str().unwrap();
        let (_, none) = api.call("GET", &format!("/api/v1/markets/positions/{pid}/result"), None).await;
        assert_eq!(none, Value::Null, "no instrument yet");
        let (st, l) = api
            .call(
                "POST",
                "/api/v1/ontology/links",
                Some(json!({"link_type": "position_on", "source_id": pid, "target_id": acme})),
            )
            .await;
        assert_eq!(st, 201, "{l}");
        let (st, res) = api.call("GET", &format!("/api/v1/markets/positions/{pid}/result"), None).await;
        assert_eq!(st, 200, "{res}");
        assert_eq!(res["entry_on"], price_day(200).to_string(), "{res}");
        assert_eq!(res["open"], true);
        let expected = res["last_close"].as_f64().unwrap() / res["entry_close"].as_f64().unwrap() - 1.0;
        assert!((res["total_return"].as_f64().unwrap() - expected).abs() < 1e-6, "{res}");
        let (st, _) = api.call("GET", &format!("/api/v1/markets/positions/{acme}/result"), None).await;
        assert_eq!(st, 404, "only a position");

        // European sources: the ECB code becomes a path, a report its filing.
        let r = api.run("European series (ECB)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let series = object_id(&db, org, "macro_series", "code", "FM.D.U2.EUR").await.expect("series");
        assert_eq!(api.observations(&series, "metric=FM.D.U2.EUR").await[0]["period_end"], "2026-09-01");
        let r = api.run("European reports (ESEF)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let f = object_id(&db, org, "filing", "accession", "LEITEST-2025").await;
        assert!(f.is_some(), "{r}");
        assert!(object_id(&db, org, "organization", "lei", "LEITEST").await.is_some());
    })
    .await;
}

/// The real SEC and FRED, by hand: `MARKETS_CONTACT="Name mail" cargo test
/// -p pnex-backend --test markets -- --ignored`.
#[tokio::test]
#[serial]
#[ignore = "reaches the real SEC and FRED"]
async fn markets_real_sources() {
    let contact = std::env::var("MARKETS_CONTACT").expect("MARKETS_CONTACT");
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        api.settings(json!({
            "source_contact": contact, "covered_tickers": "NVDA", "macro_series": "GDPC1, DGS10",
            "covered_leis": std::env::var("MARKETS_LEI").unwrap_or_default(),
        }))
        .await;
        for name in [
            "Companies followed (SEC)",
            "Filings (SEC EDGAR)",
            "Reported figures (SEC EDGAR)",
            "Macroeconomic series (FRED)",
            "European series (ECB)",
            "European reports (ESEF)",
            "Press leads (GDELT)",
        ] {
            let started = std::time::Instant::now();
            let r = api.run(name).await;
            println!("{name}: {} in {:?}: {}", r["run"]["state"], started.elapsed(), r["run"]["counts"]);
            assert_eq!(r["run"]["state"], "applied", "{r}");
        }
        let nvda = object_id(&db, org, "organization", "ticker", "NVDA").await.expect("NVDA");
        let now = api.observations(&nvda, "metric=RevenueFromContractWithCustomerExcludingAssessedTax").await;
        println!("{} revenue values, latest {}", now.len(), now[0]);
        let past = api
            .observations(&nvda, "metric=RevenueFromContractWithCustomerExcludingAssessedTax&as_of=2024-01-01T00:00:00Z")
            .await;
        println!("as of 2024-01-01: {} values, latest {}", past.len(), past[0]);
        assert!(past.iter().all(|o| o["known_at"].as_str().unwrap() < "2024-01-01"), "no look-ahead");
        let gdp = object_id(&db, org, "macro_series", "code", "GDPC1").await.expect("GDP");
        let rows = api.observations(&gdp, "").await;
        println!("GDPC1: {} values, latest {}", rows.len(), rows[0]);
        // One sweep of the documents: the text is kept, indexed and linked.
        pnex_backend::services::markets::documents::sweep(&ctx).await.unwrap();
        let (st, media) = api.call("GET", "/api/v1/media?limit=50", None).await;
        println!("media {st}: {} documents", media["count"]);
        let linked = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} \
                     AND relation = 'filing_document'"
                ),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        println!("{linked} filings linked to their document");
        assert!(linked > 0, "{media}");
        // Indexed by the document search: the job runs inline here.
        let version = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT media_version_id AS v, status FROM media_text_index \
                     WHERE org_id = {org} LIMIT 1"
                ),
            ))
            .await
            .unwrap()
            .expect("queued for indexing");
        let id: uuid::Uuid = version.try_get("", "v").unwrap();
        println!("queued as {}", version.try_get::<String>("", "status").unwrap());
        pnex_backend::services::doc_search::index_version(&ctx, org, id).await.unwrap();
        let (status, chars) = {
            let r = db
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT status, (SELECT count(*) FROM text_chunks c WHERE c.media_version_id = '{id}') AS n FROM media_text_index WHERE media_version_id = '{id}'"),
                ))
                .await
                .unwrap()
                .unwrap();
            (r.try_get::<String>("", "status").unwrap(), r.try_get::<i64>("", "n").unwrap())
        };
        println!("indexed: {status}, {chars} chunks");
        assert_eq!(status, "indexed");
        assert!(chars > 0);
    })
    .await;
}

/// A watched index as FRED answers it: 150 quiet days with a crash on day
/// 120; from the second reading on, one more day, a second crash.
async fn mock_move_series() -> String {
    use axum::{routing::get, Router};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let reads = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/graph/fredgraph.csv",
        get(move || {
            let reads = reads.clone();
            async move {
                let days = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    150
                } else {
                    151
                };
                let mut csv = String::from("observation_date,TESTIDX\n");
                let mut p = 100.0f64;
                for i in 0..days {
                    if i > 0 {
                        p *= if i == 120 || i == 150 {
                            0.94
                        } else if i % 2 == 0 {
                            1.004
                        } else {
                            0.996
                        };
                    }
                    csv.push_str(&format!("{},{p:.4}\n", move_day(i)));
                }
                csv
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn move_day(i: usize) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i as i64)
}

async fn count(db: &DatabaseConnection, sql: String) -> i64 {
    db.query_one_raw(Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        sql,
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "n")
    .unwrap()
}

/// D734-D743: a crash of a watched series becomes one market event, caught
/// up from history on the first reading and new on the next, never twice;
/// the session lists it with the world events of its window; an attribution
/// needs a case; a rule on world event dates measures one series.
#[tokio::test]
#[serial]
async fn abnormal_sessions_and_their_window() {
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api {
            server: &server,
            token,
            org,
        };
        for pack in ["markets", "world_events", "market_attribution"] {
            let (st, r) = api
                .call(
                    "POST",
                    &format!("/api/v1/ontology/packs/{pack}/install"),
                    None,
                )
                .await;
            assert_eq!(st, 200, "{pack}: {r}");
        }
        api.settings(json!({
            "source_contact": CONTACT, "move_series": "testidx:log_return:equity_index"
        }))
        .await;
        let base = mock_move_series().await;
        db.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "UPDATE object_imports SET source_url = replace(source_url, \
                 'https://fred.stlouisfed.org', '{base}') WHERE org_id = {org}"
            ),
        ))
        .await
        .unwrap();
        let events = format!(
            "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event'"
        );

        // First reading: the old crash is found, caught up from history.
        let r = api.run("Watched series (FRED)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(count(&db, events.clone()).await, 1);
        let old = object_id(
            &db,
            org,
            "market_event",
            "event_at",
            &move_day(120).to_string(),
        )
        .await
        .expect("crash of day 120");
        let (_, o) = api
            .call("GET", &format!("/api/v1/ontology/objects/{old}"), None)
            .await;
        let p = &o["properties"];
        assert_eq!(p["kind"], "market_move", "{o}");
        assert_eq!(p["move_direction"], "down");
        assert_eq!(p["asset_class"], "equity_index");
        assert_eq!(p["series_code"], "TESTIDX");
        assert_eq!(p["backfilled"], true);
        let (_, links) = api
            .call(
                "GET",
                &format!("/api/v1/ontology/objects/{old}/links"),
                None,
            )
            .await;
        assert!(links.to_string().contains("event_about"), "{links}");

        // Next reading: the new crash is news; a third adds nothing.
        api.run("Watched series (FRED)").await;
        api.run("Watched series (FRED)").await;
        assert_eq!(count(&db, events).await, 2);
        let day = move_day(150).to_string();
        let new = object_id(&db, org, "market_event", "event_at", &day)
            .await
            .expect("new crash");
        let (_, o) = api
            .call("GET", &format!("/api/v1/ontology/objects/{new}"), None)
            .await;
        assert_eq!(o["properties"]["backfilled"], false, "{o}");

        // A confirmed world event the same day, and a dismissed one: the
        // session shows the first only.
        for (status, title) in [("confirmed", "Strike"), ("dismissed", "Noise")] {
            let (st, r) = api
                .call(
                    "POST",
                    "/api/v1/ontology/objects",
                    Some(json!({
                        "type_key": "geo_event",
                        "title": title,
                        "properties": {
                            "ref": format!("manual:{title}"),
                            "kind": "attack",
                            "event_at": day,
                            "known_at": format!("{day}T12:00:00Z"),
                            "country": "UP",
                            "status": status,
                            "source_kind": "manual",
                        }
                    })),
                )
                .await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, s) = api
            .call("GET", &format!("/api/v1/markets/sessions/{day}"), None)
            .await;
        assert_eq!(st, 200, "{s}");
        assert_eq!(s["moves"][0]["series_code"], "TESTIDX", "{s}");
        assert_eq!(s["moves"][0]["event_at"], day);
        let world = s["world_events"].as_array().unwrap();
        assert_eq!(world.len(), 1, "{s}");
        assert_eq!(world[0]["status"], "confirmed");
        let strike = world[0]["id"].as_str().unwrap().to_string();
        let (st, _) = api
            .call("GET", "/api/v1/markets/sessions/not-a-day", None)
            .await;
        assert_eq!(st, 400);

        // Outside a case, no attribution.
        let (st, r) = api
            .call(
                "POST",
                "/api/v1/ontology/links",
                Some(json!({
                    "link_type": "attributed_to",
                    "source_id": new,
                    "target_id": strike,
                    "attributes": { "justification": "Same day", "source": "https://example.com/a" }
                })),
            )
            .await;
        assert_eq!(
            (st, r["error"].as_str()),
            (409, Some("ontology-link-member-only")),
            "{r}"
        );
        // From a case holding the move, a member attributes it.
        let (st, case) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "case", "title": "Crash of day 150",
                            "properties": {"status": "open", "question": "Why?"}})),
            )
            .await;
        assert_eq!(st, 201, "{case}");
        let cid = case["id"].as_str().unwrap();
        let (st, w) = api
            .call(
                "POST",
                &format!("/api/v1/cases/{cid}/attach"),
                Some(json!({"object_id": new})),
            )
            .await;
        assert_eq!(st, 200, "{w}");
        let (st, r) = api
            .call(
                "POST",
                "/api/v1/ontology/links",
                Some(json!({
                    "link_type": "attributed_to",
                    "source_id": new,
                    "target_id": strike,
                    "attributes": { "justification": "Same day", "source": "https://example.com/a" }
                })),
            )
            .await;
        assert_eq!(st, 201, "{r}");

        // The series after confirmed world events, against drawn dates.
        let (st, r) = api
            .call(
                "GET",
                "/api/v1/markets/rule-test?dates=geo_events&series=TESTIDX",
                None,
            )
            .await;
        assert_eq!(st, 200, "{r}");
        assert_eq!(r["filings"], 1, "{r}");
        assert_eq!(r["merged"], 0);
        let (st, _) = api
            .call("GET", "/api/v1/markets/rule-test?dates=geo_events", None)
            .await;
        assert_eq!(st, 400);
        let (st, _) = api
            .call(
                "GET",
                "/api/v1/markets/rule-test?dates=geo_events&series=NOPE",
                None,
            )
            .await;
        assert_eq!(st, 400);
    })
    .await;
}

/// UCDP and ACLED as they answer, with what each checks of the caller.
async fn mock_conflicts(seen: Arc<Mutex<Vec<String>>>) -> String {
    use axum::http::HeaderMap;
    use axum::{
        routing::{get, post},
        Router,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let day = (chrono::Utc::now().date_naive() - chrono::Duration::days(3)).to_string();
    let (d1, d2) = (day.clone(), day.clone());
    // GED as UCDP publishes it: a zipped CSV, a quoted field over two lines.
    let ged = {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file(
            "GEDEvent_v26_1.csv",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        w.write_all(
            b"id,country_id,date_start,type_of_violence,source_headline,latitude,longitude,best,number_of_sources,source_date\n\
              9001,369,2015-02-01 00:00:00.000,3,\"Shelling,\nDonetsk\",48.0,37.8,5,2,2015-02-03;2015-02-02\n\
              9002,365,2015-02-01 00:00:00.000,1,x,55.0,37.0,1,1,2015-02-01\n",
        )
        .unwrap();
        w.finish().unwrap().into_inner()
    };
    let (s1, s2, s3) = (seen.clone(), seen.clone(), seen);
    let app = Router::new()
        .route(
            "/downloads/ged/ged261-csv.zip",
            get(move || {
                let ged = ged.clone();
                async move { ged }
            }),
        )
        .route(
            "/api/gedevents/26.0.9",
            get(move |h: HeaderMap, q: axum::extract::RawQuery| {
                let d = d1.clone();
                let s = s1.clone();
                async move {
                    let token = h.get("x-ucdp-access-token").and_then(|v| v.to_str().ok());
                    s.lock().unwrap().push(format!("ucdp:{}", token.unwrap_or("")));
                    // UCDP's pages count from 0: any other page is out of range.
                    if !q.0.unwrap_or_default().contains("page=0") {
                        return axum::Json(json!({"TotalPages": 1, "Result": []}));
                    }
                    axum::Json(json!({"TotalPages": 1, "Result": [
                        {"id": 7001, "country_id": 369, "date_start": format!("{d} 00:00:00.000"),
                         "type_of_violence": 1, "where_coordinates": "Kharkiv city", "adm_1": "Kharkiv oblast",
                         "latitude": 49.99, "longitude": 36.23, "side_a": "Government of Russia (Soviet Union)",
                         "side_b": "Government of Ukraine", "number_of_sources": 3, "best": 4},
                        {"id": 7002, "country_id": 365, "date_start": format!("{d} 00:00:00.000"), "type_of_violence": 1}
                    ]}))
                }
            }),
        )
        .route(
            "/oauth/token",
            post(move |body: String| {
                let s = s2.clone();
                async move {
                    let ok = body.contains("username=ops%40example.com") && body.contains("password=pw");
                    s.lock().unwrap().push(format!("token:{ok}"));
                    axum::Json(json!({"access_token": if ok { "tk" } else { "" }, "expires_in": 86400}))
                }
            }),
        )
        .route(
            "/api/acled/read",
            get(move |h: HeaderMap| {
                let d = d2.clone();
                let s = s3.clone();
                async move {
                    let auth = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
                    s.lock().unwrap().push(format!("acled:{auth}"));
                    axum::Json(json!({"status": 200, "data": [
                        {"event_id_cnty": "UKR9", "event_date": d, "event_type": "Explosions/Remote violence",
                         "sub_event_type": "Air/drone strike", "iso": 804, "admin1": "Kyiv", "location": "Kyiv",
                         "latitude": "50.45", "longitude": "30.52", "actor1": "Military Forces of Russia",
                         "fatalities": "2", "timestamp": "1762300800"}
                    ]}))
                }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// D739, D741: a GDELT lead is confirmed by a UCDP event of its country and
/// day; ACLED is read with the org's own account; both reach the map.
/// Without credentials, nothing is read.
#[tokio::test]
#[serial]
async fn verified_events_confirm_leads_and_reach_the_map() {
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api {
            server: &server,
            token,
            org,
        };
        let (st, r) = api
            .call("POST", "/api/v1/ontology/packs/world_events/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        let (st, r) = api
            .call(
                "PATCH",
                "/api/v1/ontology/packs/world_events/settings",
                Some(json!({ "values": { "geo_watch": "ua" } })),
            )
            .await;
        assert_eq!(st, 200, "{r}");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let base = mock_conflicts(seen.clone()).await;
        pnex_backend::services::markets::allow_test_host(&base);
        for host in [
            "https://ucdpapi.pcr.uu.se",
            "https://ucdp.uu.se",
            "https://acleddata.com",
        ] {
            db.execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "UPDATE object_imports SET source_url = replace(source_url, '{host}', '{base}') \
                     WHERE org_id = {org}"
                ),
            ))
            .await
            .unwrap();
        }

        // The GED download needs no token: the history of the watched
        // country, dated by its first report.
        let r = api.run("Verified events history (UCDP GED download)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let old = object_id(&db, org, "geo_event", "ref", "ucdp:UA:attack:2015-02-01")
            .await
            .expect("2015 history");
        assert!(object_id(&db, org, "geo_event", "ref", "ucdp:RU:armed_conflict:2015-02-01")
            .await
            .is_none());
        let (_, o) = api
            .call("GET", &format!("/api/v1/ontology/objects/{old}"), None)
            .await;
        assert_eq!(o["properties"]["known_at"], "2015-02-02T23:59:59Z", "{o}");
        assert_eq!(o["properties"]["deaths"], 5.0);

        // No token in the vault: refused, nothing sent.
        let r = api.run("Verified events (UCDP)").await;
        assert_eq!(r["run"]["state_code"], "import-key-required", "{r}");
        assert!(seen.lock().unwrap().is_empty());

        // A GDELT lead of the day before, unconfirmed.
        let day = chrono::Utc::now().date_naive() - chrono::Duration::days(3);
        let (st, r) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({
                    "type_key": "geo_event",
                    "title": "Kharkiv lead",
                    "properties": {
                        "ref": "gdelt:UA:19:x", "kind": "attack", "country": "UA",
                        "event_at": (day - chrono::Duration::days(1)).to_string(),
                        "source_kind": "gdelt", "status": "unconfirmed"
                    }
                })),
            )
            .await;
        assert_eq!(st, 201, "{r}");
        let lead = r["id"].as_str().unwrap().to_string();

        for (name, value) in [
            ("world_events_ucdp_token", "u-123"),
            ("world_events_acled_email", "ops@example.com"),
            ("world_events_acled_password", "pw"),
        ] {
            let (st, r) = api
                .call("POST", "/api/v1/secrets", Some(json!({"name": name, "value": value})))
                .await;
            assert!(st == 200 || st == 201, "{r}");
        }
        let r = api.run("Verified events (UCDP)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        // Only the watched country, confirmed, and the lead with it.
        let ucdp_ref = format!("ucdp:UA:armed_conflict:{day}");
        let ucdp = object_id(&db, org, "geo_event", "ref", &ucdp_ref)
            .await
            .expect("UA event");
        assert!(
            object_id(&db, org, "geo_event", "ref", &format!("ucdp:RU:armed_conflict:{day}"))
                .await
                .is_none()
        );
        let (_, o) = api
            .call("GET", &format!("/api/v1/ontology/objects/{ucdp}"), None)
            .await;
        assert_eq!(o["properties"]["status"], "confirmed", "{o}");
        assert_eq!(o["properties"]["country"], "UA");
        let (_, o) = api
            .call("GET", &format!("/api/v1/ontology/objects/{lead}"), None)
            .await;
        assert_eq!(o["properties"]["status"], "confirmed", "{o}");
        assert_eq!(o["properties"]["confirmed_with"], ucdp_ref.as_str());

        let r = api.run("Verified events (ACLED, your account)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert!(
            object_id(&db, org, "geo_event", "ref", &format!("acled:UA:attack:{day}"))
                .await
                .is_some()
        );
        let seen = seen.lock().unwrap().clone();
        assert!(seen.contains(&"ucdp:u-123".to_string()), "{seen:?}");
        assert!(seen.contains(&"token:true".to_string()), "{seen:?}");
        assert!(seen.contains(&"acled:Bearer tk".to_string()), "{seen:?}");

        // The map: both verified events are in a box around Ukraine, the
        // lead has no location; a box elsewhere is empty.
        let (st, m) = api
            .call(
                "GET",
                "/api/v1/markets/world-events?west=22&south=44&east=41&north=53",
                None,
            )
            .await;
        assert_eq!(st, 200, "{m}");
        assert_eq!(m["count"], 2, "{m}");
        let (_, m) = api
            .call(
                "GET",
                "/api/v1/markets/world-events?west=0&south=0&east=1&north=1",
                None,
            )
            .await;
        assert_eq!(m["count"], 0, "{m}");
        let (st, _) = api
            .call(
                "GET",
                "/api/v1/markets/world-events?west=0&south=0&east=500&north=1",
                None,
            )
            .await;
        assert_eq!(st, 400);
    })
    .await;
}

/// The real sources of the market moves and world events (D734-D743), by
/// hand: `MARKETS_CONTACT="Name mail" WORLD_COUNTRIES="UA,IL" cargo test
/// --test markets world_events_real_sources -- --ignored --nocapture`.
/// Prints what was read and how long each step took.
#[tokio::test]
#[serial]
#[ignore = "reaches the real FRED, GDELT and UCDP"]
async fn world_events_real_sources() {
    let contact = std::env::var("MARKETS_CONTACT").expect("MARKETS_CONTACT");
    let countries =
        std::env::var("WORLD_COUNTRIES").unwrap_or_else(|_| "UA,IL,IR,RU,SY,IQ,YE,SD,CD,AF".into());
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        for pack in ["markets", "world_events"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{pack}: {r}");
        }
        // The acceptance session of MK7 needs the indices: private test only.
        api.settings(json!({
            "source_contact": contact,
            "move_series": "NASDAQCOM:log_return:equity_index, SP500:log_return:equity_index, \
                VIXCLS:diff:volatility, DGS10:diff:rates, DCOILWTICO:log_return:commodity, \
                DCOILBRENTEU:log_return:commodity, DTWEXBGS:log_return:fx",
        }))
        .await;
        let (st, r) = api
            .call(
                "PATCH",
                "/api/v1/ontology/packs/world_events/settings",
                Some(json!({ "values": { "geo_watch": countries } })),
            )
            .await;
        assert_eq!(st, 200, "{r}");
        let count = |sql: String| {
            let db = db.clone();
            async move { super_count(&db, sql).await }
        };
        for name in [
            "Watched series (FRED)",
            "World events (GDELT)",
            "Verified events history (UCDP GED download)",
            "Verified events this year (UCDP Candidate download)",
        ] {
            let (r, took) = api.run_long(name).await;
            println!("{name}: {} in {took:?}: {}", r["run"]["state"], r["run"]["counts"]);
            assert_eq!(r["run"]["state"], "applied", "{r}");
        }
        let moves = count(format!(
            "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event'"
        ))
        .await;
        let fresh = count(format!(
            "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event' \
             AND properties ->> 'backfilled' = 'false'"
        ))
        .await;
        println!("market moves: {moves} ({fresh} not caught up from history)");
        for (label, filter) in [
            ("leads", "properties ->> 'source_kind' = 'gdelt'"),
            ("confirmed leads", "properties ->> 'source_kind' = 'gdelt' AND properties ->> 'status' = 'confirmed'"),
            ("UCDP days", "properties ->> 'source_kind' = 'ucdp'"),
            ("UCDP days with known_at", "properties ->> 'source_kind' = 'ucdp' AND properties ->> 'known_at' IS NOT NULL"),
        ] {
            let n = count(format!(
                "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'geo_event' AND {filter}"
            ))
            .await;
            println!("{label}: {n}");
        }
        for day in ["2026-11-05", "2026-11-04"] {
            let (st, s) = api.call("GET", &format!("/api/v1/markets/sessions/{day}"), None).await;
            assert_eq!(st, 200, "{s}");
            let moves: Vec<String> = s["moves"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| format!("{} {} {:.4} ({:.1} sigma)", m["event_at"], m["series_code"], m["measure"].as_f64().unwrap_or(0.0), m["score"].as_f64().unwrap_or(0.0)))
                .collect();
            println!("session {day}: {moves:?}; {} world events in the window", s["world_events"].as_array().unwrap().len());
        }
        let (st, m) = api
            .call("GET", "/api/v1/markets/world-events?west=-180&south=-90&east=180&north=90&limit=300", None)
            .await;
        assert_eq!(st, 200, "{m}");
        println!("map, 30 days, world: {} events", m["count"]);
        for q in [
            "dates=geo_events&series=DCOILWTICO",
            "dates=geo_events&series=DCOILWTICO&include_unconfirmed=true",
            "dates=geo_events&series=VIXCLS&kind=armed_conflict&country=UA",
            "dates=market_moves&series=DCOILBRENTEU&asset_class=equity_index&direction=down",
        ] {
            let started = std::time::Instant::now();
            let (st, r) = api.call("GET", &format!("/api/v1/markets/rule-test?{q}"), None).await;
            assert_eq!(st, 200, "{q}: {r}");
            let w = &r["windows"][1];
            println!(
                "rule {q}: {} dates, {} merged, unconfirmed {}, [0,5] events {} baseline {} in {:?}",
                r["filings"], r["merged"], r["unconfirmed_share"], w["events"], w["baseline"], started.elapsed()
            );
        }
    })
    .await;
}

async fn super_count(db: &DatabaseConnection, sql: String) -> i64 {
    count(db, sql).await
}

/// Volume run of the world events (D739-D743): every country (`*`), the whole
/// UCDP history, three months of GDELT, then the reads people make. By hand,
/// with an O2 of its own (never the shared one: its org names would collide):
/// `MARKETS_CONTACT=… PNEX_O2_URL=http://127.0.0.1:5091 PNEX_O2_ROOT_PASSWORD=…
/// cargo test --test markets world_events_volume -- --ignored --nocapture`.
#[tokio::test]
#[serial]
#[ignore = "downloads gigabytes and writes hundreds of thousands of objects"]
async fn world_events_volume() {
    let contact = std::env::var("MARKETS_CONTACT").expect("MARKETS_CONTACT");
    with_app(|server, ctx, token| async move {
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        println!("org {org}");
        let api = Api { server: &server, token, org };
        for pack in ["markets", "world_events"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{pack}: {r}");
        }
        api.settings(json!({ "source_contact": contact })).await;
        let (st, r) = api
            .call(
                "PATCH",
                "/api/v1/ontology/packs/world_events/settings",
                Some(json!({ "values": { "geo_watch": "*", "gdelt_codes": "18, 19, 20, 163, 191, 17" } })),
            )
            .await;
        assert_eq!(st, 200, "{r}");
        let size = |what: &'static str| {
            let db = db.clone();
            async move {
                let sql = format!(
                    "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'geo_event' {what}"
                );
                count(&db, sql).await
            }
        };
        for name in [
            "Watched series (FRED)",
            "Verified events history (UCDP GED download)",
            "Verified events this year (UCDP Candidate download)",
            "World events history (GDELT, 3 months)",
            "World events (GDELT)",
        ] {
            let (r, took) = api.run_long(name).await;
            println!(
                "{name}: {} in {took:?}: {} — geo_event now {}",
                r["run"]["state"],
                r["run"]["counts"],
                size("").await
            );
            assert_eq!(r["run"]["state"], "applied", "{r}");
        }
        for (label, what) in [
            ("leads", "AND properties ->> 'source_kind' = 'gdelt'"),
            ("leads confirmed", "AND properties ->> 'source_kind' = 'gdelt' AND properties ->> 'status' = 'confirmed'"),
            ("leads Aug-Sep 2026", "AND properties ->> 'source_kind' = 'gdelt' AND properties ->> 'event_at' BETWEEN '2026-08-01' AND '2026-09-30'"),
            ("leads Aug-Sep 2026 confirmed", "AND properties ->> 'source_kind' = 'gdelt' AND properties ->> 'status' = 'confirmed' AND properties ->> 'event_at' BETWEEN '2026-08-01' AND '2026-09-30'"),
            ("UCDP days", "AND properties ->> 'source_kind' = 'ucdp'"),
        ] {
            println!("{label}: {}", size(what).await);
        }
        let started = std::time::Instant::now();
        let n = pnex_backend::services::markets::world::confirm_leads(
            &db,
            org,
            &pnex_backend::services::ontology::Actor {
                user_id: None,
                role: pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole::Owner,
                source_ref: "test:volume".into(),
            },
        )
        .await
        .unwrap();
        println!("a second confirmation pass: {n} more in {:?}", started.elapsed());
        let sizes = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT pg_size_pretty(pg_total_relation_size('objects')) AS o, \
                 pg_size_pretty(pg_total_relation_size('market_observations')) AS m, \
                 pg_size_pretty(pg_database_size(current_database())) AS d"
                    .to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        println!(
            "postgres: objects {}, market_observations {}, database {}",
            sizes.try_get::<String>("", "o").unwrap(),
            sizes.try_get::<String>("", "m").unwrap(),
            sizes.try_get::<String>("", "d").unwrap()
        );
        for path in [
            "/api/v1/markets/sessions/2026-11-05".to_string(),
            "/api/v1/markets/world-events?west=-180&south=-90&east=180&north=90".to_string(),
            "/api/v1/markets/world-events?west=20&south=40&east=50&north=55".to_string(),
            "/api/v1/markets/rule-test?dates=geo_events&series=DCOILWTICO".to_string(),
            "/api/v1/markets/rule-test?dates=geo_events&series=DCOILWTICO&include_unconfirmed=true".to_string(),
            "/api/v1/markets/rule-test?dates=geo_events&series=DCOILBRENTEU&kind=armed_conflict&country=IR".to_string(),
            "/api/v1/ontology/objects?type_key=geo_event&limit=50".to_string(),
            "/api/v1/search?q=Kharkiv&limit=20".to_string(),
        ] {
            let started = std::time::Instant::now();
            let (st, r) = api.call("GET", &path, None).await;
            let took = started.elapsed();
            let brief = r
                .get("count")
                .or_else(|| r.get("filings"))
                .cloned()
                .unwrap_or_else(|| json!(r["moves"].as_array().map(|a| a.len())));
            println!("GET {path}: {st} in {took:?} ({brief})");
            assert!(st < 500, "{path}: {r}");
        }
    })
    .await;
}

/// F6: the subsidiaries a (fictitious) group declares in the Exhibit 21 of
/// its 10-K become organizations linked to it, read once per filing. A
/// link starts at the filing date and closes when a later report drops it.
#[tokio::test]
#[serial]
async fn exhibit_21_subsidiaries_become_organizations() {
    with_app(|server, ctx, token| async move {
        use axum::{routing::get, Router};
        let db = ctx.db.clone();
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "source_contact": CONTACT })).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = "/Archives/edgar/data/42/000004226000001";
        let dir2 = "/Archives/edgar/data/42/000004227000001";
        let app = Router::new()
            .route(
                &format!("{dir}/0000042-26-000001-index.htm"),
                get(move || async move {
                    axum::response::Html(format!(
                        "<table><tr><td>1</td><td><a href=\"{dir}/acme-10k.htm\">acme-10k.htm</a></td><td>10-K</td></tr>\
                         <tr><td>5</td><td><a href=\"{dir}/ex21.htm\">ex21.htm</a></td><td>EX-21.1</td></tr></table>"
                    ))
                }),
            )
            .route(
                &format!("{dir}/ex21.htm"),
                get(|| async {
                    axum::response::Html(
                        "<table><tr><td>Name</td><td>Jurisdiction</td></tr>\
                         <tr><td>Acme Widgets LLC</td><td>Delaware</td></tr>\
                         <tr><td>Acme Widgets B.V.</td><td>Netherlands</td></tr>\
                         <tr><td>Acme Widgets (UK) Limited</td><td>England and Wales</td></tr></table>",
                    )
                }),
            )
            .route(
                &format!("{dir2}/0000042-27-000001-index.htm"),
                get(move || async move {
                    axum::response::Html(format!(
                        "<table><tr><td>5</td><td><a href=\"{dir2}/ex21.htm\">ex21.htm</a></td><td>EX-21</td></tr></table>"
                    ))
                }),
            )
            .route(
                &format!("{dir2}/ex21.htm"),
                get(|| async {
                    axum::response::Html(
                        "<table><tr><td>Acme Widgets LLC</td><td>Delaware</td></tr>\
                         <tr><td>Acme Widgets B.V.</td><td>Netherlands</td></tr></table>",
                    )
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        let (st, parent) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "organization", "title": "Acme Holdings",
                            "properties": {"cik": "0000000042", "kind": "company"}})),
            )
            .await;
        assert_eq!(st, 201, "{parent}");
        let (st, filing) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "filing", "title": "10-K 2025-02-01",
                            "properties": {"accession": "0000042-26-000001", "form": "10-K", "filed_on": "2025-02-01",
                                           "url": format!("{base}{dir}/acme-10k.htm"), "document_state": "kept"}})),
            )
            .await;
        assert_eq!(st, 201, "{filing}");
        let (st, r) = api
            .call(
                "POST",
                "/api/v1/ontology/links",
                Some(json!({"link_type": "filed_by", "source_id": filing["id"], "target_id": parent["id"]})),
            )
            .await;
        assert_eq!(st, 201, "{r}");

        for _ in 0..2 {
            pnex_backend::services::markets::documents::sweep(&ctx).await.unwrap();
        }
        let nl = object_id(&db, org, "organization", "declared_key", "NL|ACME WIDGETS B V")
            .await
            .expect("Dutch subsidiary");
        let (_, o) = api.call("GET", &format!("/api/v1/ontology/objects/{nl}"), None).await;
        assert_eq!(o["properties"]["country"], "NL", "{o}");
        assert_eq!(o["properties"]["incorporated_in"], "Netherlands");
        let links = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} AND relation = 'subsidiary_of' \
                 AND target_object_id = '{}' AND valid_to IS NULL AND attributes ->> 'nature' = 'declared' \
                 AND attributes ->> 'source' LIKE '%/ex21.htm'",
                parent["id"].as_str().unwrap()
            ),
        )
        .await;
        assert_eq!(links, 3, "one link per subsidiary, the second sweep adds none");
        let (_, f) = api
            .call("GET", &format!("/api/v1/ontology/objects/{}", filing["id"].as_str().unwrap()), None)
            .await;
        assert_eq!(f["properties"]["subsidiaries_state"], "read", "{f}");
        let parent_id = parent["id"].as_str().unwrap();
        let from = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} AND relation = 'subsidiary_of' \
                 AND target_object_id = '{parent_id}' AND valid_from = '2025-02-01T00:00:00Z'"
            ),
        )
        .await;
        assert_eq!(from, 3, "each link starts at the filing date");

        // The next report drops the British subsidiary: its link closes then.
        let (st, next) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "filing", "title": "10-K 2026-02-01",
                            "properties": {"accession": "0000042-27-000001", "form": "10-K", "filed_on": "2026-02-01",
                                           "url": format!("{base}{dir2}/acme-10k.htm"), "document_state": "kept"}})),
            )
            .await;
        assert_eq!(st, 201, "{next}");
        let (st, r) = api
            .call(
                "POST",
                "/api/v1/ontology/links",
                Some(json!({"link_type": "filed_by", "source_id": next["id"], "target_id": parent["id"]})),
            )
            .await;
        assert_eq!(st, 201, "{r}");
        pnex_backend::services::markets::documents::sweep(&ctx).await.unwrap();
        let uk = object_id(&db, org, "organization", "declared_key", "GB|ACME WIDGETS UK LIMITED")
            .await
            .expect("British subsidiary");
        let closed = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} AND relation = 'subsidiary_of' \
                 AND source_object_id = '{uk}' AND valid_to = '2026-02-01T00:00:00Z'"
            ),
        )
        .await;
        assert_eq!(closed, 1, "dropped from the later report");
        let open = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} AND relation = 'subsidiary_of' \
                 AND target_object_id = '{parent_id}' AND valid_to IS NULL AND valid_from = '2025-02-01T00:00:00Z'"
            ),
        )
        .await;
        assert_eq!(open, 2, "still declared, still dated from the first report");
    })
    .await;
}
