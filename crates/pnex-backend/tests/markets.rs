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
            "PUT" => self.server.put(path),
            "DELETE" => self.server.delete(path),
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
            "/api/v1/data/WS_CBPOL/{key}/all",
            // 100 days at 0.75, then a 25 bp cut from day 80 (2026-08-20).
            get(|| async {
                let d0 = chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
                let mut csv = String::from("FREQ,REF_AREA,TITLE,TIME_PERIOD,OBS_VALUE\n");
                for i in 0..100 {
                    let rate = if i < 80 { 0.75 } else { 0.5 };
                    csv += &format!("D,JP, Policy rate - Japan,{},{rate}\n", d0 + chrono::Duration::days(i));
                }
                csv
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
            "ecb_series": "FM.D.U2.EUR", "bis_series": "WS_CBPOL.D.JP", "covered_leis": "LEITEST",
            "move_series": "bis:WS_CBPOL.D.JP:diff:rates:1:0.05",
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
                     "https://data-api.ecb.europa.eu", "https://filings.xbrl.org", "https://stats.bis.org"] {
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
        // D813: the chart of the instrument, its session and its issuer's filings.
        let (st, chart) = api
            .call("GET", &format!("/api/v1/markets/issuers/{acme}/chart?days=3650"), None)
            .await;
        assert_eq!(st, 200, "{chart}");
        assert_eq!(chart["ticker"], "ACME");
        assert_eq!(chart["closes"].as_array().unwrap().len(), 340);
        let kinds: Vec<&str> = chart["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str()).collect();
        assert!(kinds.contains(&"move"), "{chart}");
        assert!(kinds.contains(&"filing"), "the issuer's filings: {chart}");
        let (st, _) = api.call("GET", &format!("/api/v1/markets/issuers/{acme}/chart?days=0"), None).await;
        assert_eq!(st, 400);

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
        // D880: a BIS answer names no series, the queried key is its metric.
        let r = api.run("Central bank series (BIS)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let rate = object_id(&db, org, "macro_series", "code", "WS_CBPOL.D.JP").await.expect("BIS series");
        let rates = api.observations(&rate, "metric=WS_CBPOL.D.JP").await;
        assert_eq!(rates.len(), 100, "{rates:?}");
        assert_eq!(rates[0]["period_end"], "2026-09-08");
        // D881, D888: the cut is a market move, the only one of the series.
        let cut = object_id(&db, org, "market_event", "ref", "move:WS_CBPOL.D.JP:2026-08-20:diff").await;
        assert!(cut.is_some(), "the policy rate cut is a move");
        let moves = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event' \
                 AND properties ->> 'ref' LIKE 'move:WS_CBPOL.D.JP:%'"
            ),
        )
        .await;
        assert_eq!(moves, 1);
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
            "Central bank series (BIS)",
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

/// D811: ALFRED gives each value as first published, with the org's FRED
/// key sent in the query only; an abnormal session found on these values is
/// dated by the release, not caught up from history.
#[tokio::test]
#[serial]
async fn fred_first_releases_date_the_moves() {
    with_app(|server, ctx, token| async move {
        use axum::extract::RawQuery;
        use axum::{routing::get, Router};
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
        api.settings(json!({ "source_contact": CONTACT, "move_series": "testidx:log_return:equity_index" }))
            .await;
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/fred/series/observations",
            get(move |RawQuery(q): RawQuery| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(q.unwrap_or_default());
                    let mut p = 100.0f64;
                    let obs: Vec<Value> = (0..150)
                        .map(|i| {
                            if i > 0 {
                                p *= if i == 120 { 0.94 } else if i % 2 == 0 { 1.004 } else { 0.996 };
                            }
                            json!({"realtime_start": move_day(i + 1).to_string(), "realtime_end": "9999-12-31",
                                   "date": move_day(i).to_string(), "value": format!("{p:.4}")})
                        })
                        .collect();
                    axum::Json(json!({ "observations": obs }))
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        db.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "UPDATE object_imports SET source_url = replace(source_url, 'https://api.stlouisfed.org', '{base}') \
                 WHERE org_id = {org}"
            ),
        ))
        .await
        .unwrap();
        pnex_backend::services::markets::allow_test_host(&base);
        let name = "Watched series first releases (ALFRED)";

        let r = api.run(name).await;
        assert_eq!(r["run"]["state_code"], "import-key-required", "{r}");
        assert!(seen.lock().unwrap().is_empty(), "nothing sent without a key");
        let (st, r) = api
            .call("POST", "/api/v1/secrets", Some(json!({"name": "markets_fred_key", "value": "k-fred"})))
            .await;
        assert_eq!(st, 201, "{r}");
        let r = api.run(name).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let q = seen.lock().unwrap().last().cloned().unwrap();
        assert!(q.contains("series_id=TESTIDX") && q.ends_with("&api_key=k-fred"), "{q}");
        let stored: Value = db
            .query_one_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT source_url AS u FROM object_imports WHERE org_id = {org} AND name = '{name}'"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<String>("", "u")
            .map(Value::String)
            .unwrap();
        assert!(!stored.to_string().contains("k-fred"), "the key is never stored with the URL");

        let day = move_day(120).to_string();
        let ev = object_id(&db, org, "market_event", "ref", &format!("move:TESTIDX:{day}:log_return"))
            .await
            .expect("the drop is a move");
        let (_, o) = api.call("GET", &format!("/api/v1/ontology/objects/{ev}"), None).await;
        assert_eq!(o["properties"]["backfilled"], false, "dated by the release: {o}");
        assert_eq!(o["properties"]["known_at"], format!("{}T23:59:59Z", move_day(121)), "{o}");
    })
    .await;
}

/// D812: a company created by its SEC number is looked up in GLEIF; a
/// single match is proposed, as a link to the organization that already
/// holds that LEI or as the LEI itself, and nothing is written before a
/// member accepts.
#[tokio::test]
#[serial]
async fn sec_companies_are_matched_to_their_lei_by_proposal() {
    with_app(|server, ctx, token| async move {
        use axum::extract::Query;
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
        let rec = |lei: &str, name: &str| json!({"attributes": {"lei": lei, "entity": {"legalName": {"name": name}}}});
        let (acme, beta) = ("5493000000000000ACME", "5493000000000000BETA");
        let answers = json!({
            "ACME CORP": [rec(acme, "Acme Corporation"), rec("5493000000000000AHLD", "Acme Holdings Inc.")],
            "BETA INC": [rec(beta, "Beta, Inc.")],
            "GAMMA CORP": [rec("5493000000000000GAM1", "Gamma Corp"), rec("5493000000000000GAM2", "Gamma Corporation")],
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/api/v1/lei-records",
            get(move |Query(q): Query<std::collections::HashMap<String, String>>| {
                let answers = answers.clone();
                async move {
                    assert_eq!(q.get("filter[entity.legalAddress.country]").map(String::as_str), Some("US"));
                    let name = q.get("filter[fulltext]").cloned().unwrap_or_default();
                    axum::Json(json!({ "data": answers.get(&name).cloned().unwrap_or(json!([])) }))
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let api = &api;
        let create = |title: &str, props: Value| {
            let body = json!({"type_key": "organization", "title": title, "properties": props});
            async move {
                let (st, o) = api.call("POST", "/api/v1/ontology/objects", Some(body)).await;
                assert_eq!(st, 201, "{o}");
                o["id"].as_str().unwrap().to_string()
            }
        };
        let known = create("Acme Corporation", json!({"lei": acme, "kind": "company"})).await;
        let sec_acme = create("ACME CORP", json!({"cik": "0000000042", "kind": "company"})).await;
        let sec_beta = create("BETA INC", json!({"cik": "0000000043", "kind": "company"})).await;
        let sec_gamma = create("GAMMA CORP", json!({"cik": "0000000044", "kind": "company"})).await;
        let sec_delta = create("DELTA INC", json!({"cik": "0000000045", "kind": "company"})).await;

        pnex_backend::services::markets::identity::sweep(&ctx, &base).await.unwrap();
        pnex_backend::services::markets::identity::sweep(&ctx, &base).await.unwrap();
        let state = |id: String| async move {
            let (_, o) = api.call("GET", &format!("/api/v1/ontology/objects/{id}"), None).await;
            o["properties"].clone()
        };
        assert_eq!(state(sec_acme.clone()).await["lei_state"], "proposed");
        assert_eq!(state(sec_gamma).await["lei_state"], "ambiguous");
        assert_eq!(state(sec_delta).await["lei_state"], "none");
        assert!(state(sec_beta.clone()).await.get("lei").is_none(), "nothing written before a decision");

        let (st, page) = api.call("GET", "/api/v1/proposals?limit=50", None).await;
        assert_eq!(st, 200, "{page}");
        let list = page["results"].as_array().unwrap();
        assert_eq!(list.len(), 2, "one per single match, the second sweep adds none: {page}");
        let link = list.iter().find(|p| p["kind"] == "link_create").expect("link proposal");
        assert_eq!(link["type_key"], "same_company_as");
        let update = list.iter().find(|p| p["kind"] == "object_update").expect("lei proposal");
        assert_eq!(update["values"]["lei"], beta);

        for p in [link, update] {
            let (st, r) = api
                .call("POST", &format!("/api/v1/proposals/{}/decision", p["id"].as_str().unwrap()),
                      Some(json!({"decision": "accept"})))
                .await;
            assert_eq!(st, 200, "{r}");
        }
        assert_eq!(state(sec_beta).await["lei"], beta);
        let linked = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM resource_edges WHERE org_id = {org} AND relation = 'same_company_as' \
                 AND source_object_id = '{sec_acme}' AND target_object_id = '{known}' AND valid_to IS NULL"
            ),
        )
        .await;
        assert_eq!(linked, 1);
    })
    .await;
}

/// D814: a session with moves gets one note from the org's own AI, built
/// from the session view only, sent once however many sweeps run.
#[tokio::test]
#[serial]
async fn the_session_summary_is_written_once_by_the_orgs_ai() {
    with_app(|server, ctx, token| async move {
        use axum::{routing::post, Router};
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
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let (st, ev) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({"type_key": "market_event", "title": "TESTIDX -5.5 % · 2026-11-05",
                    "properties": {"ref": "move:TESTIDX:2026-11-05:log_return", "kind": "market_move",
                        "event_at": day.to_string(), "known_at": "2026-11-05T23:59:59Z", "backfilled": false,
                        "series_code": "TESTIDX", "asset_class": "equity_index", "move_direction": "down",
                        "mode": "log_return", "measure": -0.057, "sigma": 0.012, "score": 4.75}})),
            )
            .await;
        assert_eq!(st, 201, "{ev}");

        let asked = Arc::new(Mutex::new(Vec::<Value>::new()));
        let log = asked.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let llm = format!("http://{}/v1", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |axum::Json(body): axum::Json<Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push(body);
                    axum::Json(json!({"choices": [{"message": {"content": "TESTIDX fell 4.75 sigmas.", "tool_calls": null},
                                                   "finish_reason": "stop"}]}))
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let sweep = || pnex_backend::services::markets::summary::sweep(&ctx, day);
        sweep().await.unwrap();
        assert!(asked.lock().unwrap().is_empty(), "off by default");
        api.settings(json!({ "session_summary": "on" })).await;
        sweep().await.unwrap();
        assert!(asked.lock().unwrap().is_empty(), "no provider: nothing");

        let (st, r) = api
            .call("POST", "/api/v1/ai/providers",
                  Some(json!({"name": "main", "kind": "openai_compat", "model": "m1", "base_url": llm,
                              "api_key": {"value": "k-llm"}})))
            .await;
        assert_eq!(st, 201, "{r}");
        sweep().await.unwrap();
        sweep().await.unwrap();
        let calls = asked.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "written once: {calls:?}");
        let sent = calls[0].to_string();
        assert!(sent.contains("TESTIDX") && sent.contains("4.75"), "{sent}");
        assert!(calls[0]["tools"].as_array().is_none_or(|t| t.is_empty()), "no tool: read only");
        let state = count(
            &db,
            format!("SELECT count(*) AS n FROM market_session_summaries WHERE org_id = {org} AND state = 'sent'"),
        )
        .await;
        assert_eq!(state, 1);
    })
    .await;
}

/// MK11: the minute bars of the listed symbols go into a table, one
/// partition per transaction, against a mock of Binance's public files. A
/// done partition is never fetched again; a wrong checksum and an
/// unpublished day are asked again on the next pass.
#[tokio::test]
#[serial]
async fn minute_history_is_loaded_by_partitions_and_resumed() {
    with_app(|server, ctx, token| async move {
        use axum::{extract::Path, http::StatusCode, response::IntoResponse, routing::get, Router};
        use sha2::{Digest, Sha256};
        use std::io::Write;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "history_symbols": "btcusdt", "history_from": "2026-09" }))
            .await;

        fn zip_of(name: &str, csv: &str) -> Vec<u8> {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(csv.as_bytes()).unwrap();
            w.finish().unwrap().into_inner()
        }
        let line = |ms: i64| format!("{ms},100.5,101,100,100.25000000,2,0,200.5,7,1,100,0\n");
        let sep = zip_of(
            "BTCUSDT-1m-2026-09.csv",
            &format!(
                "{}{}{}",
                line(1_788_220_800_000),
                line(1_788_220_860_000),
                line(1_788_220_920_000)
            ),
        );
        let oct = zip_of("BTCUSDT-1m-2026-10.csv", &line(1_790_812_800_000));
        // November 2 in microseconds, as Binance publishes since 2025.
        let nov2 = zip_of("BTCUSDT-1m-2026-11-02.csv", &line(1_793_577_600_000_000));
        let hits: Arc<Mutex<Vec<String>>> = Arc::default();
        let files: Arc<std::collections::HashMap<String, Vec<u8>>> = Arc::new(
            [
                ("monthly/klines/BTCUSDT/1m/BTCUSDT-1m-2026-09.zip", sep),
                ("monthly/klines/BTCUSDT/1m/BTCUSDT-1m-2026-10.zip", oct),
                ("daily/klines/BTCUSDT/1m/BTCUSDT-1m-2026-11-02.zip", nov2),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        );
        let app = Router::new().route(
            "/data/spot/{*path}",
            get({
                let (hits, files) = (hits.clone(), files.clone());
                move |Path(path): Path<String>| async move {
                    hits.lock().unwrap().push(path.clone());
                    if let Some(zip) = path.strip_suffix(".CHECKSUM") {
                        let Some(body) = files.get(zip) else {
                            return StatusCode::NOT_FOUND.into_response();
                        };
                        let mut hex: String = Sha256::digest(body)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        if zip.contains("2026-10") {
                            hex = "0".repeat(64);
                        }
                        return format!("{hex}  {zip}\n").into_response();
                    }
                    match files.get(&path) {
                        Some(b) => b.clone().into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        pnex_backend::services::markets::history::sweep_from(
            &ctx,
            &pnex_backend::services::markets::history::Hosts {
                binance: base.clone(),
                ..Default::default()
            },
            today,
        )
        .await
        .unwrap();
        let states = || async {
            ctx.db
                .query_all_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT period || ':' || period_start::text || ':' || state || ':' \
                     || coalesce(code, '') || ':' || rows::text AS s \
                     FROM market_history_partitions ORDER BY period_start"
                        .to_string(),
                ))
                .await
                .unwrap()
                .iter()
                .map(|r| r.try_get::<String>("", "s").unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            states().await,
            vec![
                "month:2026-09-01:done::3",
                "month:2026-10-01:failed:checksum:0",
                "day:2026-11-01:empty::0",
                "day:2026-11-02:done::1",
            ]
        );
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "binance_spot_klines_1m")
            .unwrap_or_else(|| panic!("{list}"))
            .clone();
        let types: Vec<String> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                format!(
                    "{}:{}",
                    c["name"].as_str().unwrap(),
                    c["data_type"].as_str().unwrap()
                )
            })
            .collect();
        assert!(types.contains(&"ts:timestamp".to_string()), "{types:?}");
        assert!(types.contains(&"close:decimal".to_string()), "{types:?}");
        assert_eq!(table["rows"], 4, "3 September minutes and 1 of November 2");

        // Second pass: only what is not settled is asked again.
        hits.lock().unwrap().clear();
        pnex_backend::services::markets::history::sweep_from(
            &ctx,
            &pnex_backend::services::markets::history::Hosts {
                binance: base.clone(),
                ..Default::default()
            },
            today,
        )
        .await
        .unwrap();
        let asked: Vec<String> = hits
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with(".zip"))
            .cloned()
            .collect();
        assert_eq!(
            asked,
            vec![
                "monthly/klines/BTCUSDT/1m/BTCUSDT-1m-2026-10.zip",
                "daily/klines/BTCUSDT/1m/BTCUSDT-1m-2026-11-01.zip",
            ],
            "a done partition is never fetched again"
        );
    })
    .await;
}

