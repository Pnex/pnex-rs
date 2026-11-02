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
            json!({"date": format!("{}T00:00:00.000Z", price_day(i)), "close": close, "volume": 1000,
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
            "ecb_series": "FM.D.U2.EUR", "covered_leis": "LEITEST"
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
