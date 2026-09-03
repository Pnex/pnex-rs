//! Tests du canal device `/ws/device` + endpoints pins/commands (Brick 0).
//!
//! Harnais identique à `ws_ingest.rs` (PG requis — TEST_DATABASE_URL).
//! Client miroir : chiffre les DeviceMsg / déchiffre les ServerMsg.

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

// ─────────────────── Client miroir (rôle firmware générique) ───────────────────

fn encrypt(plain: &str, key: &[u8; 32]) -> String {
    use rand::RngCore;
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let mut buf = plain.as_bytes().to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(&nonce)).apply_keystream(&mut buf);
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&buf);
    STANDARD.encode(wire)
}

fn decrypt(raw: &str, key: &[u8; 32]) -> String {
    let bytes = STANDARD.decode(raw.trim()).expect("b64");
    let (nonce, ct) = bytes.split_at(12);
    let mut buf = ct.to_vec();
    ChaCha20::new(Key::from_slice(key), Nonce::from_slice(nonce)).apply_keystream(&mut buf);
    String::from_utf8(buf).expect("utf8")
}

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

fn close_code(msg: axum_test::WsMessage) -> Option<u16> {
    match msg {
        axum_test::WsMessage::Close(Some(frame)) => Some(u16::from(frame.code)),
        _ => None,
    }
}

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

/// Enregistre un device generic_esp8266 via l'API.
async fn create_generic(server: &axum_test::TestServer, auth: &str, device_id: &str) -> Dev {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "generic_esp8266",
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Dev {
        id: dto["id"].as_i64().expect("id"),
        device_id: device_id.into(),
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

/// Announce → attend le ProvisionAck, retourne les caps reçues.
async fn announce_and_expect_provision(
    ws: &mut axum_test::TestWebSocket,
    key: &[u8; 32],
) -> Vec<pnex_core::PinSpec> {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "0.1.0"
    })
    .to_string();
    ws.send_text(encrypt(&announce, key)).await;
    let raw = ws.receive_text().await;
    let plain = decrypt(&raw, key);
    let msg: pnex_core::ServerMsg = serde_json::from_str(&plain).expect("ServerMsg");
    match msg {
        pnex_core::ServerMsg::ProvisionAck { caps } => caps,
        other => panic!("ProvisionAck attendu, reçu : {other:?}"),
    }
}


/// Connexion WS `/ws/device` (auth b64 query, comme le firmware).
async fn connect(server: &axum_test::TestServer, d: &Dev) -> axum_test::TestWebSocket {
    server
        .get_websocket(&format!(
            "/ws/device?token={}&device_id={}",
            b64_param(&d.token),
            b64_param(&d.device_id),
        ))
        .await
        .into_websocket()
        .await
}
// ─────────────────── Tests ───────────────────