/// MK11 acceptance: BTCUSDT and ETHUSDT spot 1m since 2017-08 from the real
/// data.binance.vision (about 220 partitions, 1 to 2 GB).
#[tokio::test]
#[serial]
#[ignore = "downloads the full Binance minute history of two symbols"]
async fn minute_history_real_binance() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::markets::history::sweep;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "history_symbols": "BTCUSDT,ETHUSDT", "history_from": "2017-08" }))
            .await;
        let one = |sql: &'static str| {
            let db = ctx.db.clone();
            async move {
                db.query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    sql.to_string(),
                ))
                .await
                .unwrap()
                .unwrap()
            }
        };
        let today = chrono::Utc::now().date_naive();
        let started = std::time::Instant::now();
        let mut known = -1_i64;
        for pass in 0..40 {
            sweep(&ctx, today).await.unwrap();
            let n: i64 = one("SELECT count(*) AS n FROM market_history_partitions")
                .await
                .try_get("", "n")
                .unwrap();
            eprintln!("pass {pass}: {n} partitions, {:?}", started.elapsed());
            if n == known {
                break;
            }
            known = n;
        }
        let failed: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT instrument || ':' || period_start::text || ':' || coalesce(code, '') AS s \
                 FROM market_history_partitions WHERE state = 'failed'"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        assert!(failed.is_empty(), "failed partitions: {failed:?}");
        // A month never holds more rows than minutes; Binance publishes few gaps.
        let over: i64 = one(
            "SELECT count(*) AS n FROM market_history_partitions WHERE period = 'month' \
               AND rows > extract(epoch FROM (period_start + interval '1 month') \
                 - period_start::timestamp) / 60",
        )
        .await
        .try_get("", "n")
        .unwrap();
        assert_eq!(over, 0, "a month with more rows than minutes");
        let row = one(
            "SELECT sum(rows)::bigint AS rows, \
               sum(extract(epoch FROM (CASE period WHEN 'month' THEN period_start + interval '1 month' \
                 ELSE period_start + interval '1 day' END) - period_start::timestamp) / 60) \
                 FILTER (WHERE state = 'done')::bigint AS minutes, \
               max(fetched_at) AS last FROM market_history_partitions",
        )
        .await;
        let rows: i64 = row.try_get("", "rows").unwrap();
        let minutes: i64 = row.try_get("", "minutes").unwrap();
        eprintln!(
            "{rows} rows for {minutes} minutes of done partitions ({:.3} %), {:?}",
            rows as f64 * 100.0 / minutes as f64,
            started.elapsed()
        );
        assert!(rows as f64 >= minutes as f64 * 0.99, "{rows} rows, {minutes} minutes");
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "binance_spot_klines_1m")
            .unwrap_or_else(|| panic!("{list}"))
            .clone();
        assert_eq!(table["rows"].as_i64(), Some(rows), "{table}");

        // A second run downloads nothing settled again.
        let before: chrono::DateTime<chrono::Utc> = row.try_get("", "last").unwrap();
        sweep(&ctx, today).await.unwrap();
        let refetched: i64 = ctx
            .db
            .query_one_raw(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT count(*) AS n FROM market_history_partitions \
                 WHERE state = 'done' AND fetched_at > $1",
                [before.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap();
        assert_eq!(refetched, 0, "a done partition was fetched again");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn tick_history_is_loaded_by_days() {
    with_app(|server, ctx, token| async move {
        use axum::{extract::Path, http::StatusCode, response::IntoResponse, routing::get, Router};
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(
            json!({ "history_fx_pairs": "eurusd,USDJPY,XAUUSD", "history_fx_from": "2026-11" }),
        )
        .await;

        fn bi5(records: &[(u32, u32, u32)]) -> Vec<u8> {
            let mut raw = Vec::new();
            for (ms, ask, bid) in records {
                raw.extend(ms.to_be_bytes());
                raw.extend(ask.to_be_bytes());
                raw.extend(bid.to_be_bytes());
                raw.extend(1.5_f32.to_bits().to_be_bytes());
                raw.extend(0.25_f32.to_bits().to_be_bytes());
            }
            let mut out = Vec::new();
            lzma_rs::lzma_compress(&mut std::io::Cursor::new(raw), &mut out).unwrap();
            out
        }
        // November is month 10 in Dukascopy's paths.
        let files: Arc<std::collections::HashMap<String, Vec<u8>>> = Arc::new(
            [
                (
                    "EURUSD/2026/10/02/00h_ticks.bi5",
                    bi5(&[(0, 108_001, 107_999), (1500, 108_002, 108_000)]),
                ),
                ("EURUSD/2026/10/02/01h_ticks.bi5", Vec::new()),
                (
                    "EURUSD/2026/10/02/13h_ticks.bi5",
                    bi5(&[(59_000, 108_101, 108_099)]),
                ),
                (
                    "USDJPY/2026/10/02/00h_ticks.bi5",
                    bi5(&[(10, 157_123, 157_120)]),
                ),
                ("USDJPY/2026/10/02/05h_ticks.bi5", b"not lzma".to_vec()),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        );
        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/datafeed/{*path}",
            get({
                let (hits, files) = (hits.clone(), files.clone());
                move |Path(path): Path<String>| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    match files.get(&path) {
                        Some(b) => b.clone().into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let hosts = pnex_backend::services::markets::history::Hosts {
            dukascopy: base.clone(),
            ..Default::default()
        };
        let sweep = || pnex_backend::services::markets::history::sweep_from(&ctx, &hosts, today);
        sweep().await.unwrap();
        let states: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT instrument || ':' || period_start::text || ':' || state || ':' \
                 || coalesce(code, '') || ':' || rows::text AS s \
                 FROM market_history_partitions ORDER BY instrument, period_start"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        assert_eq!(
            states,
            vec![
                "EURUSD:2026-11-01:empty::0",
                "EURUSD:2026-11-02:done::3",
                "USDJPY:2026-11-01:empty::0",
                "USDJPY:2026-11-02:failed:unreadable:0",
            ],
            "XAUUSD has no known scale and is never asked"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            4 * 24 - 18,
            "USDJPY stops at its bad hour"
        );
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "dukascopy_ticks")
            .unwrap_or_else(|| panic!("{list}"))
            .clone();
        assert_eq!(table["rows"], 3);
        let types: Vec<String> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                format!(
                    "{}:{}",
                    c["name"].as_str().unwrap(),
                    c["data_type"].as_str().unwrap()
                )
            })
            .collect();
        assert!(types.contains(&"bid:decimal".to_string()), "{types:?}");
        assert!(types.contains(&"ts:timestamp".to_string()), "{types:?}");

        // The done day is not asked again; the failed and recent empty ones are.
        hits.store(0, Ordering::SeqCst);
        sweep().await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 24 + 24 + 6);
    })
    .await;
}

/// MK12 acceptance: EURUSD and USDJPY ticks of the last twelve months from
/// the real datafeed.dukascopy.com (about 17 500 hour files).
#[tokio::test]
#[serial]
#[ignore = "downloads a year of Dukascopy ticks for two pairs"]
async fn tick_history_real_dukascopy() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::markets::history::sweep_from;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "history_fx_pairs": "EURUSD,USDJPY", "history_fx_from": "" }))
            .await;
        let today = chrono::Utc::now().date_naive();
        let started = std::time::Instant::now();
        let mut known = -1_i64;
        for pass in 0..80 {
            sweep_from(&ctx, &Default::default(), today).await.unwrap();
            let n: i64 = ctx
                .db
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT count(*) AS n FROM market_history_partitions".to_string(),
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get("", "n")
                .unwrap();
            eprintln!("pass {pass}: {n} days, {:?}", started.elapsed());
            if n == known {
                break;
            }
            known = n;
        }
        let summary: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT instrument || ':' || state || ':' || coalesce(code, '') || ':' \
                   || count(*)::text || ':' || sum(rows)::text || ':' || sum(bytes)::text AS s \
                 FROM market_history_partitions GROUP BY instrument, state, code ORDER BY 1"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        eprintln!("{summary:#?} in {:?}", started.elapsed());
        assert!(
            !summary.iter().any(|s| s.contains(":failed:")),
            "{summary:?}"
        );
        // Printed for a look at the scales: EURUSD near 1, USDJPY near 150.
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let id = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "dukascopy_ticks")
            .unwrap_or_else(|| panic!("{list}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, sample) = api
            .call(
                "GET",
                &format!("/api/v1/datasets/{id}/sample?limit=5"),
                None,
            )
            .await;
        eprintln!("{st} {sample}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn trade_history_is_loaded_by_days() {
    with_app(|server, ctx, token| async move {
        use axum::{extract::Path, http::StatusCode, response::IntoResponse, routing::get, Router};
        use sha2::{Digest, Sha256};
        use std::io::Write;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(
            json!({ "history_trade_symbols": "BTCUSDT", "history_trade_from": "2026-11" }),
        )
        .await;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file(
            "BTCUSDT-trades-2026-11-01.csv",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        w.write_all(
            b"1,100.50000000,0.20000000,20.10000000,1793491200000000,true,true\n\
2,100.60000000,0.10000000,10.06000000,1793491200500000,false,true\n",
        )
        .unwrap();
        let day1 = w.finish().unwrap().into_inner();
        let files: Arc<std::collections::HashMap<String, Vec<u8>>> = Arc::new(
            [
                (
                    "daily/trades/BTCUSDT/BTCUSDT-trades-2026-11-01.zip",
                    day1.clone(),
                ),
                ("daily/trades/BTCUSDT/BTCUSDT-trades-2026-11-02.zip", day1),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        );
        let app = Router::new().route(
            "/data/spot/{*path}",
            get({
                let files = files.clone();
                move |Path(path): Path<String>| async move {
                    if let Some(zip) = path.strip_suffix(".CHECKSUM") {
                        let Some(body) = files.get(zip) else {
                            return StatusCode::NOT_FOUND.into_response();
                        };
                        let mut hex: String = Sha256::digest(body)
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        if zip.contains("11-02") {
                            hex = "0".repeat(64);
                        }
                        return format!("{hex}  {zip}\n").into_response();
                    }
                    match files.get(&path) {
                        Some(b) => b.clone().into_response(),
                        None => StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        // Two sweeps at once (a pass outliving its turn): nothing loaded twice.
        let hosts = pnex_backend::services::markets::history::Hosts {
            binance: base.clone(),
            ..Default::default()
        };
        let sweep = || pnex_backend::services::markets::history::sweep_from(&ctx, &hosts, today);
        let (a, b) = tokio::join!(sweep(), sweep());
        a.unwrap();
        b.unwrap();
        let states: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT source || ':' || period_start::text || ':' || state || ':' \
                 || coalesce(code, '') || ':' || rows::text AS s \
                 FROM market_history_partitions ORDER BY period_start"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        assert_eq!(
            states,
            vec![
                "binance_public_trades:2026-11-01:done::2",
                "binance_public_trades:2026-11-02:failed:checksum:0",
                "binance_public_trades:2026-11-03:empty::0",
            ]
        );
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let table = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "binance_spot_trades")
            .unwrap_or_else(|| panic!("{list}"))
            .clone();
        assert_eq!(table["rows"], 2);
        let names: Vec<&str> = table["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "instrument",
                "ts",
                "price",
                "qty",
                "side",
                "trade_id",
                "source"
            ]
        );
    })
    .await;
}

/// D847–D849: an MT5 terminal registered in the UI, its generated EA with
/// its own token, the catalogue it announces, the configuration it reads,
/// the calls it pushes; the markets sweep turns them into the tables.
#[tokio::test]
#[serial]
async fn mt5_terminals_push_into_tables() {
    with_app(|server, ctx, token| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        assert_eq!(st, 200, "{r}");
        let (st, t) = api
            .call("POST", "/api/v1/mt5/terminals", Some(json!({ "name": "Demo gold", "account": "123456" })))
            .await;
        assert_eq!(st, 201, "{t}");
        assert_eq!(t["bars"], "*", "the whole catalogue by default (Q-MT3)");
        assert_eq!(t["ticks"], "", "no tick until chosen (Q-MT4)");
        assert_eq!(t["presence"], "never");
        assert_eq!(t["has_token"], false);
        let id = t["id"].as_str().unwrap().to_string();
        let (st, dup) = api.call("POST", "/api/v1/mt5/terminals", Some(json!({ "name": "Demo gold" }))).await;
        assert_eq!((st, dup["error"].as_str()), (409, Some("mt5-terminal-name-taken")), "{dup}");
        let (st, bad) = api
            .call("POST", "/api/v1/mt5/terminals", Some(json!({ "name": "x", "bars": "EUR USD" })))
            .await;
        assert_eq!(st, 400, "{bad}");

        // The generated EA carries the address and a token shown nowhere else.
        let ea = |server: &axum_test::TestServer| {
            server
                .post(&format!("/api/v1/mt5/terminals/{id}/ea"))
                .add_header("Authorization", format!("Bearer {token}"))
                .add_header("X-Org-Id", org.to_string())
                .add_header("x-forwarded-host", "pnex.test")
                .add_header("x-forwarded-proto", "https")
        };
        let res = ea(&server).await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let source = res.text();
        assert!(source.contains("#define PNEX_BASE    \"https://pnex.test\""), "{}", &source[..600]);
        let mt5_token = source
            .split("#define PNEX_TOKEN   \"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap()
            .to_string();
        assert!(mt5_token.starts_with("pnex_mt5_"));
        let (_, read) = api.call("GET", &format!("/api/v1/mt5/terminals/{id}"), None).await;
        assert_eq!(read["has_token"], true);
        assert!(!read.to_string().contains(&mt5_token), "never read back");

        let machine = |method: &str, path: &str, token: &str, body: Option<Value>| {
            let req = match method {
                "GET" => server.get(path),
                _ => server.post(path),
            }
            .add_header("Authorization", format!("Bearer {token}"));
            match body {
                Some(b) => req.json(&b),
                None => req,
            }
        };
        let res = machine("GET", "/api/v1/mt5/config", "pnex_mt5_00", None).await;
        assert_eq!(res.status_code().as_u16(), 401);
        assert_eq!(res.json::<Value>()["error"], "mt5-token-invalid");
        let res = machine("GET", "/api/v1/mt5/config", &mt5_token, None).await;
        assert_eq!(res.status_code().as_u16(), 200);
        assert_eq!(res.json::<Value>()["bars"], json!([]), "nothing announced yet");

        // The catalogue, then a selection chosen in the UI.
        let res = machine(
            "POST",
            "/api/v1/mt5/announce",
            &mt5_token,
            Some(json!({"v": 1, "broker": "Demo Broker Ltd", "server": "Demo-Server", "symbols": [
                {"symbol": "XAUUSD", "description": "Gold vs US Dollar", "base": "XAU", "profit": "USD",
                 "digits": 2, "contract_size": 100.0, "first_m1": 1_104_537_600},
                {"symbol": "EURUSD", "description": "Euro vs US Dollar", "digits": 5, "contract_size": 100000.0}
            ]})),
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 204, "{}", res.text());
        let cfg = machine("GET", "/api/v1/mt5/config", &mt5_token, None).await.json::<Value>();
        assert_eq!(cfg["bars"], json!(["EURUSD", "XAUUSD"]));
        assert_eq!(cfg["ticks"], json!([]));
        let (_, now) = api.call("GET", &format!("/api/v1/mt5/terminals/{id}"), None).await;
        assert_eq!(now["broker"], "Demo Broker Ltd", "an empty broker takes the announced one");
        assert_eq!(now["symbols"], 2);
        let (st, up) = api
            .call(
                "PUT",
                &format!("/api/v1/mt5/terminals/{id}"),
                Some(json!({"name": "Demo gold", "ticks": "XAU*", "expected_version": now["version"]})),
            )
            .await;
        assert_eq!(st, 200, "{up}");
        let (st, stale) = api
            .call(
                "PUT",
                &format!("/api/v1/mt5/terminals/{id}"),
                Some(json!({"name": "Demo gold", "expected_version": now["version"]})),
            )
            .await;
        assert_eq!((st, stale["error"].as_str()), (409, Some("mt5-terminal-version-conflict")));
        let cfg = machine("GET", "/api/v1/mt5/config", &mt5_token, None).await.json::<Value>();
        assert_eq!(cfg["ticks"], json!(["XAUUSD"]));
        let (_, sym) = api
            .call("GET", &format!("/api/v1/mt5/terminals/{id}/symbols?q=gold"), None)
            .await;
        assert_eq!(sym["count"], 1, "{sym}");
        assert_eq!(sym["results"][0]["ticks"], true);
        assert_eq!(sym["results"][0]["first_m1"], "2005-01-01T00:00:00+00:00");

        // Calls of bars and ticks, a heartbeat, a bad call.
        let call = json!({
            "v": 1, "broker": "Demo", "ea_version": "2.00", "build": 4755,
            "bars": [
                {"symbol": "EURUSD", "time": 1_735_689_600, "open": "1.03456", "high": "1.03470",
                 "low": "1.03400", "close": "1.03450", "volume": 120},
                {"symbol": "XAUUSD", "time": 1_735_689_600, "open": "2624.10", "high": "2625.00",
                 "low": "2623.90", "close": "2624.55", "volume": 80}
            ],
            "ticks": [{"symbol": "XAUUSD", "time_msc": 1_735_689_600_123_i64, "bid": "2624.50", "ask": "2624.80"}]
        });
        let res = machine("POST", "/api/v1/mt5/ingest", &mt5_token, Some(call)).await;
        assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
        let res = machine("POST", "/api/v1/mt5/ingest", &mt5_token, Some(json!({"v": 1, "bars": [{"symbol": "../x", "time": 1, "open": "1", "high": "1", "low": "1", "close": "1"}]}))).await;
        assert_eq!(res.status_code().as_u16(), 400);
        assert_eq!(res.json::<Value>()["errors"]["args"]["token"], "symbol");
        let res = machine("POST", "/api/v1/mt5/ingest", &mt5_token, Some(json!({"v": 1}))).await;
        assert_eq!(res.status_code().as_u16(), 202, "a heartbeat");
        let (_, seen) = api.call("GET", &format!("/api/v1/mt5/terminals/{id}"), None).await;
        assert_eq!(seen["presence"], "online");
        assert_eq!(seen["ea_version"], "2.00");
        assert_eq!(seen["terminal_build"], 4755);

        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        pnex_backend::services::markets::history::sweep_from(&ctx, &pnex_backend::services::markets::history::Hosts { binance: "http://unused".into(), ..Default::default() }, today).await.unwrap();
        let inbox = count(&ctx.db, format!("SELECT count(*) AS n FROM mt5_inbox WHERE org_id = {org}")).await;
        assert_eq!(inbox, 0, "read calls leave the inbox");
        let (st, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        let table = |name: &str| {
            list["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["name"] == name)
                .unwrap_or_else(|| panic!("{name}: {list}"))
                .clone()
        };
        assert_eq!(table("mt5_ohlcv_1m")["rows"], 2);
        assert_eq!(table("mt5_ticks")["rows"], 1);
        let bars_id = table("mt5_ohlcv_1m")["id"].as_str().unwrap().to_string();
        let (_, sample) = api.call("GET", &format!("/api/v1/datasets/{bars_id}/sample"), None).await;
        assert!(sample.to_string().contains("mt5:Demo"), "{sample}");
        assert!(sample.to_string().contains("2624.55"), "{sample}");

        // A backfill moves the EA's cursors back; a new EA revokes the old one.
        let (st, bf) = api
            .call("POST", &format!("/api/v1/mt5/terminals/{id}/backfill"), Some(json!({"days": 30})))
            .await;
        assert_eq!(st, 200, "{bf}");
        let cfg = machine("GET", "/api/v1/mt5/config", &mt5_token, None).await.json::<Value>();
        let (bid, from) = (cfg["backfill_id"].as_i64().unwrap(), cfg["backfill_from"].as_i64().unwrap());
        assert!(bid > 0 && bid - from == 30 * 86_400, "{cfg}");
        let (st, _) = api
            .call("POST", &format!("/api/v1/mt5/terminals/{id}/backfill"), Some(json!({"days": 0})))
            .await;
        assert_eq!(st, 400);
        assert_eq!(ea(&server).await.status_code().as_u16(), 200);
        let res = machine("GET", "/api/v1/mt5/config", &mt5_token, None).await;
        assert_eq!(res.status_code().as_u16(), 401, "the previous EA stops at once");

        let (st, _) = api.call("DELETE", &format!("/api/v1/mt5/terminals/{id}"), None).await;
        assert_eq!(st, 204);
        let (st, gone) = api.call("GET", &format!("/api/v1/mt5/terminals/{id}"), None).await;
        assert_eq!((st, gone["error"].as_str()), (404, Some("mt5-terminal-not-found")));

        // Anyone who signs up owns an org: its terminals are bounded (SEC-84).
        for i in 0..pnex_core::mt5::TERMINALS_MAX {
            let (st, r) = api.call("POST", "/api/v1/mt5/terminals", Some(json!({ "name": format!("t{i}") }))).await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, over) = api.call("POST", "/api/v1/mt5/terminals", Some(json!({ "name": "one more" }))).await;
        assert_eq!((st, over["error"].as_str()), (409, Some("mt5-terminals-limit")), "{over}");
    })
    .await;
}

/// HistData: the page's own form is sent back for the file, a closed year
/// per file, times turned from EST (no daylight saving) to UTC.
#[tokio::test]
#[serial]
async fn histdata_years_are_loaded_from_the_page_form() {
    with_app(|server, ctx, token| async move {
        use axum::{extract::RawQuery, http::HeaderMap, http::StatusCode, response::IntoResponse, routing::{get, post}, Form, Router};
        use std::io::Write;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "histdata_pairs": "eurusd", "histdata_from": "2023" }))
            .await;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("DAT_MT_EURUSD_M1_2024.csv", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(
            b"2024.01.01,17:00,1.104270,1.104290,1.104250,1.104290,0\n\
2024.01.01,17:01,1.104290,1.104290,1.104290,1.104290,0\n",
        )
        .unwrap();
        w.start_file("DAT_MT_EURUSD_M1_2024.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"gaps report").unwrap();
        let zip = w.finish().unwrap().into_inner();
        let posts: Arc<Mutex<Vec<String>>> = Arc::default();
        let app = Router::new()
            .route(
                "/download-free-forex-historical-data/",
                get(|RawQuery(q): RawQuery| async move {
                    // 2024 has a file; 2023 (before this mock's first) and 2025 not yet.
                    match q.as_deref() {
                        Some("/metatrader/1-minute-bar-quotes/eurusd/2024") => r#"<html>
<form id="file_down" name="file_down" target="nullDisplay" method="POST" action="/get.php">
<input type="hidden" name="tk" id="tk" value="0123456789abcdef0123456789abcdef" />
<input type="hidden" name="date" id="date" value="2024" />
<input type="hidden" name="fxpair" id="fxpair" value="EURUSD" /></form></html>"#
                            .into_response(),
                        _ => "<html>no file here</html>".into_response(),
                    }
                }),
            )
            .route(
                "/get.php",
                post({
                    let posts = posts.clone();
                    move |headers: HeaderMap, Form(f): Form<std::collections::HashMap<String, String>>| async move {
                        let referer = headers
                            .get("referer")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_string();
                        posts.lock().unwrap().push(format!(
                            "{}:{}:{}",
                            f.get("tk").cloned().unwrap_or_default(),
                            f.get("date").cloned().unwrap_or_default(),
                            referer.ends_with("eurusd/2024")
                        ));
                        if f.get("tk").map(String::as_str) != Some("0123456789abcdef0123456789abcdef") {
                            return StatusCode::FORBIDDEN.into_response();
                        }
                        zip.clone().into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        // One file per pass: 2023, 2024, 2025, then the months of 2026.
        let today = chrono::NaiveDate::from_ymd_opt(2026, 2, 10).unwrap();
        for _ in 0..5 {
            pnex_backend::services::markets::history::sweep_from(&ctx, &pnex_backend::services::markets::history::Hosts { histdata: base.clone(), ..Default::default() }, today)
                .await
                .unwrap();
        }
        let states: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT period || ':' || period_start::text || ':' || state || ':' || rows::text AS s \
                 FROM market_history_partitions WHERE source = 'histdata' ORDER BY period_start"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        assert_eq!(
            states,
            vec![
                "year:2023-01-01:empty:0",
                "year:2024-01-01:done:2",
                "year:2025-01-01:empty:0",
                "month:2026-01-01:empty:0",
            ]
        );
        assert_eq!(
            posts.lock().unwrap().clone(),
            vec!["0123456789abcdef0123456789abcdef:2024:true"],
            "the page's own fields, with the page as referer"
        );
        let (_, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        let id = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "histdata_ohlcv_1m")
            .unwrap_or_else(|| panic!("{list}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, sample) = api
            .call("GET", &format!("/api/v1/datasets/{id}/sample"), None)
            .await;
        assert_eq!(st, 200, "{sample}");
        // 17:00 EST is 22:00 UTC.
        assert!(sample.to_string().contains("2024-01-01T22:00:00"), "{sample}");
    })
    .await;
}

/// HistData acceptance: EURUSD from 2000 from the real histdata.com, one
/// file at a time (about 36 files, 5 s apart).
#[tokio::test]
#[serial]
#[ignore = "downloads the full HistData EURUSD history"]
async fn histdata_real_eurusd() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::markets::history::sweep;
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
            .call("POST", "/api/v1/ontology/packs/markets/install", None)
            .await;
        assert_eq!(st, 200, "{r}");
        api.settings(json!({ "histdata_pairs": "EURUSD", "histdata_from": "2000" }))
            .await;
        let today = chrono::Utc::now().date_naive();
        let started = std::time::Instant::now();
        let count = || async {
            ctx.db
                .query_one_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT count(*) AS n FROM market_history_partitions WHERE source = 'histdata'"
                        .to_string(),
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "n")
                .unwrap()
        };
        let (mut known, mut still) = (-1_i64, 0);
        for pass in 0..80 {
            sweep(&ctx, today).await.unwrap();
            let n = count().await;
            eprintln!("pass {pass}: {n} partitions, {:?}", started.elapsed());
            still = if n == known { still + 1 } else { 0 };
            if still == 2 {
                break;
            }
            known = n;
        }
        let summary: Vec<String> = ctx
            .db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT period || ':' || period_start::text || ':' || state || ':' \
                   || coalesce(code, '') || ':' || rows::text AS s \
                 FROM market_history_partitions WHERE source = 'histdata' ORDER BY period_start"
                    .to_string(),
            ))
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String>("", "s").unwrap())
            .collect();
        eprintln!("{summary:#?} in {:?}", started.elapsed());
        assert!(
            !summary.iter().any(|s| s.contains(":failed:")),
            "{summary:?}"
        );
        assert!(
            summary.contains(&"year:2024-01-01:done::372379".to_string()),
            "{summary:?}"
        );
    })
    .await;
}

/// MK17, MK18–MK20 (D882–D885): the calendars become time ranges of the org,
/// Eurostat and PortWatch become dated series.
#[tokio::test]
#[serial]
async fn calendars_eurostat_and_chokepoints() {
    use axum::extract::RawQuery;
    use axum::http::HeaderMap;
    use axum::routing::get;
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
            "source_contact": CONTACT, "calendars": "bls, fomc, ecb",
            "eurostat_series": "prc_hicp_manr.M.RCH_A.NRG.EA", "chokepoints": "chokepoint6",
            "cot_markets": "088691", "eia_series": "PET.WCESTUS1.W"
        }))
        .await;
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = asked.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route(
                "/bls.ics",
                get(|h: HeaderMap| async move {
                    // The BLS refuses a client without a declared contact.
                    let ua = h.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("");
                    if !ua.contains("test@example.com") {
                        return (axum::http::StatusCode::FORBIDDEN, String::new());
                    }
                    (axum::http::StatusCode::OK, "BEGIN:VCALENDAR\r\nX-WR-CALNAME:BLS\r\n\
BEGIN:VEVENT\r\nUID:cpi-dec\r\nDTSTART;TZID=US-Eastern:20261210T083000\r\nDURATION:PT0M\r\nSUMMARY:Consumer Price Index\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:jobs-dec\r\nDTSTART;TZID=US-Eastern:20261204T083000\r\nDURATION:PT0M\r\nSUMMARY:Employment Situation\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n".to_string())
                }),
            )
            .route(
                "/fomc.htm",
                get(|| async {
                    "<h4>2026 FOMC Meetings</h4><div class=\"fomc-meeting__month\"><strong>December</strong></div>\
                     <div class=\"fomc-meeting__date\">8-9*</div>"
                }),
            )
            .route(
                "/ecb.html",
                get(|| async {
                    "<p>Governing Council</p><dt>17/12/2026</dt><dd>Governing Council of the ECB: monetary policy meeting in Frankfurt (Day 2), followed by press conference</dd>"
                }),
            )
            .route(
                "/eurostat/api/dissemination/sdmx/2.1/data/PRC_HICP_MANR/M.RCH_A.NRG.EA",
                get(|| async {
                    "DATAFLOW,LAST UPDATE,freq,unit,coicop,geo,TIME_PERIOD,OBS_VALUE,OBS_FLAG\n\
                     ESTAT:PRC_HICP_MANR(1.0),06/02/26 23:00:00,M,RCH_A,NRG,EA,2026-08,-1.9,\n\
                     ESTAT:PRC_HICP_MANR(1.0),06/02/26 23:00:00,M,RCH_A,NRG,EA,2026-09,-0.4,\n"
                }),
            )
            .route(
                "/v2/seriesid/{id}",
                get(|axum::extract::Path(id): axum::extract::Path<String>, RawQuery(q): RawQuery| async move {
                    let q = q.unwrap_or_default();
                    if id != "PET.WCESTUS1.W" || q != "api_key=k-eia" {
                        return (axum::http::StatusCode::FORBIDDEN, axum::Json(json!({})));
                    }
                    (axum::http::StatusCode::OK, axum::Json(json!({"response": {"frequency": "weekly",
                        "data": [{"period": "2026-10-30", "value": "421345", "units": "MBBL"}]}})))
                }),
            )
            .route(
                "/resource/6dca-aqww.json",
                get(|| async {
                    axum::Json(json!([{"report_date_as_yyyy_mm_dd": "2026-11-03T00:00:00.000",
                        "market_and_exchange_names": "GOLD - COMMODITY EXCHANGE INC.",
                        "open_interest_all": "399169", "noncomm_positions_long_all": "220530",
                        "noncomm_positions_short_all": "40982"}]))
                }),
            )
            .route(
                "/weJ1QsnbMYJlCHdG/arcgis/rest/services/Daily_Chokepoints_Data/FeatureServer/0/query",
                get(move |RawQuery(q): RawQuery| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push(q.unwrap_or_default());
                        axum::Json(json!({"features": [
                            {"attributes": {"date": "2026-10-31", "portname": "Strait of Hormuz", "n_total": 80, "n_tanker": 40, "capacity": 9000000}},
                            {"attributes": {"date": "2026-11-01", "portname": "Strait of Hormuz", "n_total": 20, "n_tanker": 5, "capacity": 2000000}}
                        ]}))
                    }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        // Calendars: one range per release and decision, in the org's calendar.
        let urls = pnex_backend::services::markets::calendar::Urls {
            bls: format!("{base}/bls.ics"),
            fomc: format!("{base}/fomc.htm"),
            ecb: format!("{base}/ecb.html"),
        };
        pnex_backend::services::markets::calendar::sweep(&ctx, &urls).await.unwrap();
        let ranges = |sql: &str| count(&db, format!("SELECT count(*) AS n FROM time_ranges WHERE org_id = {org} AND scope_kind = 'org' {sql}"));
        assert_eq!(ranges("").await, 104);
        assert_eq!(ranges("AND external_id = 'bls:cpi-dec' AND category = 'economic_release'").await, 1);
        assert_eq!(ranges("AND external_id = 'fomc:2026-12-09' AND category = 'central_bank_meeting'").await, 1);
        assert_eq!(ranges("AND external_id = 'ecb:2026-12-17'").await, 1);
        assert_eq!(ranges("AND planned_start = '2026-12-10T13:30:00Z'").await, 1, "08:30 New York");
        let (st, list) = api.call("GET", "/api/v1/time-ranges?scope_kind=org&limit=50", None).await;
        assert_eq!(st, 200, "{list}");
        assert_eq!(list["count"], 104, "on the calendar: {list}");
        // Read again the same day: nothing asked, nothing doubled.
        pnex_backend::services::markets::calendar::sweep(&ctx, &urls).await.unwrap();
        assert_eq!(ranges("").await, 104);
        // A meeting moved: the range follows, still one.
        db.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("UPDATE time_ranges SET updated_at = now() - interval '2 days' WHERE org_id = {org}"),
        ))
        .await
        .unwrap();
        pnex_backend::services::markets::calendar::sweep(&ctx, &urls).await.unwrap();
        assert_eq!(ranges("").await, 104);

        // Eurostat and PortWatch: dated series.
        for host in ["https://ec.europa.eu", "https://services9.arcgis.com", "https://publicreporting.cftc.gov", "https://api.eia.gov"] {
            db.execute_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("UPDATE object_imports SET source_url = replace(source_url, '{host}', '{base}') WHERE org_id = {org}"),
            ))
            .await
            .unwrap();
        }
        let r = api.run("European statistics (Eurostat)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let energy = object_id(&db, org, "macro_series", "code", "PRC_HICP_MANR.M.RCH_A.NRG.EA").await.expect("series");
        let v = api.observations(&energy, "metric=PRC_HICP_MANR.M.RCH_A.NRG.EA").await;
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(v[0]["period_end"], "2026-09-01");

        // EIA: with the org's key, sent at request time only.
        let r = api.run("Oil and gas statistics (EIA)").await;
        assert_eq!(r["run"]["state_code"], "import-key-required", "{r}");
        let (st, r) = api
            .call("POST", "/api/v1/secrets", Some(json!({"name": "markets_eia_key", "value": "k-eia"})))
            .await;
        assert_eq!(st, 201, "{r}");
        let r = api.run("Oil and gas statistics (EIA)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let crude = object_id(&db, org, "macro_series", "code", "PET.WCESTUS1.W").await.expect("EIA series");
        let stocks = api.observations(&crude, "metric=PET.WCESTUS1.W").await;
        assert_eq!(stocks[0]["value"], 421345.0, "{stocks:?}");
        assert!(stocks[0]["known_at"].as_str().unwrap().starts_with("2026-11-04"), "{stocks:?}");
        let stored = count(
            &db,
            format!("SELECT count(*) AS n FROM object_imports WHERE org_id = {org} AND source_url LIKE '%k-eia%'"),
        )
        .await;
        assert_eq!(stored, 0, "the key is never stored with the URL");

        // CFTC COT: positions of Tuesday, known on Friday (no look-ahead).
        let r = api.run("Positions of traders (CFTC COT)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let gold = object_id(&db, org, "macro_series", "code", "088691").await.expect("COT series");
        let net = api.observations(&gold, "metric=noncommercial_net").await;
        assert_eq!(net[0]["value"], 179548.0, "{net:?}");
        assert!(net[0]["known_at"].as_str().unwrap().starts_with("2026-11-06"), "{net:?}");
        let before = api.observations(&gold, "metric=noncommercial_net&as_of=2026-11-05T00:00:00Z").await;
        assert!(before.is_empty(), "not known before its release: {before:?}");

        let r = api.run("Shipping chokepoints (IMF PortWatch)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let q = asked.lock().unwrap().clone();
        assert!(q[0].contains("chokepoint6"), "PortWatch ids are lower-case: {q:?}");
        assert!(q[0].contains("2000-01-01"), "the whole history the first time: {q:?}");
        let hormuz = object_id(&db, org, "macro_series", "code", "CHOKEPOINT6").await.expect("chokepoint");
        let tankers = api.observations(&hormuz, "metric=n_tanker").await;
        assert_eq!(tankers.len(), 2);
        let (_, obj) = api.call("GET", &format!("/api/v1/ontology/objects/{hormuz}"), None).await;
        assert_eq!(obj["title"], "Strait of Hormuz", "{obj}");
        api.run("Shipping chokepoints (IMF PortWatch)").await;
        let q = asked.lock().unwrap().clone();
        assert!(q[1].contains("2026-10-2"), "then a few days back from the last value: {q:?}");
    })
    .await;
}

/// MK21 (D883, D887): a followed product gets one price series per shop.
#[tokio::test]
#[serial]
async fn open_prices_give_one_series_per_shop() {
    use axum::extract::RawQuery;
    use axum::http::HeaderMap;
    use axum::routing::get;
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
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/consumer_prices/install", None).await;
        assert_eq!(st, 200, "{r}");
        let (st, r) = api
            .call(
                "PATCH",
                "/api/v1/ontology/packs/consumer_prices/settings",
                Some(json!({"values": {"source_contact": CONTACT, "tracked_products": "3017620422003"}})),
            )
            .await;
        assert_eq!(st, 200, "{r}");
        let asked = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
        let log = asked.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/api/v1/prices",
            get(move |RawQuery(q): RawQuery, h: HeaderMap| {
                let log = log.clone();
                async move {
                    let ua = h.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    log.lock().unwrap().push((q.unwrap_or_default(), ua));
                    axum::Json(json!({"items": [
                        {"price": 4.45, "currency": "EUR", "date": "2026-10-06", "location_osm_id": 7, "price_is_discounted": false,
                         "product": {"product_name": "Nutella"}, "location": {"osm_name": "Shop A", "osm_address_city": "Nancy", "osm_address_country_code": "FR"}},
                        {"price": 4.05, "currency": "EUR", "date": "2026-11-05", "location_osm_id": 7, "price_is_discounted": false,
                         "product": {"product_name": "Nutella"}, "location": {"osm_name": "Shop A", "osm_address_city": "Nancy", "osm_address_country_code": "FR"}},
                        {"price": 4.68, "currency": "EUR", "date": "2026-09-13", "location_osm_id": 9, "price_is_discounted": false,
                         "product": {"product_name": "Nutella"}, "location": {"osm_name": "Shop B", "osm_address_city": "La Tronche", "osm_address_country_code": "FR"}}
                    ]}))
                }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);
        db.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("UPDATE object_imports SET source_url = replace(source_url, 'https://prices.openfoodfacts.org', '{base}') WHERE org_id = {org}"),
        ))
        .await
        .unwrap();
        let r = api.run("Product prices (Open Prices)").await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let (q, ua) = asked.lock().unwrap()[0].clone();
        assert!(q.contains("product_code=3017620422003") && q.contains("date__gte=2000-01-01"), "{q}");
        assert!(ua.contains("test@example.com"), "the declared contact: {ua}");
        let product = object_id(&db, org, "consumer_product", "ean", "3017620422003").await.expect("product");
        let (_, obj) = api.call("GET", &format!("/api/v1/ontology/objects/{product}"), None).await;
        assert_eq!(obj["title"], "Nutella", "{obj}");
        let enc = |m: &str| format!("metric={}", m.replace(' ', "%20").replace(',', "%2C").replace('(', "%28").replace(')', "%29"));
        let a = api.observations(&product, &enc("Shop A, Nancy FR (7)")).await;
        assert_eq!(a.len(), 2, "{a:?}");
        assert_eq!(a[0]["unit"], "EUR");
        assert_eq!(a[0]["value"], 4.05);
        let b = api.observations(&product, &enc("Shop B, La Tronche FR (9)")).await;
        assert_eq!(b.len(), 1);
    })
    .await;
}

