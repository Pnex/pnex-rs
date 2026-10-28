//! OpenObserve tests (Phase 5) against a mock faithful to the actual
//! behaviour seen on v0.92.1: no org name deduplication, admin role only
//! native, passcode per user, `_json` ingestion with Basic
//! email:passcode — see services/openobserve/mod.rs.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::Path;
use axum::routing::{get, post, put};
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::openobserve::{self, ensure_org_credentials, Client};
use pnex_backend::services::settings::IngestSettings;
use pnex_backend::services::telemetry::{self, TelemetryPoint};
use prost::Message as _;
use serial_test::serial;

// ───────────────────────── Mock OpenObserve ─────────────────────────

#[derive(Default)]
struct MockState {
    /// (name, identifier) — duplicate names create a 2nd entry,
    /// as in reality.
    orgs: Vec<(String, String)>,
    /// (org identifier, email) → current password.
    users: HashMap<(String, String), String>,
    /// Received Prometheus write: org identifier, basic "email:passcode",
    /// metric, labels, value, ts_ms.
    #[allow(clippy::type_complexity)]
    ingested: Vec<(String, String, String, Vec<(String, String)>, f64, i64)>,
    org_creates: u32,
    user_creates: u32,
    resets: u32,
    /// Basics received on the Prometheus query route (order of attempts).
    query_basics: Vec<String>,
}

fn basic_of(header: Option<&str>) -> String {
    // "Basic b64(email:…)" → "email:…"
    let raw = header
        .unwrap_or_default()
        .strip_prefix("Basic ")
        .unwrap_or("");
    String::from_utf8(STANDARD.decode(raw).unwrap_or_default()).unwrap_or_default()
}

fn passcode_of(email: &str) -> String {
    format!("o2oi_{email}")
}

