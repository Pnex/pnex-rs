//! Tests OpenObserve (Phase 5) contre un mock fidèle du comportement réel
//! constaté sur v0.92.1 : pas de dédoublonnage des noms d'org, rôle admin
//! seul natif, passcode par user, ingestion `_json` en Basic
//! email:passcode — cf. services/openobserve/mod.rs.

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
    /// (name, identifier) — les doublons de nom créent une 2e entrée,
    /// comme en vrai.
    orgs: Vec<(String, String)>,
    /// (org identifier, email) → password courant.
    users: HashMap<(String, String), String>,
    /// Écriture Prometheus reçue : identifier org, basic "email:passcode",
    /// métrique, labels, valeur, ts_ms.
    #[allow(clippy::type_complexity)]
    ingested: Vec<(String, String, String, Vec<(String, String)>, f64, i64)>,
    org_creates: u32,
    user_creates: u32,
    resets: u32,
    /// Basics reçus sur la route query Prometheus (ordre des tentatives).
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
                        // Comme en vrai : pas de dédoublonnage par nom.
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
                        // Décode snappy + protobuf comme le vrai O2.
                        let raw = snap::raw::Decoder::new()
                            .decompress_vec(&body)
                            .unwrap_or_default();
                        let req =
                            pnex_backend::services::openobserve::promwrite::WriteRequest::decode(
                                raw.as_slice(),
                            )
                            .unwrap_or_else(|_| {
                                pnex_backend::services::openobserve::promwrite::WriteRequest {
                                    timeseries: vec![],
                                }
                            });
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
                        // Auth : Basic root UNIQUEMENT — fidèle au vrai O2
                        // v0.92.1 où le Basic email:passcode est refusé sur
                        // la query (il ne sert que l'ingestion).
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
                        // Fidèle v0.92.1 : les sélecteurs {__name__=~"..."}
                        // (regex) renvoient un vector VIDE — seul le nom nu
                        // (éventuellement wrapped dans last_over_time)
                        // sélectionne.
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
                        // Dernier échantillon par (métrique, device_id) de
                        // l'org — forme vector Prometheus exacte.
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
                                continue; // aucun match, fidèle v0.92.1
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
                        // Auth root uniquement, comme la query instantanée
                        // (fidèle v0.92.1 : passcode = ingestion seule).
                        let (email, secret) = basic.split_once(':').unwrap_or(("", ""));
                        if email != "root@pnex.local" || secret != "whatever" {
                            return (
                                axum::http::StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "message": "Unauthorized Access"
                                })),
                            );
                        }
                        // Formes réelles utilisées par le backend : nom nu
                        // `soil_moisture` ou égalité
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
                        // Points par (métrique, device) dans la fenêtre —
                        // forme matrix Prometheus exacte, ts en secondes.
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
                        // Streams metrics distincts de l'org (type=metrics
                        // comme le client le demande).
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
        pred_dev: "soil_sensor".into(),
        metric_name: metric.into(),
        value: value.into(),
        timestamp: chrono::Utc::now(),
        ts_source: "server",
        source_type: "sensor",
    }
}

// ───────────────────────── Tests ─────────────────────────

/// Provisioning idempotent : org cherchée par nom avant création, user
/// créé une fois, credentials servis depuis la base au 2e appel.
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
                "org créée une seule fois (lookup par nom)"
            );
            assert_eq!(guard.user_creates, 1, "user créé une seule fois");
            assert_eq!(guard.resets, 0);
        }
        let expected_email = format!("pnex-ingest+org{org}@pnex.local");
        assert_eq!(
            first.email_passcode,
            format!("{}:{}", expected_email, passcode_of(&expected_email))
        );

        // Ligne en base : correlée org ↔ org O2 ↔ token, provisioned.
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

/// Perte de la ligne PG : l'org est retrouvée par nom (pas de 2e org), le
/// password du user est réinitialisé par root, passcode relu, ligne recréée.
#[tokio::test]
#[serial]
async fn provisioning_recupere_ligne_perdue() {
    with_app(|server, auth, ctx, o2_base, mock| async move {
        let org = personal_org(&server, &auth).await;
        let client = o2_client(&o2_base);
        let first = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("provision");

        // Simule la perte de la ligne (O2 intact).
        sea_orm::ConnectionTrait::execute_unprepared(&ctx.db, "DELETE FROM openobserve_orgs")
            .await
            .expect("perte ligne");

        let second = ensure_org_credentials(&ctx.db, &client, org)
            .await
            .expect("re-provision");
        assert_eq!(first.o2_org, second.o2_org, "même org retrouvée par nom");
        assert_eq!(first.email_passcode, second.email_passcode);

        let guard = mock.lock().unwrap();
        assert_eq!(guard.org_creates, 1, "aucune 2e org créée côté O2");
        assert_eq!(guard.user_creates, 1);
        assert_eq!(
            guard.resets, 1,
            "password réinitialisé pour reprendre la main"
        );
    })
    .await;
}