fn zip_of(name: &str, text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    w.start_file(name, opts).unwrap();
    w.write_all(text.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

fn gz_of(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// MK22–MK24 (D889, Q-F7, Q-F8): AIS, ADS-B and DVF files read as they
/// arrive into tables, one transaction per file, committed only whole.
#[tokio::test]
#[serial]
async fn bulk_files_become_tables() {
    use axum::extract::Path;
    use axum::routing::get;
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
        for (pack, values) in [
            ("trade_flows", json!({"source_contact": CONTACT, "ais_dk_from": "2026-11-01", "ais_noaa_from": "2026-11-01", "adsb_from": "2026-11-01"})),
            ("real_estate", json!({"dvf_from": "2025"})),
        ] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{r}");
            let (st, r) = api
                .call("PATCH", &format!("/api/v1/ontology/packs/{pack}/settings"), Some(json!({ "values": values })))
                .await;
            assert_eq!(st, 200, "{r}");
        }
        // Only an org whose quota a platform administrator set loads bulk files.
        let quota = |org: i64| {
            Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "INSERT INTO system_settings (key, value, created_at, updated_at) \
                     VALUES ('dataset_quota_bytes:{org}', '30000000000', now(), now()) ON CONFLICT (key) DO NOTHING"
                ),
            )
        };
        pnex_backend::services::bulk_history::sweep_with(&ctx, chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(), Some("http://127.0.0.1:9"))
            .await
            .unwrap();
        assert_eq!(
            count(&db, format!("SELECT count(*) AS n FROM market_history_partitions WHERE org_id = {org}")).await,
            0,
            "no quota set: nothing asked"
        );
        db.execute_raw(quota(org)).await.unwrap();
        let dk = zip_of(
            "aisdk-2026-11-01.csv",
            "# Timestamp,Type of mobile,MMSI,Latitude,Longitude,Navigational status,ROT,SOG,COG,Heading,IMO,Callsign,Name,Ship type,Cargo type,Width,Length,Type of position fixing device,Draught,Destination,ETA,Data source type,A,B,C,D\n\
             01/11/2026 00:00:00,Base Station,2190064,56.716647,11.518977,Unknown value,,,,,Unknown,Unknown,,Undefined,,,,GPS,,Unknown,,AIS,,,,\n\
             01/11/2026 00:00:01,Class A,305287000,57.402440,8.739752,Under way using engine,0.0,15.7,225.0,222,Unknown,Unknown,NORD STAR,Cargo,,32,190,GPS,9.5,ROTTERDAM,,AIS,,,,\n\
             01/11/2026 00:00:02,Class B,219573000,56.161667,8.014000,Unknown value,,5.6,82.0,72,Unknown,Unknown,,Pleasure,,,,GPS,,Unknown,,AIS,,,,\n",
        );
        // The next day's file is cut: never committed.
        let cut = dk[..dk.len() / 2].to_vec();
        let mut tar = tar::Builder::new(Vec::new());
        for (path, data) in [
            ("./heatmap/41.bin.ttf", b"binary".to_vec()),
            (
                "./traces/ab/trace_full_ab32ab.json",
                gz_of(br#"{"icao":"ab32ab","r":"N820UP","t":"B350","timestamp":1793491200.0,"trace":[[1.0,31.2,-81.3,3775,144.2,356.8,5,3008,null,"adsb_icao"],[2.0,31.21,-81.31,"ground",3.0,10.0,0,null,null,"mlat"]]}"#),
            ),
            ("./traces/cd/trace_full_0d11cd.json", br#"{"icao":"0d11cd","timestamp":1793491200.0,"trace":[[5.0,19.4,-99.0,6950,67.2,135.6,5,320,null,"adsb_icao"]]}"#.to_vec()),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, &data[..]).unwrap();
        }
        let whole = tar.into_inner().unwrap();
        let (aa, ab) = whole.split_at(whole.len() / 3);
        let (aa, ab) = (aa.to_vec(), ab.to_vec());
        let dvf = gz_of(
            b"id_mutation,date_mutation,numero_disposition,nature_mutation,valeur_fonciere,adresse_numero,adresse_suffixe,adresse_nom_voie,adresse_code_voie,code_postal,code_commune,nom_commune,code_departement,ancien_code_commune,ancien_nom_commune,id_parcelle,ancien_id_parcelle,numero_volume,lot1_numero,lot1_surface_carrez,lot2_numero,lot2_surface_carrez,lot3_numero,lot3_surface_carrez,lot4_numero,lot4_surface_carrez,lot5_numero,lot5_surface_carrez,nombre_lots,code_type_local,type_local,surface_reelle_bati,nombre_pieces_principales,code_nature_culture,nature_culture,code_nature_culture_speciale,nature_culture_speciale,surface_terrain,longitude,latitude\n\
2025-1,2025-01-03,000001,Vente,430500,88,,RUE MARCADET,5995,75018,75118,Paris 18e Arrondissement,75,,,75118000BJ0092,,,226,49.48,,,,,,,,,1,2,Appartement,49,2,,,,,,2.346344,48.890689\n\
2025-2,2025-02-10,000001,Vente,250000,1,,RUE X,1,54000,54395,Nancy,54,,,54395000AB0001,,,,,,,,,,,,,0,1,Maison,90,4,S,sols,,,300,6.18,48.69\n",
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route("/aisdk-2026-11-01.zip", get(move || { let b = dk.clone(); async move { b } }))
            .route("/aisdk-2026-11-02.zip", get(move || { let b = cut.clone(); async move { b } }))
            .route(
                "/adsblol/globe_history_2026/releases/download/{tag}/{file}",
                get(move |Path((_, file)): Path<(String, String)>| {
                    let (aa, ab) = (aa.clone(), ab.clone());
                    async move {
                        match file.rsplit('.').next() {
                            Some("aa") => (axum::http::StatusCode::OK, aa),
                            Some("ab") => (axum::http::StatusCode::OK, ab),
                            _ => (axum::http::StatusCode::NOT_FOUND, Vec::new()),
                        }
                    }
                }),
            )
            .route("/geo-dvf/latest/csv/2025/full.csv.gz", get(move || { let b = dvf.clone(); async move { b } }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);

        let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        pnex_backend::services::bulk_history::sweep_with(&ctx, day(2), Some(&base)).await.unwrap();
        let part = |src: &str| {
            let db = db.clone();
            let src = src.to_string();
            async move {
                db.query_all_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!(
                        "SELECT period_start::text AS d, state, coalesce(code, '') AS code, rows FROM market_history_partitions \
                         WHERE org_id = {org} AND source = '{src}' ORDER BY period_start"
                    ),
                ))
                .await
                .unwrap()
                .iter()
                .map(|r| {
                    let d: String = r.try_get("", "d").unwrap();
                    let s: String = r.try_get("", "state").unwrap();
                    let c: String = r.try_get("", "code").unwrap();
                    let n: i64 = r.try_get("", "rows").unwrap();
                    format!("{d} {s}{c} {n}")
                })
                .collect::<Vec<_>>()
            }
        };
        assert_eq!(part("ais_dk").await, ["2026-11-01 done 2"], "vessels only, the base station left out");
        assert_eq!(part("adsb_lol").await, ["2026-11-01 done 3"], "two parts read as one tar");
        assert_eq!(part("dvf").await, ["2025-01-01 done 2"]);
        assert!(part("ais_noaa").await.is_empty(), "NOAA waits on a day not yet published");
        let tables = count(
            &db,
            format!(
                "SELECT count(*) AS n FROM datasets WHERE org_id = {org} \
                 AND name IN ('ais_dk_positions', 'adsb_lol_positions', 'dvf_mutations')"
            ),
        )
        .await;
        assert_eq!(tables, 3);

        // The next day: the cut file is refused and its transaction aborted.
        pnex_backend::services::bulk_history::sweep_with(&ctx, day(3), Some(&base)).await.unwrap();
        assert_eq!(part("ais_dk").await, ["2026-11-01 done 2", "2026-11-02 failedunreadable 0"]);
        let open = count(
            &db,
            format!("SELECT count(*) AS n FROM dataset_transactions WHERE state = 'committed' AND dataset_id IN (SELECT id FROM datasets WHERE org_id = {org} AND name = 'ais_dk_positions')"),
        )
        .await;
        assert_eq!(open, 1, "only the whole file is committed");
        let (st, sample) = api
            .call("GET", "/api/v1/datasets?limit=50", None)
            .await;
        assert_eq!(st, 200, "{sample}");
    })
    .await;
}