async fn spawn_mock_o2() -> (String, Arc<Mutex<MockState>>) {
    let state = Arc::new(Mutex::new(MockState::default()));
    let s = state.clone();
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/api/organizations",
            get({
                let s = s.clone();
                move || {
                    let s = s.clone();
                    async move {
                        let guard = s.lock().unwrap();
                        let data: Vec<serde_json::Value> =
                            std::iter::once(serde_json::json!({
                                "identifier": "default", "name": "default"
                            }))
                            .chain(guard.orgs.iter().map(
                                |(name, id)| serde_json::json!({"identifier": id, "name": name}),
                            ))
                            .collect();
                        axum::Json(serde_json::json!({ "data": data }))
                    }
                }
            })
            .post({
                let s = s.clone();
                move |body: String| {
                    let s = s.clone();
                    async move {
                        let name = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["name"].as_str().map(str::to_string))
                            .unwrap_or_default();
                        let mut guard = s.lock().unwrap();
                        guard.org_creates += 1;
                        // As in reality: no deduplication by name.
                        let identifier = format!("mockid{}", guard.org_creates);
                        guard.orgs.push((name.clone(), identifier.clone()));
                        axum::Json(serde_json::json!({
                            "identifier": identifier, "name": name
                        }))
                    }
                }
            }),
        )
        .route(
            "/api/{org}/users",
            post({
                let s = s.clone();
                move |Path(org): Path<String>, body: String| {
                    let s = s.clone();
                    async move {
                        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                        let email = v["email"].as_str().unwrap().to_string();
                        let password = v["password"].as_str().unwrap().to_string();
                        let mut guard = s.lock().unwrap();
                        if guard.users.contains_key(&(org.clone(), email.clone())) {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                axum::Json(serde_json::json!({
                                    "code": 400, "message": "User already exists"
                                })),
                            );
                        }
                        guard.user_creates += 1;
                        guard.users.insert((org, email), password);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "code": 200, "message": "User saved successfully"
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/{org}/users/{email}",
            put({
                let s = s.clone();
                move |Path((org, email)): Path<(String, String)>, body: String| {
                    let s = s.clone();
                    async move {
                        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                        let new_password = v["new_password"].as_str().unwrap().to_string();
                        let mut guard = s.lock().unwrap();
                        guard.resets += 1;
                        guard.users.insert((org, email), new_password);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "code": 200, "message": "User updated successfully"
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/{org}/passcode",
            get({
                let s = s.clone();
                move |Path(org): Path<String>, headers: axum::http::HeaderMap| {
                    let s = s.clone();
                    async move {
                        let basic =
                            basic_of(headers.get("authorization").and_then(|v| v.to_str().ok()));
                        let (email, password) = basic.split_once(':').unwrap_or(("", ""));
                        let guard = s.lock().unwrap();
                        let auth_ok = guard
                            .users
                            .get(&(org, email.to_string()))
                            .is_some_and(|p| p == password);
                        if !auth_ok {
                            return (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "message": "Unauthorized Access"
                                })),
                            );
                        }
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "data": {"passcode": passcode_of(email), "user": email}
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/{org}/prometheus/api/v1/write",
            post({
                let s = s.clone();
                move |Path(org): Path<String>,
                      headers: axum::http::HeaderMap,
                      body: axum::body::Bytes| {
                    let s = s.clone();
                    async move {
                        let basic =
                            basic_of(headers.get("authorization").and_then(|v| v.to_str().ok()));
                        // Decodes snappy + protobuf like the real O2.
                        let raw = snap::raw::Decoder::new()
                            .decompress_vec(&body)
                            .unwrap_or_default();
                        let req = pnex_core::WriteRequest::decode(raw.as_slice())
                            .unwrap_or_else(|_| pnex_core::WriteRequest { timeseries: vec![] });
                        let mut guard = s.lock().unwrap();
                        for ts in req.timeseries {
                            let metric = ts
                                .labels
                                .iter()
                                .find(|l| l.name == "__name__")
                                .map(|l| l.value.clone())
                                .unwrap_or_default();
                            let labels: Vec<(String, String)> = ts
                                .labels
                                .iter()
                                .filter(|l| l.name != "__name__")
                                .map(|l| (l.name.clone(), l.value.clone()))
                                .collect();
                            for sample in ts.samples {
                                guard.ingested.push((
                                    org.clone(),
                                    basic.clone(),
                                    metric.clone(),
                                    labels.clone(),
                                    sample.value,
                                    sample.timestamp,
                                ));
                            }
                        }
                        axum::http::StatusCode::OK
                    }
                }
            }),
        )
        .route(
            "/api/{org}/prometheus/api/v1/query",
            get({
                let s = s.clone();
                move |Path(org): Path<String>,
                      headers: axum::http::HeaderMap,
                      axum::extract::Query(params): axum::extract::Query<
                    HashMap<String, String>,
                >| {
                    let s = s.clone();
                    async move {
                        let basic =
                            basic_of(headers.get("authorization").and_then(|v| v.to_str().ok()));
                        let mut guard = s.lock().unwrap();
                        guard.query_basics.push(basic.clone());
                        // Auth: root Basic ONLY — faithful to the real O2
                        // v0.92.1 where the email:passcode Basic is refused on
                        // the query (it only serves ingestion).
                        let (email, secret) = basic.split_once(':').unwrap_or(("", ""));
                        let root_ok = email == "root@pnex.local" && secret == "whatever";
                        if !root_ok {
                            return (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "message": "Unauthorized Access"
                                })),
                            );
                        }
                        // Faithful to v0.92.1: {__name__=~"..."} selectors
                        // (regex) return an EMPTY vector — only the bare name
                        // (possibly wrapped in last_over_time)
                        // selects.
                        let query = params.get("query").map(String::as_str).unwrap_or("");
                        let regex_selector = query.contains("__name__=~");
                        let metric_of_query = if regex_selector {
                            None
                        } else {
                            query
                                .strip_prefix("last_over_time(")
                                .and_then(|rest| rest.split('[').next())
                                .or_else(|| {
                                    query
                                        .chars()
                                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
                                        .then_some(query)
                                })
                                .map(str::to_string)
                        };
                        // Last sample per (metric, device_id) of the
                        // org — exact Prometheus vector shape.
                        #[allow(clippy::type_complexity)]
                        let mut last: HashMap<
                            (String, String),
                            (Vec<(String, String)>, f64, i64),
                        > = HashMap::new();
                        for (o2_org, _b, metric, labels, value, ts_ms) in &guard.ingested {
                            if o2_org != &org {
                                continue;
                            }
                            if regex_selector {
                                continue; // no match, faithful to v0.92.1
                            }
                            if metric_of_query.as_ref().is_some_and(|w| w != metric) {
                                continue;
                            }
                            let device = labels
                                .iter()
                                .find(|(n, _)| n == "device_id")
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default();
                            let entry = last.entry((metric.clone(), device)).or_insert((
                                labels.clone(),
                                *value,
                                *ts_ms,
                            ));
                            if ts_ms >= &entry.2 {
                                *entry = (labels.clone(), *value, *ts_ms);
                            }
                        }
                        let result: Vec<serde_json::Value> = last
                            .into_iter()
                            .map(|((metric, _device), (labels, value, ts_ms))| {
                                let mut metric_map = serde_json::Map::new();
                                metric_map
                                    .insert("__name__".to_string(), serde_json::json!(metric));
                                for (n, v) in labels {
                                    metric_map.insert(n, serde_json::json!(v));
                                }
                                serde_json::json!({
                                    "metric": metric_map,
                                    "value": [ts_ms as f64 / 1000.0, value.to_string()]
                                })
                            })
                            .collect();
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "status": "success",
                                "data": {"resultType": "vector", "result": result}
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/{org}/prometheus/api/v1/query_range",
            get({
                let s = s.clone();
                move |Path(org): Path<String>,
                      headers: axum::http::HeaderMap,
                      axum::extract::Query(params): axum::extract::Query<
                    HashMap<String, String>,
                >| {
                    let s = s.clone();
                    async move {
                        let basic =
                            basic_of(headers.get("authorization").and_then(|v| v.to_str().ok()));
                        let mut guard = s.lock().unwrap();
                        guard.query_basics.push(basic.clone());
                        // Root auth only, like the instant query
                        // (faithful to v0.92.1: passcode = ingestion only).
                        let (email, secret) = basic.split_once(':').unwrap_or(("", ""));
                        if email != "root@pnex.local" || secret != "whatever" {
                            return (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "message": "Unauthorized Access"
                                })),
                            );
                        }
                        // Real shapes used by the backend: bare name
                        // `soil_moisture` or equality
                        // `soil_moisture{device_id="fuzzy-zebra"}`.
                        let query = params.get("query").map(String::as_str).unwrap_or("");
                        let (name, device_filter) = match query.split_once('{') {
                            Some((n, sel)) => (
                                n.to_string(),
                                sel.split("device_id=\"")
                                    .nth(1)
                                    .and_then(|r| r.split('"').next())
                                    .map(str::to_string),
                            ),
                            None => (query.to_string(), None),
                        };
                        let start: f64 = params
                            .get("start")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0.0);
                        let end: f64 = params
                            .get("end")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(f64::MAX);
                        // Points per (metric, device) within the window —
                        // exact Prometheus matrix shape, ts in seconds.
                        #[allow(clippy::type_complexity)]
                        let mut series: HashMap<
                            (String, String),
                            (Vec<(String, String)>, Vec<(f64, f64)>),
                        > = HashMap::new();
                        for (o2_org, _b, metric, labels, value, ts_ms) in &guard.ingested {
                            if o2_org != &org || metric != &name {
                                continue;
                            }
                            let device = labels
                                .iter()
                                .find(|(n, _)| n == "device_id")
                                .map(|(_, v)| v.clone())
                                .unwrap_or_default();
                            if device_filter.as_ref().is_some_and(|d| d != &device) {
                                continue;
                            }
                            let ts_s = *ts_ms as f64 / 1000.0;
                            if ts_s < start || ts_s > end {
                                continue;
                            }
                            series
                                .entry((metric.clone(), device))
                                .or_insert((labels.clone(), vec![]))
                                .1
                                .push((ts_s, *value));
                        }
                        let result: Vec<serde_json::Value> = series
                            .into_iter()
                            .map(|((metric, _device), (labels, mut points))| {
                                points.sort_by(|a, b| a.0.total_cmp(&b.0));
                                let mut metric_map = serde_json::Map::new();
                                metric_map
                                    .insert("__name__".to_string(), serde_json::json!(metric));
                                for (n, v) in labels {
                                    metric_map.insert(n, serde_json::json!(v));
                                }
                                let values: Vec<serde_json::Value> = points
                                    .into_iter()
                                    .map(|(ts, v)| serde_json::json!([ts, v.to_string()]))
                                    .collect();
                                serde_json::json!({ "metric": metric_map, "values": values })
                            })
                            .collect();
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "status": "success",
                                "data": {"resultType": "matrix", "result": result}
                            })),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/{org}/streams",
            get({
                let s = s.clone();
                move |Path(org): Path<String>,
                      headers: axum::http::HeaderMap,
                      axum::extract::Query(params): axum::extract::Query<
                    HashMap<String, String>,
                >| {
                    let s = s.clone();
                    async move {
                        let basic =
                            basic_of(headers.get("authorization").and_then(|v| v.to_str().ok()));
                        let mut guard = s.lock().unwrap();
                        guard.query_basics.push(basic.clone());
                        let (email, secret) = basic.split_once(':').unwrap_or(("", ""));
                        if email != "root@pnex.local" || secret != "whatever" {
                            return (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "message": "Unauthorized Access"
                                })),
                            );
                        }
                        // Distinct metrics streams of the org (type=metrics
                        // as the client requests it).
                        if params.get("type").map(String::as_str) != Some("metrics") {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                axum::Json(serde_json::json!({"message": "type requis"})),
                            );
                        }
                        let mut names: Vec<String> = guard
                            .ingested
                            .iter()
                            .filter(|(o2_org, _, _, _, _, _)| o2_org == &org)
                            .map(|(_, _, metric, _, _, _)| metric.clone())
                            .collect();
                        names.sort();
                        names.dedup();
                        let list: Vec<serde_json::Value> = names
                            .into_iter()
                            .map(|name| serde_json::json!({ "name": name }))
                            .collect();
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({ "list": list })),
                        )
                    }
                }
            }),
        )
        // SQL search over a metrics stream: the per-device last sample time
        // (`max(_timestamp)`, µs) that the catalog and the home latest
        // measurements use. Auth counters are left untouched.
        .route(
            "/api/{org}/_search",
            post({
                let s = s.clone();
                move |Path(org): Path<String>, body: String| {
                    let s = s.clone();
                    async move {
                        let sql = serde_json::from_str::<serde_json::Value>(&body)
                            .ok()
                            .and_then(|v| v["query"]["sql"].as_str().map(str::to_string))
                            .unwrap_or_default();
                        let metric = sql
                            .split("FROM \"")
                            .nth(1)
                            .and_then(|rest| rest.split('"').next())
                            .unwrap_or_default()
                            .to_string();
                        let guard = s.lock().unwrap();
                        let mut last: HashMap<String, i64> = HashMap::new();
                        for (o2_org, _, m, labels, _, ts_ms) in &guard.ingested {
                            if o2_org != &org || m != &metric {
                                continue;
                            }
                            let Some((_, device)) = labels.iter().find(|(k, _)| k == "device_id")
                            else {
                                continue;
                            };
                            let e = last.entry(device.clone()).or_insert(0);
                            *e = (*e).max(ts_ms * 1000);
                        }
                        let hits: Vec<serde_json::Value> = last
                            .into_iter()
                            .map(|(d, ts)| serde_json::json!({"device_id": d, "last_ts": ts}))
                            .collect();
                        axum::Json(serde_json::json!({"total": hits.len(), "hits": hits}))
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), state)
}

fn o2_client(base: &str) -> Client {
    Client::new(&openobserve::OpenobserveSettings {
        base_url: base.to_string(),
        root_email: "root@pnex.local".into(),
        root_password: "whatever".into(),
    })
}

// ───────────────────────── Harnais ─────────────────────────

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(
        axum_test::TestServer,
        String,
        loco_rs::app::AppContext,
        String,
        Arc<Mutex<MockState>>,
    ) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let (o2_base, mock) = spawn_mock_o2().await;
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx, o2_base, mock).await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org perso")
}