/// Batcher : les points partent groupés par org vers _json avec le Basic
/// correlé de la base ; les valeurs non numériques sont abandonnées.
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
        assert!(!ingested.is_empty(), "au moins un lot flushé");
        let email = format!("pnex-ingest+org{org}@pnex.local");
        for (o2_org, basic, _metric, _labels, _v, _ts) in &ingested {
            assert_eq!(basic, &format!("{}:{}", email, passcode_of(&email)));
            assert_eq!(o2_org, "mockid1");
        }
        // Deux samples numériques, le non numérique est abandonné.
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

/// Query Prometheus sur le vrai O2 v0.92.1 (constaté e2e) : le Basic
/// email:passcode est REFUSÉ sur la query (il ne sert que l'ingestion) —
/// le client tente passcode puis bascule en Basic root, qui répond. La
/// réponse vector est parsée et porte le DERNIER échantillon par série.
/// Au passage : le sélecteur regex `{__name__=~…}` renvoie VIDE (constat
/// e2e) — seuls les noms nus sélectionnent.
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
        // Une autre org : ne doit pas fuiter dans la réponse.
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
    assert_eq!(resp.data.result.len(), 1, "une série pour mockid1");
    let sample = &resp.data.result[0];
    assert_eq!(sample.metric["__name__"], "read_temperature");
    assert_eq!(sample.metric["device_id"], "capteur-1");
    assert_eq!(sample.value.1, "22.5", "dernier échantillon de la série");
    assert_eq!(sample.value.0, 1_755_600_030.0);

    // Constat e2e v0.92.1 : le catch-all regex ne sélectionne RIEN.
    let resp = client
        .prom_query(
            "mockid1",
            r#"last_over_time({__name__=~".+"}[1h])"#,
            &email_passcode,
        )
        .await
        .expect("requête acceptée");
    assert!(resp.data.result.is_empty(), "catch-all vide sur O2 réel");

    // Passcode tenté puis refusé, root a répondu (sur chaque appel).
    let basics = mock.lock().unwrap().query_basics.clone();
    assert_eq!(basics.len(), 4, "2 appels × (passcode PUIS root)");
    assert_eq!(basics[0], email_passcode);
    assert_eq!(basics[1], "root@pnex.local:whatever");
    assert_eq!(basics[3], "root@pnex.local:whatever");
}

/// Passcode ET root refusés (ex. mot de passe root retourné) → erreur
/// explicite, pas de panique — le dashboard dégradera en available:false.
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
        .expect_err("les deux auths sont refusées");
    assert!(err.contains("refusé"), "message : {err}");
}