/// The real files, by hand: `BULK_REAL=dvf|ais_dk|adsb cargo test -p
/// pnex-backend --test markets bulk_real -- --ignored --nocapture`.
#[tokio::test]
#[serial]
#[ignore = "downloads hundreds of MB to several GB"]
async fn bulk_real_files() {
    let which = std::env::var("BULK_REAL").unwrap_or_else(|_| "dvf".into());
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
        let today = chrono::Utc::now().date_naive();
        let yesterday = std::env::var("BULK_FROM").unwrap_or_else(|_| (today - chrono::Duration::days(2)).to_string());
        let (pack, values) = match which.as_str() {
            "dvf" => ("real_estate", json!({"dvf_from": (chrono::Datelike::year(&today) - 1).to_string()})),
            "ais_dk" => ("trade_flows", json!({"ais_dk_from": yesterday})),
            _ => ("trade_flows", json!({"adsb_from": "2025-12-30"})),
        };
        api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
        let (st, r) = api
            .call("PATCH", &format!("/api/v1/ontology/packs/{pack}/settings"), Some(json!({ "values": values })))
            .await;
        assert_eq!(st, 200, "{r}");
        db.execute_raw(Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "INSERT INTO system_settings (key, value, created_at, updated_at) \
                 VALUES ('dataset_quota_bytes:{org}', '30000000000', now(), now()) ON CONFLICT (key) DO NOTHING"
            ),
        ))
        .await
        .unwrap();
        let started = std::time::Instant::now();
        pnex_backend::services::bulk_history::sweep(&ctx, today).await.unwrap();
        let rows = db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT source, period_start::text AS d, state, coalesce(code, '') AS code, rows, bytes FROM market_history_partitions WHERE org_id = {org}"),
            ))
            .await
            .unwrap();
        for r in rows {
            println!(
                "{} {} {} {} rows={} bytes={} in {:?}",
                r.try_get::<String>("", "source").unwrap(),
                r.try_get::<String>("", "d").unwrap(),
                r.try_get::<String>("", "state").unwrap(),
                r.try_get::<String>("", "code").unwrap(),
                r.try_get::<i64>("", "rows").unwrap(),
                r.try_get::<i64>("", "bytes").unwrap(),
                started.elapsed()
            );
        }
    })
    .await;
}

