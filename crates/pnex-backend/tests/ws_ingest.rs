//! Tests de parité du WS d'ingestion (Phase 5) — cf.
//! `docs/phase0/ws-channels-crypto.md` §2.1 : handshake base64, frames
//! ChaCha20 nu, PING/PONG, validation key=value, close codes, anti-clone.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Config test : `silence_ttl_secs: 2`, `token_cache_secs: 0`
//! (revalidation à chaque frame → 4005 déterministe).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::{ChaCha20, Key, Nonce};
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::telemetry::{self, TelemetryPoint, TelemetrySink};
use serial_test::serial;
use std::sync::{Arc, Mutex};

// ─────────────────── Client miroir (rôle firmware) ───────────────────

fn encrypt(plain: &str, key: &[u8; 32]) -> String {
    use rand::RngCore;
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let mut buf = plain.as_bytes().to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(&nonce))
        .apply_keystream(&mut buf);
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&buf);
    STANDARD.encode(wire)
}

fn decrypt(raw: &str, key: &[u8; 32]) -> String {
    let bytes = STANDARD.decode(raw.trim()).expect("b64");
    let (nonce, ct) = bytes.split_at(12);
    let mut buf = ct.to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(nonce))
        .apply_keystream(&mut buf);
    String::from_utf8(buf).expect("utf8")
}

/// Paramètre query tel que le firmware l'envoie (base64 du texte).
fn b64_param(raw: &str) -> String {
    STANDARD.encode(raw)
}

fn key_bytes(b64: &str) -> [u8; 32] {
    STANDARD
        .decode(b64)
        .expect("clé b64")
        .try_into()
        .expect("clé 32 o")
}

/// Code d'une frame Close reçue (None si pas une close).
fn close_code(msg: axum_test::WsMessage) -> Option<u16> {
    match msg {
        axum_test::WsMessage::Close(Some(frame)) => Some(u16::from(frame.code)),
        _ => None,
    }
}

// ─────────────────── Sink enregistreur ───────────────────

#[derive(Default)]
struct RecSink(Mutex<Vec<TelemetryPoint>>);

impl TelemetrySink for RecSink {
    fn send(&self, point: TelemetryPoint) {
        self.0.lock().expect("sink").push(point);
    }
}

// ─────────────────── Harnais ───────────────────

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, loco_rs::app::AppContext) -> Fut,
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
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx).await;
        },
    )
    .await;
}

struct Dev {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

/// Enregistre un device via l'API (retourne token + clé pour le WS).
async fn create_device(
    server: &axum_test::TestServer,
    auth: &str,
    device_id: &str,
    predefined: &str,
) -> Dev {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Dev {
        id: dto["id"].as_i64().expect("id"),
        device_id: device_id.to_string(),
        token: dto["device_token"]["token"].as_str().expect("token").into(),
        key: key_bytes(dto["device_token"]["encryption_key"].as_str().expect("key")),
    }
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

/// Connexion WS avec les paramètres base64 du firmware.
async fn connect(
    server: &axum_test::TestServer,
    token: &str,
    device_id: &str,
) -> axum_test::TestWebSocket {
    server
        .get_websocket(&format!(
            "/ws/sensor/ingest?token={}&device_id={}",
            b64_param(token),
            b64_param(device_id),
        ))
        .await
        .into_websocket()
        .await
}

// ─────────────────── Tests ───────────────────

/// Cycle complet chiffré : PING/PONG, mesure ok (→ sink, scopée org),
/// erreurs de format et de validation, désalignement de clé.
#[tokio::test]
#[serial]
async fn cycle_ingest_chiffre_complet() {
    telemetry::reset_sink();
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "capteur-jardin", "sensor_probe_v1").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let org = personal_org(&server, &auth).await;

        let mut ws = connect(&server, &dev.token, &dev.device_id).await;

        ws.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws.receive_text().await, &dev.key), "PONG");

        // Mesure valide (capacité du modèle) → ok + point scopé org.
        ws.send_text(encrypt("read_temperature=21.5", &dev.key)).await;
        assert_eq!(decrypt(&ws.receive_text().await, &dev.key), "ok");

        // Mesure hors capacités (device strict) → error:invalid_capability.
        ws.send_text(encrypt("soil_moisture=42", &dev.key)).await;
        let err = decrypt(&ws.receive_text().await, &dev.key);
        assert!(err.starts_with("error:invalid_capability:"), "{err}");

        // Formats invalides.
        for (frame, expected) in [
            ("sans_egal", "error:invalid_format"),
            ("=5", "error:empty_key"),
        ] {
            ws.send_text(encrypt(frame, &dev.key)).await;
            assert_eq!(decrypt(&ws.receive_text().await, &dev.key), expected);
        }
        let long = format!("{}=1", "x".repeat(101));
        ws.send_text(encrypt(&long, &dev.key)).await;
        assert_eq!(
            decrypt(&ws.receive_text().await, &dev.key),
            "error:measurement_name_too_long"
        );

        // Frame non déchiffrable (clé désynchronisée) → ERROR:decryption_failed.
        ws.send_text(encrypt("read_temperature=1", &[9u8; 32])).await;
        assert_eq!(
            decrypt(&ws.receive_text().await, &dev.key),
            "ERROR:decryption_failed"
        );

        // Le sink a reçu exactement la mesure valide, avec le routage org.
        let points = sink.0.lock().expect("sink").clone();
        assert_eq!(points.len(), 1);
        let p = &points[0];
        assert_eq!(p.org_id, org);
        assert_eq!(p.device_id, "capteur-jardin");
        assert_eq!(p.metric_name, "read_temperature");
        assert_eq!(p.value, "21.5");
        assert_eq!(p.pred_dev, "sensor_probe_v1");
        assert_eq!(p.source_type, "sensor");
        assert_eq!(p.ts_source, "server");

        // Le bail de vie est pris (last_seen frais, connected).
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        use pnex_backend::models::_entities::device_states;
        let state = device_states::Entity::find()
            .filter(device_states::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("state")
            .expect("state présent");
        assert!(state.connected);
        ws.close().await;
        telemetry::reset_sink();
    })
    .await;
}