/// Cycle nominal : Announce → ProvisionAck (pin map NodeMCU) → StateReport
/// (mémoire last_values + sortie télémétrie) → GET /pins.
#[tokio::test]
#[serial]
async fn announce_provision_et_state_report() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-jardin").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let caps = announce_and_expect_provision(&mut ws, &dev.key).await;
        assert_eq!(caps.len(), 10, "NodeMCU : D0-D8 + A0");
        let d5 = caps.iter().find(|c| c.label == "D5").expect("D5");
        assert_eq!((d5.gpio, d5.mode), (14, pnex_core::Mode::DigitalIn));
        let a0 = caps.iter().find(|c| c.label == "A0").expect("A0");
        assert_eq!((a0.gpio, a0.mode), (17, pnex_core::Mode::AdcIn));

        // StateReport D5=HIGH → mémoire + télémétrie (série d5, generic_gpio).
        let report = serde_json::json!({"t": "state_report", "gpio": 14, "value": 1}).to_string();
        ws.send_text(encrypt(&report, &dev.key)).await;
        // StateReport D6 booléen (le firmware envoie true/false pour les pins
        // digitaux) → télémétrie 1/0 (Prometheus n'a pas de booléens), UI
        // garde le booléen brut pour l'affichage HIGH/LOW. Avant le fix, ce
        // point était silencieusement jeté par le parse f64 de promwrite.
        let report = serde_json::json!({"t": "state_report", "gpio": 12, "value": true}).to_string();
        ws.send_text(encrypt(&report, &dev.key)).await;
        // Attente active brève : la session traite les frames en tâche de fond.
        let org = personal_org(&server, &auth).await;
        for _ in 0..40 {
            let res = server
                .get(&format!("/api/v1/devices/{}/pins", dev.id))
                .add_header("Authorization", format!("Bearer {auth}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            let body: serde_json::Value = res.json();
            let d6row = body["pins"].as_array().unwrap().iter()
                .find(|p| p["label"] == "D6").cloned();
            if d6row.as_ref().and_then(|p| p.get("last_value")).is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        // GET /pins final : 10 pins triés (A0, D0…D8 — l'ordre SQL est
        // arbitraire), D5 numérique, D6 booléen brut, connected=true.
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status_ok();
        let body: serde_json::Value = res.json();
        let pins = body["pins"].as_array().expect("pins array");
        assert_eq!(pins.len(), 10);
        assert_eq!(body["connected"], serde_json::json!(true));
        let labels: Vec<&str> = pins.iter().map(|p| p["label"].as_str().unwrap()).collect();
        assert_eq!(labels, vec!["A0", "D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8"]);
        let d5 = pins.iter().find(|p| p["label"] == "D5").expect("D5");
        assert_eq!(d5["last_value"], serde_json::json!(1));
        assert_eq!(d5["mode"], serde_json::json!("digital_in"));
        let d6 = pins.iter().find(|p| p["label"] == "D6").expect("D6");
        assert_eq!(d6["last_value"], serde_json::json!(true));
        // Télémétrie : d5 numérique 1 ET d6 booléen converti "1" (même sortie
        // que l'ingest).
        let pts = sink.0.lock().unwrap().clone();
        assert!(
            pts.iter().any(|p| p.metric_name == "d5"
                && p.source_type == "generic_gpio"
                && p.value == "1"
                && p.device_id == "gen-jardin"),
            "point télémétrie d5 attendu, reçu : {pts:?}"
        );
        assert!(
            pts.iter().any(|p| p.metric_name == "d6" && p.value == "1"),
            "point télémétrie d6 (bool → 1) attendu, reçu : {pts:?}"
        );
        ws.close().await;
    })
    .await;
}

/// set_mode → write : validation chip-caps AVANT push (400 avec raison),
/// offline = 409 (jamais d'attente serveur, D17), mode persisté même offline
/// (le prochain Announce l'appliquera au reconnect).
#[tokio::test]
#[serial]
async fn commandes_validation_puis_offline_409() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-relais").await;
        let org = personal_org(&server, &auth).await;
        // Announce préalable : les instances (pins) n'existent qu'après.
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws, &dev.key).await;
        ws.close().await;
        // write sur un pin en digital_in → 400.
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "write", "gpio": 5, "value": true}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        // set_mode illégal : D8 = GPIO15 avec safe_state high → 400 (strapping).
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 15, "mode": "digital_out",
                "opts": {"safe_state": "high"}
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert!(
            res.json::<serde_json::Value>().to_string().contains("strapping"),
            "raison chip-caps attendue"
        );
        // set_mode légal mais device offline → 409 ; mode persisté quand même.
        // (D5 = GPIO14 sur NodeMCU — le label D5 n'a jamais désigné GPIO5.)
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 14, "mode": "digital_out",
                "opts": {"safe_state": "low"}
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        // Le mode est persisté : GET /pins montre D5 digital_out.
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body: serde_json::Value = res.json();
        let d5 = body["pins"].as_array().unwrap().iter()
            .find(|p| p["label"] == "D5").expect("D5");
        assert_eq!(d5["mode"], serde_json::json!("digital_out"));
        // write désormais légal sur D5 (gpio 14) mais toujours offline → 409.
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "write", "gpio": 14, "value": true}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
    })
    .await;
}

/// Anti-clone : deuxième session pendant la première → close 4003.
#[tokio::test]
#[serial]
async fn anti_clone_4003() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-clone").await;
        let mut ws1 = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws1, &dev.key).await;
        let mut ws2 = connect(&server, &dev).await;
        let code = loop {
            let m = ws2.receive_message().await;
            if let Some(c) = close_code(m) {
                break c;
            }
        };
        assert_eq!(code, 4003);
        ws1.close().await;
    })
    .await;
}