/// The gold case on the real sources, by hand (appendix F.6 bis):
/// `MARKETS_CONTACT="Name mail" cargo test -p pnex-backend --test markets
/// gold_real -- --ignored --nocapture`. Prints what each source gave.
#[tokio::test]
#[serial]
#[ignore = "reaches FRED, the BIS, the CFTC, Eurostat, the BLS, the Fed, the ECB and HistData"]
async fn gold_real_sources() {
    let contact = std::env::var("MARKETS_CONTACT").expect("MARKETS_CONTACT");
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::markets::history::sweep;
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
        api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        api.settings(json!({
            "source_contact": contact, "covered_tickers": "",
            "macro_series": "DFII10, T10YIE, DTWEXBGS, FEDFUNDS",
            "bis_series": "WS_CBPOL.D.US, WS_CBPOL.D.XM, WS_CBPOL.D.JP",
            "cot_markets": "088691", "eurostat_series": "prc_hicp_minr.M.RCH_A.AP.EA",
            "calendars": "bls, fomc, ecb", "histdata_pairs": "XAUUSD", "histdata_from": "2025",
        }))
        .await;
        for name in [
            "Macroeconomic series (FRED)",
            "Central bank series (BIS)",
            "Positions of traders (CFTC COT)",
            "European statistics (Eurostat)",
        ] {
            let started = std::time::Instant::now();
            let r = api.run(name).await;
            println!("{name}: {} in {:?}: {}", r["run"]["state"], started.elapsed(), r["run"]["counts"]);
            assert_eq!(r["run"]["state"], "applied", "{r}");
        }
        let per_metric = db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!(
                    "SELECT metric, count(*) AS n, min(period_end)::text AS a, max(period_end)::text AS b \
                     FROM market_observations WHERE org_id = {org} GROUP BY metric ORDER BY metric"
                ),
            ))
            .await
            .unwrap();
        for r in per_metric {
            println!(
                "  {:<40} {:>6} values {} .. {}",
                r.try_get::<String>("", "metric").unwrap(),
                r.try_get::<i64>("", "n").unwrap(),
                r.try_get::<String>("", "a").unwrap(),
                r.try_get::<String>("", "b").unwrap()
            );
        }
        let moves = count(&db, format!("SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'market_event'")).await;
        println!("market moves (rate decisions): {moves}");
        pnex_backend::services::markets::calendar::sweep(&ctx, &Default::default()).await.unwrap();
        let ranges = count(&db, format!("SELECT count(*) AS n FROM time_ranges WHERE org_id = {org} AND scope_kind = 'org'")).await;
        println!("calendar ranges: {ranges}");
        assert!(ranges > 0);
        let today = chrono::Utc::now().date_naive();
        sweep(&ctx, today).await.unwrap();
        let bars = db
            .query_all_raw(Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT period_start::text AS d, state, rows FROM market_history_partitions WHERE org_id = {org} AND source = 'histdata'"),
            ))
            .await
            .unwrap();
        for r in bars {
            println!(
                "XAUUSD {} {} rows={}",
                r.try_get::<String>("", "d").unwrap(),
                r.try_get::<String>("", "state").unwrap(),
                r.try_get::<i64>("", "rows").unwrap()
            );
        }
    })
    .await;
}

/// Minute bars of `instrument` from `start` (µs), one per minute, a close
/// rising by 0.01 a minute, as the `ohlcv` shape (D809).
fn minute_rows(instrument: &str, source: &str, start_us: i64, n: i64) -> arrow_array::RecordBatch {
    use pnex_core::markets::history::Kline;
    let u: i128 = 100_000_000;
    let rows: Vec<Kline> = (0..n)
        .map(|i| {
            let c = 2000 * u + i128::from(i) * u / 100;
            Kline {
                open_us: start_us + i * 60_000_000,
                open: c,
                high: c + u,
                low: c - u,
                close: c,
                volume: u,
                quote_volume: None,
                trades: None,
            }
        })
        .collect();
    let names = vec![instrument; rows.len()];
    pnex_backend::services::markets::history::ohlcv_batch("1m", source, &names, &rows).unwrap()
}

/// Appendix G (MK27–MK29): a service account reads bars as known at a date
/// and posts decisions, its only write; PNeX checks, stores and measures them,
/// and draws them on the instrument's chart.
#[tokio::test]
#[serial]
async fn an_external_model_reads_bars_and_posts_decisions() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::datasets;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/markets/install", None).await;
        assert_eq!(st, 200, "{r}");

        // 100 days of XAUUSD minutes from 2026-01-05 (HistData), and 3 days
        // up to now; plus an MT5 hour written twice (a backfill).
        let store = datasets::store_of(&ctx.config).unwrap();
        let hist = datasets::create(&ctx.db, org, "histdata_ohlcv_1m").await.unwrap();
        let hist_id: uuid::Uuid = hist.id.parse().unwrap();
        let start = chrono::DateTime::parse_from_rfc3339("2026-01-05T00:00:00Z").unwrap().timestamp_micros();
        let min = 60_000_000i64;
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("XAUUSD", "histdata", start, 100 * 1440))
            .await
            .unwrap();
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("EURUSD", "histdata", start, 1440))
            .await
            .unwrap();
        let now = chrono::Utc::now().timestamp_micros();
        let recent = now - now.rem_euclid(min) - 3 * 1440 * min;
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("XAUUSD", "histdata", recent, 3 * 1440))
            .await
            .unwrap();
        let mt5 = datasets::create(&ctx.db, org, "mt5_ohlcv_1m").await.unwrap();
        let mt5_id: uuid::Uuid = mt5.id.parse().unwrap();
        for _ in 0..2 {
            datasets::append_batch(&ctx.db, &store, org, mt5_id, &minute_rows("XAUUSD", "mt5:Demo", start, 60))
                .await
                .unwrap();
        }
        ctx.db
            .execute_unprepared(&format!(
                "INSERT INTO market_history_partitions (org_id, source, instrument, period, period_start, state) \
                 VALUES ({org}, 'histdata', 'XAUUSD', 'year', '2026-01-01', 'done')"
            ))
            .await
            .unwrap();

        // The runner: a service account with a role that reads and decides.
        let (st, sa) = api.call("POST", "/api/v1/iam/service-accounts", Some(json!({ "name": "gold-runner" }))).await;
        assert_eq!(st, 201, "{sa}");
        let sa_id = sa["id"].as_str().unwrap().to_string();
        let (st, role) = api
            .call("POST", "/api/v1/iam/roles", Some(json!({
                "key": "decision-runner", "name": "Decision runner",
                "statements": [{ "effect": "allow", "permissions": [
                    "datasets:read", "ontology:read", "time-ranges:read",
                    "market-decisions:read", "market-decisions:write", "taxonomies:write"
                ] }]
            })))
            .await;
        assert_eq!(st, 201, "{role}");
        let (st, a) = api
            .call("POST", "/api/v1/iam/assignments", Some(json!({ "service_account_id": sa_id, "role_id": role["id"] })))
            .await;
        assert_eq!(st, 201, "{a}");
        let expires = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        let (st, key) = api
            .call("POST", &format!("/api/v1/iam/service-accounts/{sa_id}/keys"), Some(json!({ "expires_at": expires })))
            .await;
        assert_eq!(st, 201, "{key}");
        let secret = key["secret"].as_str().unwrap().to_string();
        let runner = |method: &str, path: &str, body: Option<Value>| {
            let req = match method {
                "GET" => server.get(path),
                _ => server.post(path),
            }
            .add_header("Authorization", format!("Bearer {secret}"));
            async move {
                let res = match body {
                    Some(b) => req.json(&b).await,
                    None => req.await,
                };
                (res.status_code().as_u16(), serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null))
            }
        };

        // 500 H4 bars as known at a date; the bar in progress never shows.
        let close_of = |h: i64| chrono::DateTime::from_timestamp_micros(start + h * 3_600 * 1_000_000).unwrap();
        let as_of = close_of(4 * 520).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let path = format!("/api/v1/markets/bars?instrument=XAUUSD&source=histdata&interval=4h&limit=500&as_of={as_of}");
        let (st, page) = runner("GET", &path, None).await;
        assert_eq!(st, 200, "{page}");
        let bars = page["results"].as_array().unwrap();
        assert_eq!(bars.len(), 500);
        assert_eq!(bars[499]["ts"], close_of(4 * 519).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        // First bar: open of minute 4h·20, close 4 hours of minutes later.
        let first_min = 4 * 20 * 60;
        assert_eq!(bars[0]["open"], format!("{}", 2000.0 + first_min as f64 / 100.0));
        assert_eq!(bars[0]["volume"], "240");
        let minute_before = (close_of(4 * 520) - chrono::Duration::minutes(1)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let path = format!("/api/v1/markets/bars?instrument=XAUUSD&source=histdata&interval=4h&limit=500&as_of={minute_before}");
        let (_, page) = runner("GET", &path, None).await;
        let last = page["results"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["ts"], close_of(4 * 518).to_rfc3339_opts(chrono::SecondsFormat::Secs, true), "one minute short: not closed");
        // A range, and the MT5 hour written twice counted once.
        let from = close_of(0).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let (st, page) = runner("GET", &format!("/api/v1/markets/bars?instrument=XAUUSD&source=mt5:Demo&interval=1h&from={from}"), None).await;
        assert_eq!(st, 200, "{page}");
        assert_eq!(page["count"], 1);
        assert_eq!(page["results"][0]["volume"], "60");
        let (_, page) = runner("GET", &format!("/api/v1/markets/bars?instrument=XAUUSD&source=mt5:Demo&interval=1m&from={from}"), None).await;
        assert_eq!(page["count"], 60);
        let (st, e) = runner("GET", "/api/v1/markets/bars?instrument=XAUUSD&source=elsewhere&interval=4h", None).await;
        assert_eq!((st, e["errors"]["fields"]["source"].as_str()), (400, Some("invalid")), "{e}");
        let (st, e) = runner("GET", "/api/v1/markets/bars?instrument=XAUUSD&source=histdata&interval=3h", None).await;
        assert_eq!(st, 400, "{e}");

        // Decisions: 100 by one runner, each its own version.
        let bar_now = now - now.rem_euclid(4 * 3_600 * 1_000_000);
        let as_of_now = chrono::DateTime::from_timestamp_micros(bar_now).unwrap().to_rfc3339();
        let decision = |version: &str, p: [f64; 3]| {
            json!({
                "model": "gold", "model_version": version, "instrument": "XAUUSD", "source": "histdata",
                "interval": "4h", "horizon_bars": 6, "as_of": as_of_now,
                "options": ["buy", "neutral", "sell"], "probabilities": p,
                "barriers": { "take_profit_atr": 1.5, "stop_atr": 1.0, "atr_period": 14 },
                "inputs": { "last_bar": "x" }
            })
        };
        for i in 0..100 {
            let (st, d) = runner("POST", "/api/v1/markets/decisions", Some(decision(&format!("v{i}"), [0.7, 0.2, 0.1]))).await;
            assert_eq!(st, 201, "{d}");
            assert_eq!(d["best"], "buy");
            assert!(d["confidence"].as_f64().unwrap() > 0.0);
        }
        let (st, _) = runner("POST", "/api/v1/markets/decisions", Some(decision("v0", [0.7, 0.2, 0.1]))).await;
        assert_eq!(st, 200, "the same again");
        let (st, e) = runner("POST", "/api/v1/markets/decisions", Some(decision("v0", [0.1, 0.2, 0.7]))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("market-decision-conflict")));
        let (st, e) = runner("POST", "/api/v1/markets/decisions", Some(decision("bad", [0.5, 0.5, 0.5]))).await;
        assert_eq!((st, e["errors"]["fields"]["probabilities"].as_str()), (400, Some("sum:1")), "{e}");
        let mut stale = decision("old", [0.7, 0.2, 0.1]);
        stale["as_of"] = json!(chrono::DateTime::from_timestamp_micros(bar_now - 9 * 3_600 * 1_000_000).unwrap().to_rfc3339());
        let (st, e) = runner("POST", "/api/v1/markets/decisions", Some(stale)).await;
        assert_eq!((st, e["error"].as_str()), (400, Some("market-decision-stale")), "{e}");
        let mut unknown = decision("x", [0.7, 0.2, 0.1]);
        unknown["instrument"] = json!("NOPE");
        let (st, e) = runner("POST", "/api/v1/markets/decisions", Some(unknown)).await;
        assert_eq!((st, e["errors"]["fields"]["instrument"].as_str()), (400, Some("unknown")), "{e}");

        // Never another write, whatever the role says (D891).
        let (st, _) = runner("POST", "/api/v1/taxonomies", Some(json!({ "name": "nope" }))).await;
        assert_eq!(st, 403);
        let (st, list) = runner("GET", "/api/v1/markets/decisions?model=gold&limit=5", None).await;
        assert_eq!((st, list["count"].as_i64()), (200, Some(100)), "{list}");

        // Scores: past decisions with known outcomes, on the histdata bars
        // (the close rises 2.4 a 4h bar, whose range is about 4.4: the ATR).
        for (i, p) in [[0.8, 0.1, 0.1], [0.1, 0.1, 0.8]].iter().enumerate() {
            let at = close_of(4 * (100 + i as i64)).to_rfc3339();
            ctx.db
                .execute_unprepared(&format!(
                    "INSERT INTO market_decisions (id, org_id, model, model_version, instrument, source, interval, \
                       horizon_bars, as_of, probabilities, best, confidence, barriers) VALUES \
                     ('{}', {org}, 'past', '1', 'XAUUSD', 'histdata', '4h', 6, '{at}', ARRAY[{},{},{}]::float8[], \
                       '{}', 0.5, '{{\"take_profit_atr\": 1.0, \"stop_atr\": 1.0, \"atr_period\": 14}}')",
                    uuid::Uuid::new_v4(), p[0], p[1], p[2], if i == 0 { "buy" } else { "sell" }
                ))
                .await
                .unwrap();
        }
        let (st, s) = api.call("GET", "/api/v1/markets/decisions/scores?model=past", None).await;
        assert_eq!(st, 200, "{s}");
        let s = &s["results"][0]["scores"];
        assert_eq!((s["decisions"].as_i64(), s["measured"].as_i64()), (Some(2), Some(2)), "{s}");
        assert_eq!(s["hit_rate"].as_f64(), Some(0.5), "a steady rise hits the upper barrier: buy");
        let (_, s) = api.call("GET", "/api/v1/markets/decisions/scores?model=gold", None).await;
        assert_eq!(s["count"], 100);
        assert_eq!(s["results"][0]["scores"]["measured"], 0, "the horizon has not passed");

        // The chart of the instrument: daily closes from the bars, decisions as markers.
        let (st, inst) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({
                "type_key": "instrument", "title": "Gold", "properties": { "ticker": "XAUUSD" }
            })))
            .await;
        assert_eq!(st, 201, "{inst}");
        let (st, chart) = api
            .call("GET", &format!("/api/v1/markets/issuers/{}/chart?days=3650", inst["id"].as_str().unwrap()), None)
            .await;
        assert_eq!(st, 200, "{chart}");
        assert!(chart["closes"].as_array().unwrap().len() >= 100, "{}", chart["closes"]);
        let marks: Vec<&Value> = chart["events"].as_array().unwrap().iter().filter(|e| e["kind"] == "decision").collect();
        assert!(marks.len() >= 3, "{chart}");
        assert!(marks.iter().any(|m| m["model"] == "past 1" && m["probability"] == 0.8), "{marks:?}");
    })
    .await;
}