/// Close codes d'auth : 4002 sans token, 4001 token inconnu, 4006 mismatch,
/// 4008 sans clé. Paramètre `\n` trailing (encodage firmware) trimé.
#[tokio::test]
#[serial]
async fn close_codes_authentification() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "dev-a", "sensor_probe_v1").await;
        let other = create_device(&server, &auth, "dev-b", "sensor_probe_v1").await;

        // 4002 : pas de token.
        let mut ws = server
            .get_websocket("/ws/sensor/ingest")
            .await
            .into_websocket()
            .await;
        assert_eq!(close_code(ws.receive_message().await), Some(4002));

        // 4001 : token inconnu.
        let mut ws = connect(&server, "inconnu", "dev-a").await;
        assert_eq!(close_code(ws.receive_message().await), Some(4001));

        // 4006 : token de dev-a, device_id de dev-b.
        let mut ws = connect(&server, &dev.token, &other.device_id).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4006));

        // 4008 : clé absente.
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        use pnex_backend::models::_entities::device_tokens;
        let mut row: device_tokens::ActiveModel = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(other.id))
            .one(&ctx.db)
            .await
            .expect("tok")
            .expect("tok")
            .into();
        row.encryption_key = Set(None);
        row.update(&ctx.db).await.expect("key null");
        let mut ws = connect(&server, &other.token, &other.device_id).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4008));

        // Trim `\n` : le firmware encode `echo | base64` (newline final)
        // — le serveur trime après décodage.
        let mut ws = connect(&server, &format!("{}\n", dev.token), &dev.device_id).await;
        ws.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws.receive_text().await, &dev.key), "PONG");
        ws.close().await;
    })
    .await;
}

/// Anti-clone : 4003 pendant une session ouverte ; déconnexion propre =
/// bail libéré (reconnect immédiat accepté) ; last_seen périmé d'un crash
/// n'occupe plus le bail.
#[tokio::test]
#[serial]
async fn anti_clone_bail() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_device(&server, &auth, "clone-target", "sensor_probe_v1").await;

        // Session 1 ouverte.
        let mut ws1 = connect(&server, &dev.token, &dev.device_id).await;
        ws1.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws1.receive_text().await, &dev.key), "PONG");

        // Clone rejeté pendant la session.
        let mut ws2 = connect(&server, &dev.token, &dev.device_id).await;
        assert_eq!(close_code(ws2.receive_message().await), Some(4003));

        // Déconnexion propre : bail libéré, reconnect immédiat OK.
        ws1.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let mut ws3 = connect(&server, &dev.token, &dev.device_id).await;
        ws3.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws3.receive_text().await, &dev.key), "PONG");
        ws3.close().await;

        // Simule un crash (session non refermée) : last_seen périmé
        // (TTL test = 2 s) → le bail est expiré, connexion acceptée.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws4 = connect(&server, &dev.token, &dev.device_id).await;
        ws4.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws4.receive_text().await, &dev.key), "PONG");
        ws4.close().await;
    })
    .await;
}

/// Device dynamique : découverte des mesures, plafond max_unique.
#[tokio::test]
#[serial]
async fn dynamique_decouverte_et_plafond() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "custom-1", "custom_sensor").await;

        // Plafond à 2 mesures distinctes pour tester vite.
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        use pnex_backend::models::_entities::device_registries;
        let mut row: device_registries::ActiveModel =
            device_registries::Entity::find_by_id(dev.id)
                .one(&ctx.db)
                .await
                .expect("dev")
                .expect("dev")
                .into();
        row.max_unique_measurements = Set(2);
        row.update(&ctx.db).await.expect("plafond");

        let mut ws = connect(&server, &dev.token, &dev.device_id).await;
        for (name, value) in [("pression", "1.2"), ("humidite", "88")] {
            ws.send_text(encrypt(&format!("{name}={value}"), &dev.key)).await;
            assert_eq!(decrypt(&ws.receive_text().await, &dev.key), "ok");
        }
        ws.send_text(encrypt("tension=3.3", &dev.key)).await;
        assert_eq!(
            decrypt(&ws.receive_text().await, &dev.key),
            "error:too_many_measurements"
        );

        // La découverte est persistée (JSONB, relecture au reconnect).
        let row = device_registries::Entity::find_by_id(dev.id)
            .one(&ctx.db)
            .await
            .expect("dev")
            .expect("dev");
        let names = row.discovered_measurements.expect("jsonb");
        assert!(names.get("pression").is_some() && names.get("humidite").is_some());
        ws.close().await;
    })
    .await;
}

/// 4005 : token désactivé en cours de session (cache test = 0 s → la frame
/// suivante revalide et coupe).
#[tokio::test]
#[serial]
async fn revalidation_token_desactive() {
    with_app(|server, auth, ctx| async move {
        let dev = create_device(&server, &auth, "ephemere", "sensor_probe_v1").await;
        let mut ws = connect(&server, &dev.token, &dev.device_id).await;
        ws.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(decrypt(&ws.receive_text().await, &dev.key), "PONG");

        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        use pnex_backend::models::_entities::device_tokens;
        let mut row: device_tokens::ActiveModel = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("tok")
            .expect("tok")
            .into();
        row.is_active = Set(false);
        row.update(&ctx.db).await.expect("désactivation");

        ws.send_text(encrypt("PING", &dev.key)).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4005));
    })
    .await;
}