/// Query range Prometheus (page Visualisation) : même bascule
/// passcode→root que la query instantanée, sélecteur égalité
/// `{device_id="…"}` respecté, points rendus en ordre chronologique
/// dans la fenêtre [start, end] uniquement.
#[tokio::test]
async fn query_range_bascule_root_et_parse() {
    let (base, mock) = spawn_mock_o2().await;
    let email = "pnex-ingest+org7@pnex.local";
    {
        let mut guard = mock.lock().unwrap();
        guard
            .users
            .insert(("mockid1".into(), email.into()), "pw".into());
        // Trois points pour fuzzy-zebra (dont un HORS fenêtre) et un
        // point d'un autre device — seuls les deux premiers doivent
        // revenir, le filtre device excluant le reste.
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
    // Fenêtre couvrant les deux premiers points uniquement.
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
    assert_eq!(resp.data.result.len(), 1, "une seule série (filtre device)");
    let sample = &resp.data.result[0];
    assert_eq!(sample.metric["__name__"], "soil_moisture");
    assert_eq!(sample.metric["device_id"], "fuzzy-zebra");
    assert_eq!(sample.values.len(), 2, "point hors fenêtre exclu");
    assert_eq!(sample.values[0], (1_755_600_000.0, "100".to_string()));
    assert_eq!(sample.values[1], (1_755_600_030.0, "99.5".to_string()));

    // Nom nu SANS sélecteur : les deux devices de la métrique.
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
        .expect("nom nu accepté");
    assert_eq!(resp.data.result.len(), 2, "une série par device");

    // Passcode tenté puis refusé, root a répondu (sur chaque appel).
    let basics = mock.lock().unwrap().query_basics.clone();
    assert_eq!(basics.len(), 4, "2 appels × (passcode PUIS root)");
    assert_eq!(basics[0], email_passcode);
    assert_eq!(basics[1], "root@pnex.local:whatever");
}

/// Summary dashboard complet : PNEX_O2_URL active la section openobserve
/// de test.yaml (Tera), l'org provisionnée + mesures dans le mock →
/// available:true avec les dernières mesures triées et dédoublonnées par
/// série. Harnais dédié : la variable doit être posée AVANT le boot.
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
            // Le boot a lu la variable — on la retire pour ne pas fuiter
            // sur les tests suivants du binaire.
            unsafe { std::env::remove_var("PNEX_O2_URL") };

            let org = personal_org(&server, &alice).await;
            // Provisionne l'org O2 (comme la première donnée le ferait)
            // puis pousse des mesures côté mock.
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
            assert_eq!(latest.len(), 2, "une ligne par (métrique, device)");
            assert_eq!(latest[0]["metric"], "soil_moisture", "tri ts décroissant");
            assert_eq!(latest[0]["value"], 42.0);
            assert_eq!(latest[0]["device_id"], "esp-001");
            assert!(!latest[0]["timestamp"].as_str().unwrap_or("").is_empty());
            assert_eq!(
                latest[1]["value"], 22.0,
                "dernier échantillon de read_temperature"
            );

            // Auth : passcode refusé puis root a répondu (comportement
            // réel v0.92.1) — sur le streams ET chaque query par métrique
            // (2 métriques : read_temperature, soil_moisture).
            let basics = mock.lock().unwrap().query_basics.clone();
            assert_eq!(
                basics.len(),
                6,
                "streams + 2 queries × (passcode PUIS root)"
            );
            assert_eq!(basics[0], creds.email_passcode);
            assert_eq!(basics[1], "root@pnex.local:whatever");
            assert_eq!(basics[5], "root@pnex.local:whatever");
        },
    )
    .await;
}

/// Endpoints Visualisation complets : PNEX_O2_URL active la section
/// openobserve de test.yaml, l'org provisionnée + points dans le mock →
/// catalogue disponible (une série par métrique × device) et série sur
/// fenêtre avec points ordonnés. Les points utilisent des timestamps
/// RÉELS (now) : contrairement au mock instantané, query_range filtre
/// sur [start, end] calculés serveur.
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
                            ("pred_dev".into(), "soil_sensor".into()),
                        ],
                        value,
                        ts_ms,
                    ));
                }
            }

            // Catalogue : une série par (métrique × device), triée.
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
            assert_eq!(series[0]["metric"], "read_temperature", "tri alphabétique");
            assert_eq!(series[0]["device_id"], "esp-001");
            assert_eq!(series[0]["pred_dev"], "soil_sensor");
            assert_eq!(series[0]["last_value"], 21.5);
            assert_eq!(series[1]["metric"], "soil_moisture");
            assert_eq!(series[1]["last_value"], 99.5, "dernier échantillon");

            // Série : points ordonnés de la fenêtre (1 h couvre now-2 min).
            let res = server
                .get("/api/v1/telemetry/series?metric=soil_moisture&device_id=esp-001&window=1h")
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200);
            let body: serde_json::Value = res.json();
            assert_eq!(body["available"], true);
            let points = body["points"].as_array().unwrap();
            assert_eq!(points.len(), 2, "les deux points de la série");
            assert_eq!(points[0]["value"], 100.0, "ordre chronologique");
            assert_eq!(points[1]["value"], 99.5);
            let ts0 = points[0]["ts"].as_f64().unwrap();
            let ts1 = points[1]["ts"].as_f64().unwrap();
            assert!(ts0 < ts1);
            // Timestamps réels : dans la fenêtre 1 h qui vient de s'ouvrir
            // (les points datent de now-2 min et now-1 min).
            let now_secs = now_ms as f64 / 1000.0;
            assert!(
                ts0 > now_secs - 3600.0 && ts1 <= now_secs,
                "timestamps dans la fenêtre : {ts0}..{ts1} vs now {now_secs}"
            );

            // Le filtre device exclut les autres devices (fidélité mock).
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