/// Appendix H (D900, D901): canaries written after T0 never show in a
/// point-in-time read at T0, on every `point_in_time` route of the
/// inventory; a status changed after T0 reads as it was. A route added to
/// the inventory without its canary fails here.
#[tokio::test]
#[serial]
async fn point_in_time_reads_never_show_a_canary() {
    with_app(|server, ctx, token| async move {
        use pnex_api_contract::as_of::{AsOf, SEMANTICS};
        use pnex_backend::services::datasets;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        for pack in ["markets", "world_events", "prediction_markets", "trading_accounts"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{r}");
        }

        // Before T0: a day of minutes, a reported event, a macro series and
        // its first value.
        let min = 60_000_000i64;
        let now_us = chrono::Utc::now().timestamp_micros();
        let day_start = now_us - now_us.rem_euclid(min) - 2 * 1440 * min;
        let store = datasets::store_of(&ctx.config).unwrap();
        let hist = datasets::create(&ctx.db, org, "histdata_ohlcv_1m").await.unwrap();
        let hist_id: uuid::Uuid = hist.id.parse().unwrap();
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("XAUUSD", "histdata", day_start, 1440))
            .await
            .unwrap();
        ctx.db
            .execute_unprepared(&format!(
                "INSERT INTO market_history_partitions (org_id, source, instrument, period, period_start, state) \
                 VALUES ({org}, 'histdata', 'XAUUSD', 'year', date_trunc('year', now())::date, 'done'), \
                        ({org}, 'histdata', 'CANARY', 'year', date_trunc('year', now())::date, 'done')"
            ))
            .await
            .unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        let (st, event) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({
                "type_key": "geo_event", "title": "Border clash",
                "properties": { "ref": "pit-1", "status": "unconfirmed", "known_at": old }
            })))
            .await;
        assert_eq!(st, 201, "{event}");
        let event_id = event["id"].as_str().unwrap().to_string();
        let (st, gold) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({
                "type_key": "instrument", "title": "Gold", "properties": { "ticker": "XAUUSD" }
            })))
            .await;
        assert_eq!(st, 201, "{gold}");
        let gold_id = gold["id"].as_str().unwrap().to_string();
        let create = |body: Value| {
            let api = &api;
            async move {
                let (st, r) = api.call("POST", "/api/v1/ontology/objects", Some(body)).await;
                assert_eq!(st, 201, "{r}");
                r["id"].as_str().unwrap().to_string()
            }
        };
        let link = |kind: &'static str, from: String, to: String| {
            let api = &api;
            async move {
                let (st, r) = api
                    .call("POST", "/api/v1/ontology/links", Some(json!({
                        "link_type": kind, "source_id": from, "target_id": to,
                        "valid_from": "2025-01-01T00:00:00Z"
                    })))
                    .await;
                assert_eq!(st, 201, "{r}");
                r["id"].as_i64().unwrap()
            }
        };
        // Links known before T0: an exposure and an ownership (D916).
        let iran = create(json!({ "type_key": "nation", "title": "Iran", "properties": { "code": "IR" } })).await;
        let exposure = link("exposed_to", iran.clone(), gold_id.clone()).await;
        let holding = create(json!({ "type_key": "organization", "title": "Holding" })).await;
        let sub = create(json!({ "type_key": "organization", "title": "Subsidiary" })).await;
        let owns = link("owns", holding.clone(), sub.clone()).await;
        // A prediction market bearing on gold, its price 0.4 before T0 (J).
        let pm_market = |cid: &str, title: &str, known: &str| {
            json!({ "type_key": "prediction_market", "title": title, "properties": {
                "condition_id": cid, "outcomes": "Yes,No", "tokens": format!("{}1,{}2", &cid[2..6], &cid[2..6]),
                "known_at": known } })
        };
        let fed_cid = format!("0x{}", "b".repeat(64));
        let fed = create(pm_market(&fed_cid, "Fed cut", &old)).await;
        link("bears_on", fed.clone(), gold_id.clone()).await;
        let price = |cid: &str, token: &str, at: String, p: f64| {
            let (db, cid, token) = (ctx.db.clone(), cid.to_string(), token.to_string());
            async move {
                db.execute_unprepared(&format!(
                    "INSERT INTO prediction_prices (org_id, condition_id, token, outcome, fidelity, ts, price, known_at) \
                     VALUES ({org}, '{cid}', '{token}', 'Yes', 60, '{at}', {p}, '{at}')"
                ))
                .await
                .unwrap();
            }
        };
        price(&fed_cid, "bbbb1", (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339(), 0.4).await;
        let (st, series) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({
                "type_key": "macro_series", "title": "CPI", "properties": { "code": "CPI" }
            })))
            .await;
        assert_eq!(st, 201, "{series}");
        let series_id = series["id"].as_str().unwrap().to_string();
        let observe = |known: String, value: f64| {
            let (db, sid) = (ctx.db.clone(), series_id.clone());
            async move {
                db.execute_unprepared(&format!(
                    "INSERT INTO market_observations (org_id, object_id, metric, period_start, period_end, known_at, value, source_ref) \
                     VALUES ({org}, '{sid}', 'value', '2026-01-01', '2026-01-31', '{known}', {value}, 'test')"
                ))
                .await
                .unwrap();
            }
        };
        observe(old.clone(), 1.0).await;
        // A public account's trade, imported before T0 (D928).
        let import_trade = |comment: &'static str, known: chrono::DateTime<chrono::Utc>| {
            let db = ctx.db.clone();
            async move {
                let trade = pnex_core::markets::accounts::Trade {
                    trade_key: format!("ticket:{comment}"),
                    // No instrument object: no link to change the other reads.
                    symbol: "EURUSD".into(),
                    buy: false,
                    lots: Some(0.1),
                    open_at: known.timestamp() - 3_600,
                    close_at: None,
                    open_price: 2_000.0,
                    close_price: None,
                    sl: None,
                    tp: None,
                    profit: None,
                    pips: None,
                    commission: None,
                    swap: None,
                    gain_pct: None,
                    magic: None,
                    comment: Some(comment.into()),
                };
                let actor = pnex_backend::services::ontology::Actor::manual(1, pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole::Owner);
                pnex_backend::services::markets::accounts::import(&db, org, &actor, "myfxbook:777", Some("Pseudo"), None, &[trade], "test.csv", known)
                    .await
                    .unwrap();
            }
        };
        import_trade("early", chrono::Utc::now() - chrono::Duration::days(3)).await;

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let t0 = chrono::Utc::now();
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let canary_at = (t0 + chrono::Duration::minutes(1)).to_rfc3339();

        // After T0: one canary per source.
        import_trade("CANARY", t0 + chrono::Duration::minutes(1)).await;
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("CANARY", "histdata", day_start, 30))
            .await
            .unwrap();
        datasets::append_batch(&ctx.db, &store, org, hist_id, &minute_rows("XAUUSD", "histdata", t0.timestamp_micros() - t0.timestamp_micros().rem_euclid(min), 1))
            .await
            .unwrap();
        let (st, r) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({
                "type_key": "geo_event", "title": "CANARY event",
                "properties": { "ref": "pit-2", "status": "unconfirmed", "known_at": canary_at }
            })))
            .await;
        assert_eq!(st, 201, "{r}");
        let (st, r) = api
            .call("PUT", &format!("/api/v1/ontology/objects/{event_id}"), Some(json!({
                "type_key": "geo_event", "title": "Border clash",
                "properties": { "ref": "pit-1", "status": "confirmed", "known_at": old },
                "expected_version": event["version"]
            })))
            .await;
        assert_eq!(st, 200, "{r}");
        observe(canary_at.clone(), 31415.9).await;
        // Links: closed after T0 back to 2025 (still open as known at T0),
        // written after T0 since 2025 (unknown at T0), an end renamed.
        for id in [exposure, owns] {
            let (st, r) = api
                .call("POST", &format!("/api/v1/ontology/links/{id}/close"), Some(json!({ "valid_to": "2025-06-01T00:00:00Z" })))
                .await;
            assert_eq!(st, 200, "{r}");
        }
        let canary_land = create(json!({ "type_key": "nation", "title": "CANARY land", "properties": { "code": "ZZ" } })).await;
        link("exposed_to", canary_land, gold_id.clone()).await;
        // A later price, and a market read after T0 linked "since 2025".
        price(&fed_cid, "bbbb1", canary_at.clone(), 0.31415).await;
        let canary_cid = format!("0x{}", "c".repeat(64));
        let canary_market = create(pm_market(&canary_cid, "CANARY market", &canary_at)).await;
        link("bears_on", canary_market, gold_id.clone()).await;
        let canary_co = create(json!({ "type_key": "organization", "title": "CANARY Co" })).await;
        link("owns", holding.clone(), canary_co).await;
        let (st, r) = api
            .call("PUT", &format!("/api/v1/ontology/objects/{gold_id}"), Some(json!({
                "type_key": "instrument", "title": "Gold CANARY", "properties": { "ticker": "XAUUSD" },
                "expected_version": gold["version"]
            })))
            .await;
        assert_eq!(st, 200, "{r}");

        let at = t0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let enc = |s: &str| s.replace('+', "%2B");
        let ds = hist.id.clone();
        let (_, files_before) = api.call("GET", &format!("/api/v1/datasets/{ds}/files?as_of={}", enc(&at)), None).await;
        let n_before = files_before["count"].as_i64().unwrap();
        let (_, files_now) = api.call("GET", &format!("/api/v1/datasets/{ds}/files"), None).await;
        assert_eq!(files_now["count"].as_i64().unwrap(), n_before + 2, "both canary files are there now");

        // Every point-in-time route, read at T0.
        let reads: Vec<(&str, String, Option<Value>)> = vec![
            ("datasets_files_list", format!("/api/v1/datasets/{ds}/files?limit=500&as_of={}", enc(&at)), None),
            ("datasets_files_get", format!("/api/v1/datasets/{ds}/files/{n_before}?as_of={}", enc(&at)), None),
            ("datasets_sample", format!("/api/v1/datasets/{ds}/sample?limit=1000&as_of={}", enc(&at)), None),
            (
                "markets_bars",
                format!("/api/v1/markets/bars?instrument=XAUUSD&source=histdata&interval=1m&limit=5000&as_of={}", enc(&at)),
                None,
            ),
            ("ontology_objects_get", format!("/api/v1/ontology/objects/{event_id}?as_of={}", enc(&at)), None),
            ("ontology_objects_links", format!("/api/v1/ontology/objects/{gold_id}/links?as_of={}", enc(&at)), None),
            (
                "ontology_objects_hierarchy",
                format!("/api/v1/ontology/objects/{holding}/hierarchy?key=group&at={}", enc(&at)),
                None,
            ),
            ("ontology_objects_observations", format!("/api/v1/ontology/objects/{series_id}/observations?as_of={}", enc(&at)), None),
            (
                "ontology_query",
                "/api/v1/ontology/query".into(),
                Some(json!({ "type_key": "geo_event", "as_of": at, "limit": 500 })),
            ),
            (
                "ontology_query",
                "/api/v1/ontology/query".into(),
                Some(json!({ "ids": [gold_id], "traverse": [{ "link_type": "exposed_to", "direction": "in" }], "as_of": at })),
            ),
            (
                "markets_chains",
                format!("/api/v1/markets/chains?instrument=XAUUSD&source=histdata&as_of={}", enc(&at)),
                None,
            ),
            (
                "markets_prediction_features",
                format!("/api/v1/markets/prediction-features?instrument=XAUUSD&as_of={}", enc(&at)),
                None,
            ),
            ("markets_account_trades", format!("/api/v1/markets/account-trades?as_of={}", enc(&at)), None),
        ];
        let (_, t_then) = api.call("GET", &format!("/api/v1/markets/account-trades?as_of={}", enc(&at)), None).await;
        assert_eq!(t_then["count"], 1, "the trade imported before T0 is there: {t_then}");
        let later = (t0 + chrono::Duration::minutes(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (_, t_now) = api.call("GET", &format!("/api/v1/markets/account-trades?account=myfxbook:777&as_of={}", enc(&later)), None).await;
        assert_eq!(t_now["count"], 2, "both trades once the canary is known: {t_now}");
        let covered: std::collections::BTreeSet<&str> = reads.iter().map(|(id, ..)| *id).collect();
        let inventory: std::collections::BTreeSet<&str> = SEMANTICS
            .iter()
            .filter(|(_, s)| matches!(s, AsOf::PointInTime(_)))
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(covered, inventory, "every point_in_time route has its canary read");

        let t0_minute = t0.timestamp() - t0.timestamp().rem_euclid(60);
        for (id, path, body) in &reads {
            let (st, r) = api.call(if body.is_some() { "POST" } else { "GET" }, path, body.clone()).await;
            let text = r.to_string();
            match *id {
                "datasets_files_get" => assert_eq!(st, 404, "{id}: the canary file is not in the view at T0: {text}"),
                _ => assert_eq!(st, 200, "{id}: {text}"),
            }
            assert!(!text.contains("CANARY"), "{id} leaks a canary: {text}");
            assert!(!text.contains("31415.9"), "{id} leaks the canary value: {text}");
            assert!(!text.contains("\"confirmed\""), "{id} leaks the later status: {text}");
            assert!(!text.contains("0.31415"), "{id} leaks the later price: {text}");
            if *id == "markets_bars" {
                let last = r["results"].as_array().unwrap().last().unwrap()["ts"].as_str().unwrap().to_string();
                let last = chrono::DateTime::parse_from_rfc3339(&last).unwrap().timestamp();
                assert!(last < t0_minute, "{id}: the bar of T0's minute shows");
            }
        }
        let (_, obj) = api.call("GET", &format!("/api/v1/ontology/objects/{event_id}?as_of={}", enc(&at)), None).await;
        assert_eq!(obj["properties"]["status"], "unconfirmed");
        assert_eq!(obj["properties_as_of"], "exact");
        let (_, q) = api
            .call("POST", "/api/v1/ontology/query", Some(json!({ "type_key": "geo_event", "as_of": at })))
            .await;
        assert_eq!(q["count"], 1, "{q}");
        assert_eq!(q["rows"][0]["object"]["properties"]["status"], "unconfirmed");
        let (_, f) = api.call("GET", &format!("/api/v1/markets/prediction-features?instrument=XAUUSD&as_of={}", enc(&at)), None).await;
        assert_eq!(f["count"], 1, "{f}");
        assert!((f["results"][0]["features"]["probability"].as_f64().unwrap() - 0.4).abs() < 1e-9, "{f}");
        // The retro-closed links were still believed open at T0.
        let (_, l) = api.call("GET", &format!("/api/v1/ontology/objects/{gold_id}/links?as_of={}", enc(&at)), None).await;
        assert_eq!(l["count"], 2, "the exposure and the market's link: {l}");
        let only = l["results"].as_array().unwrap().iter().find(|x| x["link_type"] == "exposed_to").unwrap();
        assert_eq!(only["source"]["id"], iran.as_str(), "{l}");
        assert!(only["valid_to"].is_null(), "{l}");
        assert_eq!(only["target"]["title"], "Gold", "{l}");
        let (_, h) = api.call("GET", &format!("/api/v1/ontology/objects/{holding}/hierarchy?key=group&at={}", enc(&at)), None).await;
        assert!(h["nodes"].as_array().unwrap().iter().any(|n| n["id"] == sub.as_str()), "{h}");
        // Today's knowledge of T0 (valid_at alone): the closing and the late link count.
        let (_, l) = api.call("GET", &format!("/api/v1/ontology/objects/{gold_id}/links?valid_at={}", enc(&at)), None).await;
        let exposures: Vec<&Value> = l["results"].as_array().unwrap().iter().filter(|x| x["link_type"] == "exposed_to").collect();
        assert_eq!(exposures.len(), 1, "{l}");
        assert!(l.to_string().contains("CANARY land"), "{l}");

        // Two minutes later, every canary is there.
        let later = (t0 + chrono::Duration::minutes(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let (_, q) = api
            .call("POST", "/api/v1/ontology/query", Some(json!({ "type_key": "geo_event", "as_of": later })))
            .await;
        assert_eq!(q["count"], 2, "{q}");
        assert!(q.to_string().contains("\"confirmed\""), "{q}");
        let (_, o) = api.call("GET", &format!("/api/v1/ontology/objects/{series_id}/observations?as_of={}", enc(&later)), None).await;
        assert!(o.to_string().contains("31415.9"), "{o}");

        // D902: an assistant frozen at T0 reads as of T0 whatever the model
        // asks, and has no tool that reads the present or writes.
        {
            use pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole;
            use pnex_backend::services::ai::tools::{execute, ToolDeps};
            let deps = ToolDeps {
                db: &ctx.db,
                org_id: org,
                grants: pnex_backend::services::iam::Grants::managed(&OrgMemberRole::Owner),
                author: None,
                o2: None,
                config: None,
                app: None,
                actor: Some(pnex_backend::services::ontology::Actor::manual(1, OrgMemberRole::Owner)),
                frozen_at: Some(t0.fixed_offset()),
            };
            let q = execute(&deps, "query_ontology", &json!({ "query": { "type_key": "geo_event", "as_of": later } }))
                .await
                .unwrap()
                .value;
            let text = q.to_string();
            assert!(!text.contains("CANARY") && !text.contains("\"confirmed\""), "{text}");
            assert!(q["as_of_note"].as_str().is_some_and(|n| n.contains("frozen")), "{q}");
            let o = execute(&deps, "get_object", &json!({ "object_id": event_id })).await.unwrap().value;
            assert_eq!(o["object"]["properties"]["status"], "unconfirmed", "{o}");
            for (tool, args) in [
                ("create_object", json!({ "type_key": "nation", "title": "X", "properties": { "code": "XX" } })),
                ("list_devices", json!({})),
            ] {
                let e = execute(&deps, tool, &args).await.unwrap_err();
                assert_eq!(e.code, Some("ai-frozen-tool"), "{tool}: {}", e.message);
            }
        }
    })
    .await;
}

/// Minute rows whose close is `price(i)` (in units of 10⁻⁸).
fn minute_rows_at(
    instrument: &str,
    start_us: i64,
    n: i64,
    price: impl Fn(i64) -> i128,
) -> arrow_array::RecordBatch {
    use pnex_core::markets::history::Kline;
    let rows: Vec<Kline> = (0..n)
        .map(|i| {
            let c = price(i);
            Kline {
                open_us: start_us + i * 60_000_000,
                open: c,
                high: c,
                low: c,
                close: c,
                volume: 100_000_000,
                quote_volume: None,
                trades: None,
            }
        })
        .collect();
    let names = vec![instrument; rows.len()];
    pnex_backend::services::markets::history::ohlcv_batch("1m", "histdata", &names, &rows).unwrap()
}

/// H.7: a world event in a country a member declared exposing gold explains
/// the next 3.5 % hourly move, with its lift; nothing before the event is
/// known, nothing through a link opened later, nothing without the link.
#[tokio::test]
#[serial]
async fn price_moves_get_candidate_causal_chains() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::datasets;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        for pack in ["markets", "world_events"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{r}");
        }
        // Two days of gold minutes from 2026-01-05; +3.5 % at 2026-01-06 10:30.
        let start = chrono::DateTime::parse_from_rfc3339("2026-01-05T00:00:00Z").unwrap();
        let jump = 24 * 60 + 10 * 60 + 30;
        let u: i128 = 100_000_000;
        let store = datasets::store_of(&ctx.config).unwrap();
        let hist = datasets::create(&ctx.db, org, "histdata_ohlcv_1m").await.unwrap();
        let hist_id: uuid::Uuid = hist.id.parse().unwrap();
        let rows = minute_rows_at("XAUUSD", start.timestamp_micros(), 2 * 1440, |i| if i < jump { 2000 * u } else { 2070 * u });
        datasets::append_batch(&ctx.db, &store, org, hist_id, &rows).await.unwrap();

        let create = |body: Value| {
            let api = &api;
            async move {
                let (st, r) = api.call("POST", "/api/v1/ontology/objects", Some(body)).await;
                assert_eq!(st, 201, "{r}");
                r["id"].as_str().unwrap().to_string()
            }
        };
        let gold = create(json!({ "type_key": "instrument", "title": "Gold", "properties": { "ticker": "XAUUSD" } })).await;
        let iran = create(json!({ "type_key": "nation", "title": "Iran", "properties": { "code": "IR" } })).await;
        let event = create(json!({
            "type_key": "geo_event", "title": "Strike",
            "properties": { "ref": "ch-1", "kind": "attack", "country": "IR", "status": "confirmed",
                            "known_at": "2026-01-06T08:00:00Z" }
        }))
        .await;
        let chains = |as_of: &str| {
            let api = &api;
            let path = format!("/api/v1/markets/chains?instrument=XAUUSD&source=histdata&as_of={as_of}&from=2026-01-05T00:00:00Z");
            async move {
                let (st, r) = api.call("GET", &path, None).await;
                assert_eq!(st, 200, "{r}");
                r
            }
        };
        // Without the exposure link: no path, no chain.
        assert_eq!(chains("2026-01-07T00:00:00Z").await["count"], 0);

        // A member declares that Iran exposes gold since 2025.
        let (st, r) = api
            .call("POST", "/api/v1/ontology/links", Some(json!({
                "link_type": "exposed_to", "source_id": iran, "target_id": gold,
                "valid_from": "2025-01-01T00:00:00Z"
            })))
            .await;
        assert_eq!(st, 201, "{r}");
        let link = r["id"].as_i64().unwrap();
        // Declared today "since 2025": nobody knew it in January (D913).
        assert_eq!(chains("2026-01-07T00:00:00Z").await["count"], 0, "a link declared today is unknown in January");
        // As if it had been recorded on 2025-12-01.
        ctx.db
            .execute_unprepared(&format!("UPDATE link_versions SET at = '2025-12-01T00:00:00Z' WHERE edge_id = {link}"))
            .await
            .unwrap();
        let r = chains("2026-01-07T00:00:00Z").await;
        assert_eq!(r["count"], 1, "{r}");
        let c = &r["results"][0];
        let hop = &c["hops"][0];
        assert_eq!(hop["cause"]["id"], format!("world.event:{event}"));
        assert_eq!(hop["cause"]["kind"], "world.event");
        assert_eq!(hop["cause"]["on"], iran);
        assert_eq!(hop["effect"]["kind"], "price.move");
        assert_eq!(hop["effect"]["on"], gold);
        assert_eq!(hop["distance"], 1);
        // The move is known at the close of its hour (11:00): 3 h after the event.
        assert_eq!(hop["gap"], 3 * 3_600);
        assert!(c["confidence"].as_f64().unwrap() > 0.0);
        assert!(c["lift"].as_f64().unwrap() >= 2.0, "{c}");

        // Before the move closed: nothing to explain.
        assert_eq!(chains("2026-01-06T10:59:00Z").await["count"], 0);
        // Closed today back to 2025-06: in January it was still believed open.
        let (st, r) = api
            .call("POST", &format!("/api/v1/ontology/links/{link}/close"), Some(json!({ "valid_to": "2025-06-01T00:00:00Z" })))
            .await;
        assert_eq!(st, 200, "{r}");
        assert_eq!(chains("2026-01-07T00:00:00Z").await["count"], 1, "a closing recorded later is not seen");
        // A threshold above the move: no effect.
        let (_, obj) = api.call("GET", &format!("/api/v1/ontology/objects/{gold}"), None).await;
        let (st, r) = api
            .call("PUT", &format!("/api/v1/ontology/objects/{gold}"), Some(json!({
                "type_key": "instrument", "title": "Gold",
                "properties": { "ticker": "XAUUSD", "move_threshold_pct": 5 },
                "expected_version": obj["version"]
            })))
            .await;
        assert_eq!(st, 200, "{r}");
        assert_eq!(chains("2026-01-07T00:00:00Z").await["count"], 0, "a setting: the current threshold applies");
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let (_, r) = api
            .call("GET", &format!("/api/v1/markets/chains?instrument=XAUUSD&source=histdata&as_of={now}"), None)
            .await;
        assert_eq!(r["count"], 0, "no bars in the last 7 days");

        // R2: datasets:read alone is not enough, the chains read the ontology.
        let (_, sa) = api.call("POST", "/api/v1/iam/service-accounts", Some(json!({ "name": "bars-only" }))).await;
        let (_, role) = api
            .call("POST", "/api/v1/iam/roles", Some(json!({
                "key": "bars-only", "name": "Bars only",
                "statements": [{ "effect": "allow", "permissions": ["datasets:read"] }]
            })))
            .await;
        api.call("POST", "/api/v1/iam/assignments", Some(json!({ "service_account_id": sa["id"], "role_id": role["id"] }))).await;
        let exp = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
        let (_, key) = api.call("POST", &format!("/api/v1/iam/service-accounts/{}/keys", sa["id"].as_str().unwrap()), Some(json!({ "expires_at": exp }))).await;
        let res = server
            .get("/api/v1/markets/chains?instrument=XAUUSD&source=histdata&as_of=2026-01-07T00:00:00Z&from=2026-01-05T00:00:00Z")
            .add_header("Authorization", format!("Bearer {}", key["secret"].as_str().unwrap()))
            .await;
        assert_eq!(res.status_code().as_u16(), 403, "{}", res.text());
        assert!(res.text().contains("ontology:read"), "{}", res.text());

        // Unknown ticker, window too wide.
        let (st, e) = api.call("GET", "/api/v1/markets/chains?instrument=NOPE&source=histdata&as_of=2026-01-07T00:00:00Z", None).await;
        assert_eq!((st, e["errors"]["fields"]["instrument"].as_str()), (400, Some("unknown")), "{e}");
        let (st, e) = api
            .call("GET", "/api/v1/markets/chains?instrument=XAUUSD&source=histdata&as_of=2026-03-01T00:00:00Z&from=2026-01-01T00:00:00Z", None)
            .await;
        assert_eq!((st, e["errors"]["fields"]["from"].as_str()), (400, Some("range:31d")), "{e}");
    })
    .await;
}

