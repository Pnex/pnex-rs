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
use prost::Message as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::openobserve::{self, ensure_org_credentials, Client};
use pnex_backend::services::settings::IngestSettings;
use pnex_backend::services::telemetry::{self, TelemetryPoint};
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
}

fn basic_of(header: Option<&str>) -> String {
    // "Basic b64(email:…)" → "email:…"
    let raw = header.unwrap_or_default().strip_prefix("Basic ").unwrap_or("");
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
                            .chain(guard.orgs.iter().map(|(name, id)| {
                                serde_json::json!({"identifier": id, "name": name})
                            }))
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
                        let basic = basic_of(
                            headers.get("authorization").and_then(|v| v.to_str().ok()),
                        );
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
                        let basic = basic_of(
                            headers.get("authorization").and_then(|v| v.to_str().ok()),
                        );
                        // Décode snappy + protobuf comme le vrai O2.
                        let raw = snap::raw::Decoder::new()
                            .decompress_vec(&body)
                            .unwrap_or_default();
                        let req = pnex_backend::services::openobserve::promwrite::WriteRequest::decode(
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
    let base = common::spawn_mock_keycloak().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("KEYCLOAK_URL", &base) };
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

        let first = ensure_org_credentials(&ctx.db, &client, org).await.expect("provision 1");
        let second = ensure_org_credentials(&ctx.db, &client, org).await.expect("provision 2");
        assert_eq!(first.o2_org, second.o2_org);

        {
            let guard = mock.lock().unwrap();
            assert_eq!(guard.org_creates, 1, "org créée une seule fois (lookup par nom)");
            assert_eq!(guard.user_creates, 1, "user créé une seule fois");
            assert_eq!(guard.resets, 0);
        }
        let expected_email = format!("pnex-ingest+org{org}@pnex.local");
        assert_eq!(
            first.email_passcode,
            format!("{}:{}", expected_email, passcode_of(&expected_email))
        );

        // Ligne en base : correlée org ↔ org O2 ↔ token, provisioned.
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        use pnex_backend::models::_entities::{openobserve_orgs,
            sea_orm_active_enums::OpenobserveOrgStatus};
        let row = openobserve_orgs::Entity::find()
            .filter(openobserve_orgs::Column::OrgId.eq(org))
            .one(&ctx.db)
            .await
            .expect("row")
            .expect("row");
        assert_eq!(row.status, OpenobserveOrgStatus::Provisioned);
        assert_eq!(row.o2_org, first.o2_org);
        assert_eq!(row.ingestion_token.as_deref(), Some(first.email_passcode.as_str()));
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
        let first = ensure_org_credentials(&ctx.db, &client, org).await.expect("provision");

        // Simule la perte de la ligne (O2 intact).
        sea_orm::ConnectionTrait::execute_unprepared(
            &ctx.db,
            "DELETE FROM openobserve_orgs",
        )
        .await
        .expect("perte ligne");

        let second = ensure_org_credentials(&ctx.db, &client, org).await.expect("re-provision");
        assert_eq!(first.o2_org, second.o2_org, "même org retrouvée par nom");
        assert_eq!(first.email_passcode, second.email_passcode);

        let guard = mock.lock().unwrap();
        assert_eq!(guard.org_creates, 1, "aucune 2e org créée côté O2");
        assert_eq!(guard.user_creates, 1);
        assert_eq!(guard.resets, 1, "password réinitialisé pour reprendre la main");
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