fn point(org_id: i64, metric: &str, value: &str) -> TelemetryPoint {
    TelemetryPoint {
        org_id,
        device_registry_id: 1,
        device_id: "capteur-1".into(),
        pred_dev: "temp_sensor".into(),
        metric_name: metric.into(),
        value: value.into(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: "sensor",
        record: true,
    }
}

// ───────────────────────── Tests ─────────────────────────

/// Idempotent provisioning: org looked up by name before creation, user
/// created once, credentials served from the database on the 2nd call.
#[tokio::test]
#[serial]
async fn provisioning_idempotent() {
    with_app(|server, auth, ctx, o2_base, mock| async move {
        let org = personal_org(&server, &auth).await;
        let client = o2_client(&o2_base);

        let first = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("provision 1");
        let second = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("provision 2");
        assert_eq!(first.o2_org, second.o2_org);

        {
            let guard = mock.lock().unwrap();
            assert_eq!(
                guard.org_creates, 1,
                "org created only once (lookup by name)"
            );
            assert_eq!(guard.user_creates, 1, "user created only once");
            assert_eq!(guard.resets, 0);
        }
        let expected_email = format!("pnex-ingest+org{org}@pnex.local");
        assert_eq!(
            first.email_passcode,
            format!("{}:{}", expected_email, passcode_of(&expected_email))
        );

        // Database row: correlates org ↔ O2 org ↔ token, provisioned.
        use pnex_backend::models::_entities::{
            openobserve_orgs, sea_orm_active_enums::OpenobserveOrgStatus,
        };
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let row = openobserve_orgs::Entity::find()
            .filter(openobserve_orgs::Column::OrgId.eq(org))
            .one(&ctx.db)
            .await
            .expect("row")
            .expect("row");
        assert_eq!(row.status, OpenobserveOrgStatus::Provisioned);
        assert_eq!(row.o2_org, first.o2_org);
        assert_eq!(
            row.ingestion_token.as_deref(),
            Some(first.email_passcode.as_str())
        );
    })
    .await;
}

/// PG row lost: the org is found again by name (no 2nd org), the user's
/// password is reset by root, passcode re-read, row recreated.
#[tokio::test]
#[serial]
async fn provisioning_recupere_ligne_perdue() {
    with_app(|server, auth, ctx, o2_base, mock| async move {
        let org = personal_org(&server, &auth).await;
        let client = o2_client(&o2_base);
        let first = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("provision");

        // Simulates the loss of the row (O2 intact).
        sea_orm::ConnectionTrait::execute_unprepared(&ctx.db, "DELETE FROM openobserve_orgs")
            .await
            .expect("line lost");

        let second = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("re-provision");
        assert_eq!(first.o2_org, second.o2_org, "same org found again by name");
        assert_eq!(first.email_passcode, second.email_passcode);

        let guard = mock.lock().unwrap();
        assert_eq!(guard.org_creates, 1, "no 2nd org created on the O2 side");
        assert_eq!(guard.user_creates, 1);
        assert_eq!(guard.resets, 1, "password reset to regain control");
    })
    .await;
}

/// Batcher: points leave grouped by org towards _json with the Basic
/// correlated from the database; non-numeric values are dropped.
#[tokio::test]
#[serial]
async fn batcher_flush_vers_openobserve() {
    telemetry::reset_sink();
    with_app(|server, auth, ctx, o2_base, mock| async move {
        let org = personal_org(&server, &auth).await;
        let client = o2_client(&o2_base);
        openobserve::sink::spawn_batcher_with(
            ctx.db.clone(),
            client,
            IngestSettings {
                batch_flush_secs: 1,
                ..Default::default()
            },
        );

        telemetry::sink().send(point(org, "read_temperature", "21.5"));
        telemetry::sink().send(point(org, "read_temperature", "22.5"));
        telemetry::sink().send(point(org, "read_temperature", "n/a"));

        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

        let ingested = mock.lock().unwrap().ingested.clone();
        assert!(!ingested.is_empty(), "at least one batch flushed");
        let email = format!("pnex-ingest+org{org}@pnex.local");
        for (o2_org, basic, _metric, _labels, _v, _ts) in &ingested {
            assert_eq!(basic, &format!("{}:{}", email, passcode_of(&email)));
            assert_eq!(o2_org, "mockid1");
        }
        // Two numeric samples, the non-numeric one is dropped.
        assert_eq!(ingested.len(), 2);
        let (_, _, metric, labels, value, ts_ms) = &ingested[0];
        assert_eq!(metric, "read_temperature");
        assert_eq!(*value, 21.5);
        assert!(labels.contains(&("device_id".to_string(), "capteur-1".to_string())));
        assert!(labels.contains(&("source_type".to_string(), "sensor".to_string())));
        assert!(labels.contains(&("ts_source".to_string(), "server".to_string())));
        assert!(*ts_ms > 1_700_000_000_000, "timestamp ms epoch");
    })
    .await;
    telemetry::reset_sink();
}

/// Prometheus query on the real O2 v0.92.1 (seen e2e): the email:passcode
/// Basic is REFUSED on the query (it only serves ingestion) — the client
/// tries passcode then falls back to root Basic, which answers. The
/// vector response is parsed and carries the LAST sample per series.
/// Along the way: the regex selector `{__name__=~…}` returns EMPTY (seen
/// e2e) — only bare names select.
#[tokio::test]
async fn prom_query_bascule_root_et_parse() {
    let (base, mock) = spawn_mock_o2().await;
    let email = "pnex-ingest+org7@pnex.local";
    {
        let mut guard = mock.lock().unwrap();
        guard
            .users
            .insert(("mockid1".into(), email.into()), "pw".into());
        for (value, ts_ms) in [(21.5, 1_755_600_000_000i64), (22.5, 1_755_600_030_000)] {
            guard.ingested.push((
                "mockid1".into(),
                String::new(),
                "read_temperature".into(),
                vec![("device_id".into(), "capteur-1".into())],
                value,
                ts_ms,
            ));
        }
        // Another org: must not leak into the response.
        guard.ingested.push((
            "mockid2".into(),
            String::new(),
            "secret_metric".into(),
            vec![("device_id".into(), "autre".into())],
            99.0,
            1_755_600_090_000,
        ));
    }
    let client = o2_client(&base);
    let email_passcode = format!("{email}:{}", passcode_of(email));
    let resp = client
        .prom_query(
            "mockid1",
            "last_over_time(read_temperature[1h])",
            &email_passcode,
        )
        .await
        .expect("query via root");
    assert_eq!(resp.status, "success");
    assert_eq!(resp.data.result_type, "vector");
    assert_eq!(resp.data.result.len(), 1, "one series for mockid1");
    let sample = &resp.data.result[0];
    assert_eq!(sample.metric["__name__"], "read_temperature");
    assert_eq!(sample.metric["device_id"], "capteur-1");
    assert_eq!(sample.value.1, "22.5", "last sample of the series");
    assert_eq!(sample.value.0, 1_755_600_030.0);

    // Seen e2e on v0.92.1: the catch-all regex selects NOTHING.
    let resp = client
        .prom_query(
            "mockid1",
            r#"last_over_time({__name__=~".+"}[1h])"#,
            &email_passcode,
        )
        .await
        .expect("query accepted");
    assert!(resp.data.result.is_empty(), "catch-all empty on real O2");

    // Passcode tried then refused, root answered (on each call).
    let basics = mock.lock().unwrap().query_basics.clone();
    assert_eq!(basics.len(), 4, "2 calls × (passcode THEN root)");
    assert_eq!(basics[0], email_passcode);
    assert_eq!(basics[1], "root@pnex.local:whatever");
    assert_eq!(basics[3], "root@pnex.local:whatever");
}

/// Passcode AND root refused (e.g. root password rotated) → explicit
/// error, no panic — the dashboard will degrade to available:false.
#[tokio::test]
async fn prom_query_erreur_si_tout_refuse() {
    let (base, _mock) = spawn_mock_o2().await;
    let client = Client::new(&openobserve::OpenobserveSettings {
        base_url: base,
        root_email: "root@pnex.local".into(),
        root_password: "mauvais-mot-de-passe".into(),
    });
    let err = client
        .prom_query("mockid1", "up", "pnex-ingest+org7@pnex.local:o2oi_x")
        .await
        .expect_err("both auths are refused");
    assert!(err.contains("refused"), "message: {err}");
}

/// Prometheus range query (Visualisation page): same passcode→root
/// fallback as the instant query, equality selector
/// `{device_id="…"}` honoured, points returned in chronological order
/// within the [start, end] window only.
#[tokio::test]
async fn query_range_bascule_root_et_parse() {
    let (base, mock) = spawn_mock_o2().await;
    let email = "pnex-ingest+org7@pnex.local";
    {
        let mut guard = mock.lock().unwrap();
        guard
            .users
            .insert(("mockid1".into(), email.into()), "pw".into());
        // Three points for fuzzy-zebra (one OUTSIDE the window) and one
        // point from another device — only the first two must
        // come back, the device filter excluding the rest.
        for (value, ts_ms) in [
            (100.0, 1_755_600_000_000i64),
            (99.5, 1_755_600_030_000),
            (98.0, 1_755_600_300_000),
        ] {
            guard.ingested.push((
                "mockid1".into(),
                String::new(),
                "soil_moisture".into(),
                vec![("device_id".into(), "fuzzy-zebra".into())],
                value,
                ts_ms,
            ));
        }
        guard.ingested.push((
            "mockid1".into(),
            String::new(),
            "soil_moisture".into(),
            vec![("device_id".into(), "autre-capteur".into())],
            42.0,
            1_755_600_015_000,
        ));
    }
    let client = o2_client(&base);
    let email_passcode = format!("{email}:{}", passcode_of(email));
    // Window covering the first two points only.
    let resp = client
        .prom_query_range(
            "mockid1",
            r#"soil_moisture{device_id="fuzzy-zebra"}"#,
            1_755_600_000,
            1_755_600_060,
            30,
            &email_passcode,
        )
        .await
        .expect("query range via root");
    assert_eq!(resp.status, "success");
    assert_eq!(resp.data.result_type, "matrix");
    assert_eq!(resp.data.result.len(), 1, "a single series (device filter)");
    let sample = &resp.data.result[0];
    assert_eq!(sample.metric["__name__"], "soil_moisture");
    assert_eq!(sample.metric["device_id"], "fuzzy-zebra");
    assert_eq!(sample.values.len(), 2, "point outside the window excluded");
    assert_eq!(sample.values[0], (1_755_600_000.0, "100".to_string()));
    assert_eq!(sample.values[1], (1_755_600_030.0, "99.5".to_string()));

    // Bare name WITHOUT a selector: both devices of the metric.
    let resp = client
        .prom_query_range(
            "mockid1",
            "soil_moisture",
            0,
            1_755_600_400,
            60,
            &email_passcode,
        )
        .await
        .expect("bare name accepted");
    assert_eq!(resp.data.result.len(), 2, "one series per device");

    // Passcode tried then refused, root answered (on each call).
    let basics = mock.lock().unwrap().query_basics.clone();
    assert_eq!(basics.len(), 4, "2 calls × (passcode THEN root)");
    assert_eq!(basics[0], email_passcode);
    assert_eq!(basics[1], "root@pnex.local:whatever");
}

/// Full dashboard summary: PNEX_O2_URL enables the openobserve section
/// of test.yaml (Tera), the provisioned org + measurements in the mock →
/// available:true with the latest measurements sorted and deduplicated per
/// series. Dedicated harness: the variable must be set BEFORE boot.
#[tokio::test]
#[serial]
async fn dashboard_summary_complet_contre_mock() {
    let kc = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &kc) };
    let alice = common::valid_token(
        &kc,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let (o2_base, mock) = spawn_mock_o2().await;
    unsafe { std::env::set_var("PNEX_O2_URL", &o2_base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            // Boot has read the variable — remove it so it doesn't leak
            // into the binary's following tests.
            unsafe { std::env::remove_var("PNEX_O2_URL") };

            let org = personal_org(&server, &alice).await;
            // Provisions the O2 org (as the first datum would)
            // then pushes measurements into the mock.
            let client = o2_client(&o2_base);
            let creds = ensure_org_credentials(&ctx.db, &client, org)
                .await
                .expect("provision");
            {
                let mut guard = mock.lock().unwrap();
                for (metric, value, ts_ms) in [
                    ("read_temperature", 21.5, 1_755_600_000_000i64),
                    ("read_temperature", 22.0, 1_755_600_030_000),
                    ("soil_moisture", 42.0, 1_755_600_060_000),
                ] {
                    guard.ingested.push((
                        creds.o2_org.clone(),
                        String::new(),
                        metric.into(),
                        vec![("device_id".into(), "esp-001".into())],
                        value,
                        ts_ms,
                    ));
                }
            }

            let res = server
                .get("/api/v1/dashboard/summary")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200);
            let body: serde_json::Value = res.json();
            assert_eq!(body["telemetry"]["available"], true);
            let latest = body["telemetry"]["latest"].as_array().unwrap();
            assert_eq!(latest.len(), 2, "one row per (metric, device)");
            assert_eq!(
                latest[0]["metric"], "soil_moisture",
                "sorted by ts descending"
            );
            assert_eq!(latest[0]["value"], 42.0);
            assert_eq!(latest[0]["device_id"], "esp-001");
            assert!(!latest[0]["timestamp"].as_str().unwrap_or("").is_empty());
            assert_eq!(latest[1]["value"], 22.0, "last sample of read_temperature");

            // Auth: passcode refused then root answered (actual
            // v0.92.1 behaviour) — on streams AND each per-metric query
            // (2 metrics: read_temperature, soil_moisture).
            let basics = mock.lock().unwrap().query_basics.clone();
            assert_eq!(
                basics.len(),
                6,
                "streams + 2 queries × (passcode THEN root)"
            );
            assert_eq!(basics[0], creds.email_passcode);
            assert_eq!(basics[1], "root@pnex.local:whatever");
            assert_eq!(basics[5], "root@pnex.local:whatever");
        },
    )
    .await;
}