/// Appendix J (D918–D925): a followed market read from the platform's
/// public APIs (mocked), its resolution kept as dated states (proposed,
/// disputed, final), its probability an input of the model only while
/// unresolved and only as known at the instant, a model's forecast scored
/// against the market, and an early bet proposed in a case, then retained
/// as a wallet object linked to the market.
#[tokio::test]
#[serial]
async fn prediction_markets_collect_resolve_and_feed_the_model() {
    with_app(|server, ctx, token| async move {
        use axum::extract::Query;
        use pnex_backend::services::markets::prediction::{self, Hosts};
        use std::collections::HashMap;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        for pack in ["markets", "prediction_markets"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{r}");
        }
        const C: &str = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        const W: &str = "0x2222222222222222222222222222222222222222";
        let now = chrono::Utc::now().timestamp();
        let hour = now - now.rem_euclid(3_600);
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let app = axum::Router::new()
            .route(
                "/markets",
                axum::routing::get(move || {
                    let n = c.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let (status, closed, prices) = match n {
                            0 => ("proposed", false, ["0.6", "0.4"]),
                            1 => ("disputed", false, ["0.6", "0.4"]),
                            _ => ("resolved", true, ["0", "1"]),
                        };
                        axum::Json(json!([{
                            "conditionId": C, "slug": "fed-cut", "question": "Fed cut in December?",
                            "description": "Resolves Yes if the Fed cuts.", "resolutionSource": "federalreserve.gov",
                            "endDate": "2027-01-01T00:00:00Z", "outcomes": "[\"Yes\",\"No\"]",
                            "outcomePrices": serde_json::to_string(&prices).unwrap(),
                            "clobTokenIds": "[\"111\",\"222\"]", "volumeNum": 50000, "closed": closed,
                            "umaResolutionStatus": status
                        }]))
                    }
                }),
            )
            .route(
                "/prices-history",
                axum::routing::get(move |Query(q): Query<HashMap<String, String>>| async move {
                    let n = |k: &str| q.get(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                    let (start, end) = (n("startTs"), n("endTs"));
                    // 0.30 for three days, 0.60 from ten hours ago.
                    let history: Vec<Value> = (0..=72)
                        .map(|i| hour - 72 * 3_600 + i * 3_600)
                        .filter(|t| *t >= start && *t <= end)
                        .map(|t| json!({ "t": t, "p": if t < hour - 10 * 3_600 { 0.3 } else { 0.6 } }))
                        .collect();
                    axum::Json(json!({ "history": history }))
                }),
            )
            .route(
                "/trades",
                axum::routing::get(move |Query(q): Query<HashMap<String, String>>| async move {
                    if q.get("offset").map(String::as_str) != Some("0") {
                        return axum::Json(json!([]));
                    }
                    axum::Json(json!([
                        { "proxyWallet": W, "side": "BUY", "asset": "111", "conditionId": C, "outcome": "Yes",
                          "size": 24000, "price": 0.5, "timestamp": hour - 20 * 3_600, "transactionHash": "0x01" },
                        { "proxyWallet": "0x3333333333333333333333333333333333333333", "side": "BUY", "asset": "111",
                          "conditionId": C, "outcome": "Yes", "size": 10, "price": 0.3, "timestamp": hour - 30 * 3_600,
                          "transactionHash": "0x02" }
                    ]))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let hosts = Hosts { gamma: base.clone(), clob: base.clone(), data: base };
        let (st, r) = api
            .call("PATCH", "/api/v1/ontology/packs/prediction_markets/settings",
                  Some(json!({ "values": { "market_ids": format!("{C}, not a market"), "history_days": 3 } })))
            .await;
        assert_eq!(st, 200, "{r}");
        let client = reqwest::Client::new();
        let run = |at: i64| {
            let (db, client, hosts) = (ctx.db.clone(), client.clone(), hosts.clone());
            async move {
                // Each pass reads the market again (the hourly refresh is reset).
                db.execute_unprepared("UPDATE prediction_cursors SET last_ts = 0 WHERE key = 'm:refresh'").await.unwrap();
                prediction::pass(&db, &client, &hosts, org, at).await.unwrap();
            }
        };
        run(now).await;
        let market = object_id(&ctx.db, org, "prediction_market", "condition_id", C).await.expect("market recorded");
        let count = |sql: String| {
            let db = ctx.db.clone();
            async move {
                db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<i64>("", "n")
                    .unwrap()
            }
        };
        assert!(count(format!("SELECT count(*) AS n FROM prediction_prices WHERE org_id = {org} AND token = '111'")).await >= 70);
        assert_eq!(count(format!("SELECT count(*) AS n FROM prediction_trades WHERE org_id = {org}")).await, 2);

        // The model's input: linked by a member, unresolved, as known now.
        let (st, gold) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "instrument", "title": "Gold", "properties": { "ticker": "XAUUSD" } })))
            .await;
        assert_eq!(st, 201, "{gold}");
        let (st, r) = api
            .call("POST", "/api/v1/ontology/links", Some(json!({
                "link_type": "bears_on", "source_id": market, "target_id": gold["id"],
                "attributes": { "direction": "down" }
            })))
            .await;
        assert_eq!(st, 201, "{r}");
        let features = |as_of: String| {
            let api = &api;
            async move {
                let (st, r) = api.call("GET", &format!("/api/v1/markets/prediction-features?instrument=XAUUSD&as_of={as_of}"), None).await;
                assert_eq!(st, 200, "{r}");
                r
            }
        };
        let iso = |t: i64| chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let later = iso(now + 60);
        let f = features(later.clone()).await;
        assert_eq!(f["count"], 1, "{f}");
        let x = &f["results"][0];
        assert_eq!((x["direction"].as_str(), x["outcome"].as_str()), (Some("down"), Some("Yes")), "{x}");
        assert!((x["features"]["probability"].as_f64().unwrap() - 0.6).abs() < 1e-9, "{x}");
        assert!((x["features"]["change_24h"].as_f64().unwrap() - 0.3).abs() < 1e-9, "{x}");
        assert_eq!(features(iso(now - 2 * 86_400)).await["count"], 0, "neither the market nor the link were known then");
        let (st, e) = api.call("GET", "/api/v1/markets/prediction-features?instrument=NOPE", None).await;
        assert_eq!((st, e["errors"]["fields"]["instrument"].as_str()), (400, Some("unknown")), "{e}");

        // Disputed, then final « No »: dated states, never resolved ahead of time.
        run(now + 1).await;
        assert_eq!(features(iso(now + 2)).await["count"], 1, "a dispute is no resolution");
        run(now + 3).await;
        let states = count(format!(
            "SELECT count(*) AS n FROM objects WHERE org_id = {org} AND type_key = 'prediction_resolution' AND properties ->> 'market' = '{market}'"
        ))
        .await;
        assert_eq!(states, 3);
        assert_eq!(features(iso(now + 4)).await["count"], 0, "resolved: no longer an input");
        assert_eq!(features(iso(now + 2)).await["count"], 1, "as known before the final state");

        // A model's forecast against the market at the decision's instant.
        ctx.db
            .execute_unprepared(&format!(
                "INSERT INTO market_decisions (id, org_id, model, model_version, instrument, source, interval, horizon_bars, \
                   as_of, probabilities, best, confidence, forecasts) VALUES ('{}', {org}, 'm', '1', 'XAUUSD', 'histdata', '1h', 4, \
                   to_timestamp({}), ARRAY[0.2, 0.5, 0.3], 'neutral', 0.5, '[{{\"market\": \"{C}\", \"outcome\": \"No\", \"probability\": 0.8}}]')",
                uuid::Uuid::new_v4(),
                hour - 3_600
            ))
            .await
            .unwrap();
        let (st, s) = api.call("GET", "/api/v1/markets/decisions/forecast-scores", None).await;
        assert_eq!(st, 200, "{s}");
        let g = &s["results"][0];
        assert_eq!((g["forecasts"].as_u64(), g["scored"].as_u64()), (Some(1), Some(1)), "{s}");
        assert!((g["brier_model"].as_f64().unwrap() - 0.04).abs() < 1e-9, "{s}");
        assert!((g["brier_market"].as_f64().unwrap() - 0.16).abs() < 1e-9, "{s}");

        // Early bet: a fresh wallet put 12 000 USDC on the market ten hours
        // before the jump. Proposed in a case, never an accusation.
        let (st, case) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "case", "title": "Fed leak", "properties": { "status": "open" } })))
            .await;
        assert_eq!(st, 201, "{case}");
        let cid = case["id"].as_str().unwrap();
        let (st, r) = api.call("POST", &format!("/api/v1/cases/{cid}/attach"), Some(json!({ "object_id": market }))).await;
        assert_eq!(st, 200, "{r}");
        let window = json!({ "from": iso(hour - 2 * 86_400), "to": iso(now + 10) });
        let (st, d) = api.call("POST", &format!("/api/v1/cases/{cid}/detect"), Some(window)).await;
        assert_eq!(st, 200, "{d}");
        assert_eq!(d["inserted"], 1, "{d}");
        let (_, list) = api.call("GET", &format!("/api/v1/cases/{cid}/proposals"), None).await;
        let p = list["results"].as_array().unwrap().iter().find(|p| p["kind"] == "early_bet").cloned().expect("early bet proposed");
        let (st, w) = api
            .call("POST", &format!("/api/v1/cases/{cid}/proposals/{}/retain", p["id"].as_str().unwrap()), Some(json!({ "title": "Early bet" })))
            .await;
        assert_eq!(st, 200, "{w}");
        let wallet = object_id(&ctx.db, org, "wallet", "address", W).await.expect("wallet object on retain");
        let (_, l) = api.call("GET", &format!("/api/v1/ontology/objects/{wallet}/links"), None).await;
        assert!(l["results"].as_array().unwrap().iter().any(|x| x["link_type"] == "traded"), "{l}");
        assert!(object_id(&ctx.db, org, "wallet", "address", "0x3333333333333333333333333333333333333333").await.is_none(), "no object per wallet");

        // Blocked hosts (France): the org's proxy, from its vault, carries
        // every call; the hosts themselves are never resolved here.
        let proxy = hosts.gamma.clone();
        pnex_backend::services::markets::allow_test_host(&proxy);
        let (st, r) = api.call("POST", "/api/v1/secrets", Some(json!({ "name": "polymarket_proxy", "value": proxy }))).await;
        assert_eq!(st, 201, "{r}");
        ctx.db.execute_unprepared("UPDATE prediction_cursors SET last_ts = 0 WHERE key = 'm:refresh'").await.unwrap();
        let before = calls.load(Ordering::SeqCst);
        let blocked = Hosts {
            gamma: "http://gamma.blocked.invalid".into(),
            clob: "http://clob.blocked.invalid".into(),
            data: "http://data.blocked.invalid".into(),
        };
        prediction::sweep(&ctx, &blocked, chrono::Utc::now()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), before + 1, "the market was read through the proxy");
    })
    .await;
}

/// Public trading accounts (markets-research.md appendix K): trades stored
/// once (D927), read as known at a date (D928), an entry-timing proposal
/// around releases (D930) retained into the account object, never a link to
/// a person whatever the link type (D933), purged with the account (D932).
#[tokio::test]
#[serial]
async fn trading_accounts_import_detect_and_never_name_a_person() {
    with_app(|server, ctx, token| async move {
        use pnex_backend::services::markets::accounts;
        use pnex_backend::services::ontology::Actor;
        use pnex_core::markets::accounts::Trade;
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: token.clone(), org };
        for pack in ["markets", "trading_accounts", "investigation"] {
            let (st, r) = api.call("POST", &format!("/api/v1/ontology/packs/{pack}/install"), None).await;
            assert_eq!(st, 200, "{r}");
        }
        let iso = |t: i64| chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339();
        let (st, gold) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "instrument", "title": "Gold", "properties": { "ticker": "XAUUSD" } })))
            .await;
        assert_eq!(st, 201, "{gold}");
        let gold = gold["id"].as_str().unwrap().to_string();

        // A release, and twelve sells opened within 20 minutes of it.
        let now = chrono::Utc::now().timestamp();
        let release = now - 2 * 86_400;
        let (st, r) = api
            .call("POST", "/api/v1/time-ranges", Some(json!({
                "scope_kind": "org", "label": "Consumer Price Index", "category": "economic_release",
                "planned_start": iso(release), "planned_end": iso(release + 60)
            })))
            .await;
        assert_eq!(st, 201, "{r}");
        let trade = |i: i64| Trade {
            trade_key: format!("ticket:{i}"),
            symbol: "XAUUSD#".into(),
            buy: false,
            lots: Some(0.01),
            open_at: release - 600 + i * 60,
            close_at: Some(release + 3_600),
            open_price: 2_000.0,
            close_price: Some(1_990.0),
            sl: None,
            tp: None,
            profit: Some(10.0),
            pips: Some(100.0),
            commission: None,
            swap: None,
            gain_pct: None,
            magic: None,
            comment: None,
        };
        let trades: Vec<Trade> = (0..12).map(trade).collect();
        let actor = Actor::manual(1, pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole::Owner);
        let t_import = chrono::Utc::now();
        let r = accounts::import(&ctx.db, org, &actor, "myfxbook:12305445", Some("Grid gold"), Some("USC"), &trades, "export.csv", t_import)
            .await
            .unwrap();
        assert_eq!((r.inserted, r.kept), (12, 0));
        // The same export again: nothing doubled, the first knowledge kept.
        let r2 = accounts::import(&ctx.db, org, &actor, "myfxbook:12305445", None, None, &trades, "export.csv", t_import + chrono::Duration::days(1))
            .await
            .unwrap();
        assert_eq!((r2.inserted, r2.kept), (0, 12));
        assert_eq!(r2.account_id, r.account_id);
        let account = r.account_id.clone();
        let (_, acc_obj) = api.call("GET", &format!("/api/v1/ontology/objects/{account}"), None).await;
        assert_eq!(acc_obj["properties"]["currency"], "USC", "profits of a cent account: {acc_obj}");

        // Point in time: absent before the import, there after, backfilled.
        let enc = |s: String| s.replace('+', "%2B");
        let (st, before) = api
            .call("GET", &format!("/api/v1/markets/account-trades?as_of={}", enc(iso(t_import.timestamp() - 60))), None)
            .await;
        assert_eq!(st, 200, "{before}");
        assert_eq!(before["count"], 0, "{before}");
        let (_, after) = api.call("GET", "/api/v1/markets/account-trades?account=myfxbook:12305445&limit=5", None).await;
        assert_eq!(after["count"], 12, "{after}");
        assert_eq!(after["results"].as_array().unwrap().len(), 5);
        assert_eq!(after["results"][0]["backfilled"], true);
        let (st, bad) = api.call("GET", "/api/v1/markets/account-trades?account=elsewhere:1", None).await;
        assert_eq!(st, 400, "{bad}");

        // The route a client sends its CSV to: refused at the faulty cell,
        // stored once accepted.
        let post_csv = |csv: &'static str| {
            let req = server
                .post("/api/v1/markets/account-trades/import?account=myfxbook:42&name=Second&source=s.csv")
                .add_header("Authorization", format!("Bearer {token}"))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "text/csv")
                .bytes(csv.as_bytes().to_vec().into());
            async move {
                let res = req.await;
                (res.status_code().as_u16(), res.json::<Value>())
            }
        };
        let (st, e) = post_csv("symbol,side,lots,open_at,open_price\nXAUUSD,long,1,2024-01-01T00:00:00Z,2000\n").await;
        assert_eq!(st, 400, "{e}");
        assert_eq!(e["error"], "account-trades-csv-invalid", "{e}");
        assert_eq!((e["errors"]["args"]["line"].as_u64(), e["errors"]["args"]["column"].as_str()), (Some(1), Some("side")), "{e}");
        let (st, ok) = post_csv("ticket,symbol,side,lots,open_at,open_price\n9,XAUUSD,buy,1,2024-01-01T00:00:00Z,2000\n").await;
        assert_eq!(st, 200, "{ok}");
        assert_eq!(ok["inserted"], 1, "{ok}");
        // An owner who hides position sizes: no lots column at all.
        let (st, hidden) = post_csv("symbol,side,open_at,open_price,profit\nXAUUSD,sell,2024-01-02T00:00:00Z,2010,5\n").await;
        assert_eq!(st, 200, "{hidden}");
        assert_eq!(hidden["inserted"], 1, "{hidden}");

        // The import linked the account to the instrument it traded.
        let (_, l) = api.call("GET", &format!("/api/v1/ontology/objects/{account}/links"), None).await;
        let link = l["results"].as_array().unwrap().iter().find(|x| x["link_type"] == "trades").cloned().expect("trades link");
        assert_eq!(link["attributes"]["trade_count"], 12, "{link}");

        // Entry timing in a case holding the instrument: a proposal, figures only.
        let (st, case) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "case", "title": "Gold entries", "properties": { "status": "open" } })))
            .await;
        assert_eq!(st, 201, "{case}");
        let cid = case["id"].as_str().unwrap();
        let (st, r) = api.call("POST", &format!("/api/v1/cases/{cid}/attach"), Some(json!({ "object_id": gold }))).await;
        assert_eq!(st, 200, "{r}");
        let window = json!({ "from": iso(release - 86_400), "to": iso(now) });
        let (st, d) = api.call("POST", &format!("/api/v1/cases/{cid}/detect"), Some(window)).await;
        assert_eq!(st, 200, "{d}");
        let (_, list) = api.call("GET", &format!("/api/v1/cases/{cid}/proposals"), None).await;
        let p = list["results"].as_array().unwrap().iter().find(|p| p["kind"] == "entry_timing").cloned().expect("entry timing proposed");
        assert_eq!(p["payload"]["figures"]["around_release"], 12, "{p}");
        let (st, w) = api
            .call("POST", &format!("/api/v1/cases/{cid}/proposals/{}/retain", p["id"].as_str().unwrap()), Some(json!({ "title": "Entries around CPI" })))
            .await;
        assert_eq!(st, 200, "{w}");
        let text = w.to_string();
        assert!(text.contains("no intention is attributed"), "{text}");

        // Never tied to a person: not by a link type a member writes, not
        // by a resource edge.
        let (st, r) = api
            .call("POST", "/api/v1/ontology/link-types", Some(json!({
                "key": "operated_by", "name": "operated by", "from_types": ["trading_account"], "to_types": ["person"]
            })))
            .await;
        assert_eq!(st, 201, "{r}");
        let (st, person) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "person", "title": "Someone" })))
            .await;
        assert_eq!(st, 201, "{person}");
        let person = person["id"].as_str().unwrap();
        let (st, r) = api
            .call("POST", "/api/v1/ontology/links", Some(json!({ "link_type": "operated_by", "source_id": account, "target_id": person })))
            .await;
        assert_eq!(st, 400, "a trading account is never linked to a person: {r}");
        let (st, r) = api
            .call("POST", "/api/v1/resources/edges", Some(json!({
                "relation": "operated_by", "source_kind": "trading_account", "source_id": account,
                "target_kind": "person", "target_id": person
            })))
            .await;
        assert!(st == 400 || st == 422, "nor by a resource edge ({st}): {r}");

        // Erasing the account purges its trades (D932).
        let (st, r) = api.call("POST", &format!("/api/v1/ontology/objects/{account}/erase"), Some(json!({}))).await;
        assert_eq!(st, 200, "{r}");
        let (_, gone) = api.call("GET", "/api/v1/markets/account-trades?account=myfxbook:12305445", None).await;
        assert_eq!(gone["count"], 0, "{gone}");
    })
    .await;
}

/// HistData ticks: a closed month per file, its CSV streamed into the
/// `quote_tick` table as one transaction, EST (no daylight saving) turned
/// to UTC; a month without a file stays empty, a loaded one is not asked
/// again.
#[tokio::test]
#[serial]
async fn histdata_ticks_are_streamed_month_by_month() {
    with_app(|server, ctx, token| async move {
        use axum::{extract::RawQuery, response::IntoResponse, routing::{get, post}, Router};
        use pnex_backend::services::markets::history::{sweep_from, Hosts};
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
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
        api.settings(json!({ "histdata_tick_pairs": "xauusd", "histdata_tick_from": "2024-05" })).await;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.start_file("DAT_ASCII_XAUUSD_T_202406.csv", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(
            b"20240602 180001677,2327.425000,2328.175000,0\n\
20240602 180002329,2327.325000,2328.075000,0\n\
20240630 235959159,2324.608000,2324.995000,0\n",
        )
        .unwrap();
        w.start_file("DAT_ASCII_XAUUSD_T_202406.txt", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"gaps report").unwrap();
        let zip = w.finish().unwrap().into_inner();
        let files = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/download-free-forex-historical-data/",
                get(|RawQuery(q): RawQuery| async move {
                    // June 2024 has a file; May (before the mock's first) has none.
                    match q.as_deref() {
                        Some("/ascii/tick-data-quotes/xauusd/2024/6") => r#"<html>
<form id="file_down" name="file_down" target="nullDisplay" method="POST" action="/get.php">
<input type="hidden" name="tk" id="tk" value="0123456789abcdef0123456789abcdef" />
<input type="hidden" name="datemonth" id="datemonth" value="202406" />
<input type="hidden" name="platform" id="platform" value="ASCII" />
<input type="hidden" name="timeframe" id="timeframe" value="T" />
<input type="hidden" name="fxpair" id="fxpair" value="XAUUSD" /></form></html>"#
                            .into_response(),
                        _ => "<html>no file here</html>".into_response(),
                    }
                }),
            )
            .route(
                "/get.php",
                post({
                    let files = files.clone();
                    move || async move {
                        files.fetch_add(1, Ordering::SeqCst);
                        zip.clone().into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::markets::allow_test_host(&base);
        let hosts = Hosts { histdata: base.clone(), ..Default::default() };

        // One month per pass: May (no file), then June; July is the current month.
        let today = chrono::NaiveDate::from_ymd_opt(2024, 7, 10).unwrap();
        for _ in 0..3 {
            sweep_from(&ctx, &hosts, today).await.unwrap();
        }
        let states = || async {
            ctx.db
                .query_all_raw(Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    "SELECT period || ':' || period_start::text || ':' || state || ':' || rows::text AS s \
                     FROM market_history_partitions WHERE source = 'histdata_tick' ORDER BY period_start"
                        .to_string(),
                ))
                .await
                .unwrap()
                .iter()
                .map(|r| r.try_get::<String>("", "s").unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(states().await, vec!["month:2024-05-01:empty:0", "month:2024-06-01:done:3"]);
        assert_eq!(files.load(Ordering::SeqCst), 1, "the month already loaded is not asked again");

        let (_, list) = api.call("GET", "/api/v1/datasets?limit=50", None).await;
        let id = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "histdata_ticks")
            .unwrap_or_else(|| panic!("{list}"))["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, sample) = api.call("GET", &format!("/api/v1/datasets/{id}/sample?limit=10"), None).await;
        assert_eq!(st, 200, "{sample}");
        let text = sample.to_string();
        // 18:00:01.677 EST is 23:00:01.677 UTC; the last tick of June is in July in UTC.
        assert!(text.contains("2024-06-02T23:00:01.677"), "{text}");
        assert!(text.contains("2024-07-01T04:59:59.159"), "{text}");
        assert!(text.contains("histdata_tick"), "{text}");
        let (st, files_list) = api.call("GET", &format!("/api/v1/datasets/{id}/files"), None).await;
        assert_eq!(st, 200, "{files_list}");
        assert_eq!(files_list["count"], 1, "one month, one transaction: {files_list}");
    })
    .await;
}