/// Full Visualisation endpoints: PNEX_O2_URL enables the openobserve
/// section of test.yaml, the provisioned org + points in the mock →
/// catalogue available (one series per metric × device) and a windowed
/// series with ordered points. The points use REAL timestamps
/// (now): unlike the instant mock, query_range filters
/// on server-computed [start, end].
#[tokio::test]
#[serial]
async fn visualization_catalog_et_series_contre_mock() {
    let kc = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &kc) };
    let alice = common::valid_token(
        &kc,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let (o2_base, mock) = spawn_mock_o2().await;
    unsafe { std::env::set_var("PNEX_O2_URL", &o2_base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            unsafe { std::env::remove_var("PNEX_O2_URL") };

            let org = personal_org(&server, &alice).await;
            let client = o2_client(&o2_base);
            let creds = ensure_org_credentials(&ctx.db, &client, org)
                .await
                .expect("provision");
            let now_ms = chrono::Utc::now().timestamp_millis();
            {
                let mut guard = mock.lock().unwrap();
                for (metric, value, ts_ms) in [
                    ("soil_moisture", 100.0, now_ms - 120_000),
                    ("soil_moisture", 99.5, now_ms - 60_000),
                    ("read_temperature", 21.5, now_ms - 90_000),
                ] {
                    guard.ingested.push((
                        creds.o2_org.clone(),
                        String::new(),
                        metric.into(),
                        vec![
                            ("device_id".into(), "esp-001".into()),
                            ("pred_dev".into(), "temp_sensor".into()),
                        ],
                        value,
                        ts_ms,
                    ));
                }
            }

            // Catalogue: one series per (metric × device), sorted.
            let res = server
                .get("/api/v1/telemetry/catalog")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200);
            let body: serde_json::Value = res.json();
            assert_eq!(body["available"], true);
            let series = body["series"].as_array().unwrap();
            assert_eq!(series.len(), 2);
            assert_eq!(series[0]["metric"], "read_temperature", "alphabetical sort");
            assert_eq!(series[0]["device_id"], "esp-001");
            assert_eq!(series[0]["pred_dev"], "temp_sensor");
            assert_eq!(series[0]["last_value"], 21.5);
            assert_eq!(series[1]["metric"], "soil_moisture");
            assert_eq!(series[1]["last_value"], 99.5, "last sample");

            // Series: ordered points of the window (1 h covers now-2 min).
            let res = server
                .get("/api/v1/telemetry/series?metric=soil_moisture&device_id=esp-001&window=1h")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200);
            let body: serde_json::Value = res.json();
            assert_eq!(body["available"], true);
            let points = body["points"].as_array().unwrap();
            assert_eq!(points.len(), 2, "both points of the series");
            assert_eq!(points[0]["value"], 100.0, "ordre chronologique");
            assert_eq!(points[1]["value"], 99.5);
            let ts0 = points[0]["ts"].as_f64().unwrap();
            let ts1 = points[1]["ts"].as_f64().unwrap();
            assert!(ts0 < ts1);
            // Real timestamps: within the 1 h window that just opened
            // (the points date from now-2 min and now-1 min).
            let now_secs = now_ms as f64 / 1000.0;
            assert!(
                ts0 > now_secs - 3600.0 && ts1 <= now_secs,
                "timestamps within the window: {ts0}..{ts1} vs now {now_secs}"
            );

            // The device filter excludes the other devices (mock fidelity).
            let res = server
                .get("/api/v1/telemetry/series?metric=soil_moisture&device_id=autre&window=1h")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200);
            let body: serde_json::Value = res.json();
            assert_eq!(body["available"], true);
            assert_eq!(body["points"].as_array().unwrap().len(), 0);
        },
    )
    .await;
}
