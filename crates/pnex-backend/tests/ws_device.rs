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
    use rand::RngExt;
    let mut nonce = [0u8; 12];
    rand::rng().fill(&mut nonce);
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
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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
    create_of(server, auth, device_id, "generic_esp8266").await
}

/// Creates a device on a given model.
async fn create_of(
    server: &axum_test::TestServer,
    auth: &str,
    device_id: &str,
    predefined: &str,
) -> Dev {
    create_with(
        server,
        auth,
        serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }),
    )
    .await
}

/// Creates a custom firmware device: an ESP32 firmware project attached to
/// the seeded `generic_esp32` model (the sketch owns the pins).
async fn create_custom(server: &axum_test::TestServer, auth: &str, device_id: &str) -> Dev {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/firmware-projects")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({"name": device_id, "chip_family": "esp32"}))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let project = res.json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("project id");
    create_with(
        server,
        auth,
        serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": "generic_esp32",
            "firmware_project_id": project,
        }),
    )
    .await
}

async fn create_with(server: &axum_test::TestServer, auth: &str, body: serde_json::Value) -> Dev {
    let org = personal_org(server, auth).await;
    let device_id = body["device_id"].as_str().expect("device_id").to_string();
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&body)
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Dev {
        id: dto["id"].as_i64().expect("id"),
        device_id,
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
        pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
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

/// Attend la libération effective de la session device (GET /pins
/// connected=false) — la course close()-client → teardown serveur est
/// asynchrone ; sans attente, un POST « offline » peut gagner la course et
/// répondre 200 (flake observé 2026-09-13).
async fn wait_offline(server: &axum_test::TestServer, auth: &str, org: i64, dev_id: i64) {
    for _ in 0..80 {
        let res = server
            .get(&format!("/api/v1/devices/{dev_id}/pins"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        if res.json::<serde_json::Value>()["connected"] == serde_json::json!(false) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("session jamais libérée après close()");
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
        let report =
            serde_json::json!({"t": "state_report", "gpio": 12, "value": true}).to_string();
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
            let d6row = body["pins"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["label"] == "D6")
                .cloned();
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
        assert_eq!(
            labels,
            vec!["A0", "D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8"]
        );
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
        wait_offline(&server, &auth, org, dev.id).await;
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
            res.json::<serde_json::Value>()
                .to_string()
                .contains("strapping"),
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
        let d5 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "D5")
            .expect("D5");
        assert_eq!(d5["mode"], serde_json::json!("digital_out"));
        // write désormais légal sur D5 (gpio 14) mais toujours offline → 409
        // (wait_offline déjà passé — la session reste fermée).
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

/// A half-open session (silent beyond the silence TTL) is superseded by
/// the reconnection: the new session is admitted, the old one is closed,
/// its late teardown keeps the device online, and commands reach the new
/// session (route/session authority, owner-checked release).
#[tokio::test]
#[serial]
async fn stale_session_is_superseded_and_commands_reach_the_new_one() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-stale").await;
        let org = personal_org(&server, &auth).await;
        let mut ws1 = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws1, &dev.key).await;
        // ws1 goes silent: its lease is stale after the 2 s test TTL.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws2 = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws2, &dev.key).await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let state = pnex_backend::services::device_liveness::seen_of(&ctx.db, dev.id)
            .await
            .expect("state");
        assert!(state.connected, "old session teardown keeps the new lease");
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(
            res.json::<serde_json::Value>()["connected"],
            serde_json::json!(true)
        );

        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 14, "mode": "digital_out",
                "opts": {"safe_state": "low"}
            }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.status_code());
        let plain = decrypt(&ws2.receive_text().await, &dev.key);
        let msg: pnex_core::ServerMsg = serde_json::from_str(&plain).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::SetMode { gpio: 14, .. }),
            "command delivered to the new session: {msg:?}"
        );
        drop(ws1);
        ws2.close().await;
    })
    .await;
}

// ─────────────────── F2 : manifeste (D47) + horloge (D46) ───────────────────

// ─────────────────── OTA phase 2 : announce inventory ───────────────────

/// OTA inventory ("who runs where"): the announce persists fw_version and
/// the `ota` cap attestation — GET /devices/{id} reflects both.
#[tokio::test]
#[serial]
async fn announce_persiste_fw_version_et_ota_ready() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-inventaire").await;
        let org = personal_org(&server, &auth).await;
        let mut ws = connect(&server, &dev).await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "42",
            "caps": [
                {"id": "digital_state", "family": "state"},
                {"id": "adc_raw", "family": "measurement", "unit": "raw"},
                {"id": "relay_cmd", "family": "actuator"},
                {"id": "ota", "family": "maintenance"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::ProvisionAck { .. }),
            "ProvisionAck attendu, reçu : {msg:?}"
        );
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;

        let res = server
            .get(&format!("/api/v1/devices/{}", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::OK);
        let dto: serde_json::Value = res.json();
        assert_eq!(dto["fw_version"], "42");
        assert_eq!(dto["ota_ready"], true);
    })
    .await;
}

/// Without the `ota` cap the device stays ota_ready=false (the OTA API
/// refuses it) — but fw_version persists anyway (inventory regardless of
/// the manifest).
#[tokio::test]
#[serial]
async fn announce_sans_cap_ota_reste_pas_admis() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-off").await;
        let org = personal_org(&server, &auth).await;
        let mut ws = connect(&server, &dev).await;
        let _ = announce_and_expect_provision(&mut ws, &dev.key).await;
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;

        let res = server
            .get(&format!("/api/v1/devices/{}", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::OK);
        let dto: serde_json::Value = res.json();
        assert_eq!(dto["fw_version"], "0.1.0");
        assert_eq!(dto["ota_ready"], false);
    })
    .await;
}

/// Announce avec manifeste de capacités (D47) : admission inchangée
/// (overlay = autorité du pin_slave, attestation journalisée) → ProvisionAck.
#[tokio::test]
#[serial]
async fn announce_avec_manifeste_accepte() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-manifeste").await;
        let mut ws = connect(&server, &dev).await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "1.1.0",
            "caps": [
                {"id": "digital_state", "family": "state"},
                {"id": "adc_raw", "family": "measurement", "unit": "raw"},
                {"id": "relay_cmd", "family": "actuator"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::ProvisionAck { ref caps, .. } if caps.len() == 10),
            "ProvisionAck attendu, reçu : {msg:?}"
        );
        ws.close().await;
    })
    .await;
}

/// StateReport étendu SANS cap_id (le pin_slave ne l'émet pas — deux pins
/// digital_in partagent `digital_state`) : routage par label overlay
/// inchangé, champs D46 tolérés.
#[tokio::test]
#[serial]
async fn state_report_etendu_route_par_label() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-horloge").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws, &dev.key).await;
        let report = serde_json::json!({
            "t": "state_report", "gpio": 14, "value": 1,
            "uptime_ms": 123456u64, "boot_id": "a1b2c3d4", "seq": 42u64
        })
        .to_string();
        ws.send_text(encrypt(&report, &dev.key)).await;
        for _ in 0..40 {
            {
                let pts = sink.0.lock().unwrap();
                if !pts.is_empty() {
                    let p = &pts[0];
                    assert_eq!(p.metric_name, "d5");
                    assert_eq!(p.source_type, "generic_gpio");
                    assert_eq!(p.value, "1");
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let pts = sink.0.lock().unwrap().clone();
        assert!(
            pts.iter()
                .any(|p| p.metric_name == "d5" && p.source_type == "generic_gpio"),
            "routage label attendu, reçu : {pts:?}"
        );
        ws.close().await;
    })
    .await;
}

/// cap_id présent (F3 : futur firmware par capacité) → série sémantique
/// `{org}/{device}/{capacité}` (source_type generic_cap), LAST_VALUES reste
/// alimenté par gpio (UI /pins — bascule douce §7).
#[tokio::test]
#[serial]
async fn state_report_cap_route_le_sens() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-cap").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws, &dev.key).await;
        let report = serde_json::json!({
            "t": "state_report", "gpio": 14, "value": 25.5,
            "cap_id": "temperature", "uptime_ms": 1000u64, "boot_id": "b1", "seq": 1u64
        })
        .to_string();
        ws.send_text(encrypt(&report, &dev.key)).await;
        for _ in 0..40 {
            if !sink.0.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let pts = sink.0.lock().unwrap().clone();
        assert!(
            pts.iter()
                .any(|p| p.metric_name == "temperature" && p.source_type == "generic_cap"),
            "série par capacité attendue, reçu : {pts:?}"
        );
        assert!(
            !pts.iter().any(|p| p.metric_name == "d5"),
            "pas de série doublée par label quand cap_id routé"
        );
        // UI : LAST_VALUES toujours par gpio (bascule douce).
        let org = personal_org(&server, &auth).await;
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let body: serde_json::Value = res.json();
        let d5 = body["pins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["label"] == "D5")
            .expect("D5");
        assert_eq!(d5["last_value"], serde_json::json!(25.5));
        ws.close().await;
    })
    .await;
}

/// RegState → séries O2 (F2 : lève la limite « journalisé, pas encore
/// routé » du §8 de control-cards.md). Mesure NaN = pas de point (trou,
/// jamais d'invention) ; out/cycles toujours émis.
#[tokio::test]
#[serial]
async fn reg_state_route_vers_o2() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-regdiag").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws, &dev.key).await;
        let msg = serde_json::json!({
            "t": "reg_state", "entries": [
                {"node_id": "r1", "kind": "tt_heat", "setpoint": 19.0,
                 "sensor_value": 18.4, "output_pct": 100.0, "cycle_count": 3},
                {"node_id": "r2", "kind": "pid", "setpoint": 21.0,
                 "sensor_value": null, "output_pct": 0.0, "cycle_count": 0}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&msg, &dev.key)).await;
        for _ in 0..40 {
            if sink.0.lock().unwrap().len() >= 5 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let pts = sink.0.lock().unwrap().clone();
        let reg: Vec<_> = pts
            .iter()
            .filter(|p| p.source_type == "reg_state")
            .collect();
        assert_eq!(
            reg.len(),
            5,
            "r1 sensor+out+cycles, r2 out+cycles (sensor NaN) : {pts:?}"
        );
        assert!(reg
            .iter()
            .any(|p| p.metric_name == "r1_sensor" && p.value == "18.4"));
        assert!(reg
            .iter()
            .any(|p| p.metric_name == "r1_out" && p.value == "100.0"));
        assert!(reg
            .iter()
            .any(|p| p.metric_name == "r1_cycles" && p.value == "3"));
        assert!(reg
            .iter()
            .any(|p| p.metric_name == "r2_out" && p.value == "0.0"));
        assert!(reg
            .iter()
            .any(|p| p.metric_name == "r2_cycles" && p.value == "0"));
        ws.close().await;
    })
    .await;
}

/// Générique ESP32-C3 (Seeed XIAO ESP32C3) : admission overlay D0–D10 avec
/// les chip-caps C3 — analog_in refusé sur D3 (GPIO5 = ADC2, cassé avec
/// WiFi), safe_state: low refusé en digital_out sur D8/D9 (strapping
/// boot-HIGH), analog_in admis sur D0 (GPIO2 = ADC1).
#[tokio::test]
#[serial]
async fn admission_et_chip_caps_esp32c3() {
    with_app(|server, auth, ctx| async move {
        // Board C3 + overlay D0–D10 + predefined device (miroir du seed
        // fixtures/devices/board_overlay_xiao_esp32c3.yaml).
        use pnex_backend::models::_entities::{device_types, mcu_boards, predefined_devices};
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let overlay: pnex_core::BoardOverlay = serde_json::from_value(serde_json::json!({
            "board": "xiao_esp32c3",
            "pins": [
                {"label": "D0", "gpio": 2, "kind": "digital"},
                {"label": "D1", "gpio": 3, "kind": "digital"},
                {"label": "D2", "gpio": 4, "kind": "digital"},
                {"label": "D3", "gpio": 5, "kind": "digital"},
                {"label": "D4", "gpio": 6, "kind": "digital"},
                {"label": "D5", "gpio": 7, "kind": "digital"},
                {"label": "D6", "gpio": 21, "kind": "digital"},
                {"label": "D7", "gpio": 20, "kind": "digital"},
                {"label": "D8", "gpio": 8, "kind": "digital"},
                {"label": "D9", "gpio": 9, "kind": "digital"},
                {"label": "D10", "gpio": 10, "kind": "digital"}
            ]
        }))
        .expect("overlay c3");
        let board = mcu_boards::ActiveModel {
            name: Set("esp32-c3".into()),
            soc: Set("esp32-c3".into()),
            details: Set(Some(serde_json::to_value(&overlay).expect("overlay json"))),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("board c3");
        let mixed = device_types::Entity::find()
            .filter(device_types::Column::Name.eq("mixed"))
            .one(&ctx.db)
            .await
            .expect("db")
            .expect("type mixed");
        predefined_devices::ActiveModel {
            name: Set("generic_esp32c3".into()),
            revision: Set("v1".into()),
            device_type_id: Set(mixed.id),
            board_id: Set(board.id),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("predefined c3");

        // Enregistrement + annonce : le ProvisionAck porte la carte D0–D10.
        let org = personal_org(&server, &auth).await;
        let res = server
            .post("/api/v1/devices")
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "device_id": "gen-xiao",
                "predefined_device_name": "generic_esp32c3",
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let dto: serde_json::Value = res.json();
        let dev = Dev {
            id: dto["id"].as_i64().expect("id"),
            device_id: "gen-xiao".into(),
            token: dto["device_token"]["token"].as_str().expect("token").into(),
            key: key_bytes(dto["device_token"]["encryption_key"].as_str().expect("key")),
        };
        let mut ws = connect(&server, &dev).await;
        // Announce avec le chip réel du firmware C3 (miroir main.cpp).
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32-c3", "board": "xiao_esp32c3", "fw": "1.0.0"
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck attendu, reçu : {other:?}"),
        };
        assert_eq!(caps.len(), 11, "11 pins D0–D10 attendus");
        let labels: Vec<&str> = caps.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10"]
        );
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;

        // analog_in sur D3 (GPIO5 = ADC2) → 400 avec la raison chip-caps C3.
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "set_mode", "gpio": 5, "mode": "analog_in"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert!(
            res.json::<serde_json::Value>().to_string().contains("ADC2"),
            "raison ADC2 attendue"
        );

        // analog_in sur D0 (GPIO2 = ADC1) → légal (persisté, offline = 409).
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "set_mode", "gpio": 2, "mode": "analog_in"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);

        // safe_state: low en digital_out sur D9 (GPIO9, strapping boot-HIGH)
        // → 400 ; high → légal (409 offline, persisté).
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 9, "mode": "digital_out",
                "opts": {"safe_state": "low"}
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert!(
            res.json::<serde_json::Value>()
                .to_string()
                .contains("strapping"),
            "raison strapping attendue"
        );
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 9, "mode": "digital_out",
                "opts": {"safe_state": "high"}
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
    })
    .await;
}

/// Custom firmware: admission by the pins declared in the sketch
/// (`Announce.pins`), strict chip-caps on the board's SoC; the sketch's
/// labels show in GET /pins; undeclared pins are pruned.
#[tokio::test]
#[serial]
async fn custom_firmware_admits_declared_pins() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_custom(&server, &auth, "custom-1").await;
        let mut ws = connect(&server, &dev).await;

        // ── Announce avec pins déclarées (sketch = source) ──
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [
                {"gpio": 5, "label": "pump", "mode": "digital_out"},
                {"gpio": 4, "label": "door", "mode": "digital_in", "pullup": true},
                {"gpio": 34, "label": "photo", "mode": "adc_in"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck attendu, reçu : {other:?}"),
        };
        assert_eq!(caps.len(), 3, "3 pins déclarées admises");
        let by_label = |l: &str| caps.iter().find(|p| p.label == l).expect("label");
        let pump = by_label("pump");
        assert_eq!(pump.gpio, 5);
        assert_eq!(pump.mode, pnex_core::Mode::DigitalOut);
        assert_eq!(pump.safe_state, Some(pnex_core::SafeState::Low));
        // Round-trip pullup : opts.porte la pullup déclarée (fix).
        let door = by_label("door");
        assert_eq!(
            door.opts.as_ref().and_then(|o| o.pullup),
            Some(true),
            "pullup déclarée écho dans ProvisionAck.opts"
        );
        let photo = by_label("photo");
        assert_eq!(photo.mode, pnex_core::Mode::AdcIn);

        // ── Les pins admises sont visibles dans GET /pins (labels sketch) ──
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::OK);
        let body = res.json::<serde_json::Value>();
        let list = body["pins"].as_array().expect("liste pins");
        let labels: Vec<&str> = list.iter().filter_map(|p| p["label"].as_str()).collect();
        assert!(
            labels.contains(&"pump") && labels.contains(&"door") && labels.contains(&"photo"),
            "labels du sketch attendus dans /pins : {labels:?}"
        );

        // ── Strict chip-caps: flash pin 6 refused ──
        let announce_flash = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [{"gpio": 6, "label": "x", "mode": "digital_in"}]
        })
        .to_string();
        ws.send_text(encrypt(&announce_flash, &dev.key)).await;
        let raw = ws.receive_text().await;
        let plain = decrypt(&raw, &dev.key);
        assert!(
            plain.contains("flash SPI"),
            "rejet chip-caps attendu pour gpio flash 6 : {plain}"
        );

        // ── Prune : re-announce SANS « door » → 2 pins restantes ──
        let announce_prune = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [
                {"gpio": 5, "label": "pump", "mode": "digital_out"},
                {"gpio": 34, "label": "photo", "mode": "adc_in"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce_prune, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck attendu, reçu : {other:?}"),
        };
        assert_eq!(caps.len(), 2, "door purgée : 2 pins restantes");

        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;
    })
    .await;
}

/// UART0 console pins (ESP32 GPIO1/3) are never provisioned: a sketch
/// declaring TX is still admitted, without that pin, and it stays out of
/// GET /pins.
#[tokio::test]
#[serial]
async fn admission_skips_uart_console_pins() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_custom(&server, &auth, "custom-console").await;
        let mut ws = connect(&server, &dev).await;

        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [
                {"gpio": 1, "label": "TXD", "mode": "digital_in"},
                {"gpio": 5, "label": "pump", "mode": "digital_out"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("expected ProvisionAck, got: {other:?}"),
        };
        let gpios: Vec<u16> = caps.iter().map(|p| p.gpio).collect();
        assert_eq!(gpios, vec![5], "console TX must not be provisioned");

        let res = server
            .get(&format!("/api/v1/devices/{}/pins", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::OK);
        let body = res.json::<serde_json::Value>();
        let listed: Vec<i64> = body["pins"]
            .as_array()
            .expect("pins list")
            .iter()
            .filter_map(|p| p["gpio"].as_i64())
            .collect();
        assert!(!listed.contains(&1), "console TX listed: {listed:?}");

        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;
    })
    .await;
}

/// Custom firmware without declared pins (a sketch that only publishes
/// metrics): admitted with an empty pin map, previously admitted pins
/// pruned — the overlay never takes over the sketch's pins.
#[tokio::test]
#[serial]
async fn custom_firmware_without_pins_gets_an_empty_pin_map() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_custom(&server, &auth, "custom-2").await;
        let mut ws = connect(&server, &dev).await;
        for (pins, expected) in [
            (
                serde_json::json!([{"gpio": 5, "label": "pump", "mode": "digital_out"}]),
                1,
            ),
            (serde_json::json!([]), 0),
        ] {
            let announce = serde_json::json!({
                "t": "announce", "chip": "esp32", "board": "any", "fw": "1.0.0", "pins": pins
            })
            .to_string();
            ws.send_text(encrypt(&announce, &dev.key)).await;
            let raw = ws.receive_text().await;
            match serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg") {
                pnex_core::ServerMsg::ProvisionAck { caps, .. } => {
                    assert_eq!(caps.len(), expected, "pins: {pins}")
                }
                other => panic!("expected ProvisionAck, got: {other:?}"),
            }
        }
        ws.close().await;
    })
    .await;
}

/// A generic model whose board has no overlay and no custom firmware has
/// nothing to admit: refused at connect (4007).
#[tokio::test]
#[serial]
async fn generic_without_overlay_nor_custom_firmware_refused() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_of(&server, &auth, "plain-esp32", "generic_esp32").await;
        let mut ws = connect(&server, &dev).await;
        let code = loop {
            if let Some(c) = close_code(ws.receive_message().await) {
                break c;
            }
        };
        assert_eq!(code, 4007);
    })
    .await;
}

// ─────────────────── OTA phase 3 : assignment lifecycle ───────────────────

/// Announce with the `ota` cap (fw version announced) — helper: returns
/// after reading the ProvisionAck frame.
async fn announce_ota(ws: &mut axum_test::TestWebSocket, key: &[u8; 32], fw: &str) {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": fw,
        "caps": [
            {"id": "digital_state", "family": "state"},
            {"id": "adc_raw", "family": "measurement", "unit": "raw"},
            {"id": "relay_cmd", "family": "actuator"},
            {"id": "ota", "family": "maintenance"}
        ]
    })
    .to_string();
    ws.send_text(encrypt(&announce, key)).await;
    let raw = ws.receive_text().await;
    let msg: pnex_core::ServerMsg = serde_json::from_str(&decrypt(&raw, key)).expect("ServerMsg");
    assert!(
        matches!(msg, pnex_core::ServerMsg::ProvisionAck { .. }),
        "ProvisionAck attendu, reçu : {msg:?}"
    );
}

/// POST /build-firmware via the fixture toolchain → returns the build id.
/// Raises the Free-tier mixed-device quota first (seeded at 1: a build for
/// the single generic device would be rejected before this bump).
async fn post_build(
    ctx: &loco_rs::app::AppContext,
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    device_id: &str,
) -> i64 {
    use sea_orm::ConnectionTrait;
    ctx.db
        .execute_unprepared(
            "UPDATE subscription_tiers SET max_mixed_devices = 9 WHERE name = 'Free'",
        )
        .await
        .expect("tier bump");
    let res = server
        .post("/api/v1/build-firmware")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "wifi_ssid": "net", "wifi_password": "pw",
            "device_id": device_id, "predefined_device_name": "generic_esp8266",
            "pnex_host": "h", "ws_ssl": false
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["build_id"]
        .as_i64()
        .expect("build_id")
}

/// Full online cycle: build (fixture toolchain) → announce with `ota` cap
/// (fw "41") → POST /ota → pushed, device receives ota_available →
/// OtaState downloading/flashing → GET /ota reflects → re-announce with
/// fw == target → succeeded → POST again → 409 same_version.
#[tokio::test]
#[serial]
async fn ota_cycle_complet_en_ligne() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-cycle").await;
        let org = personal_org(&server, &auth).await;
        let build_id = post_build(&ctx, &server, &auth, org, &dev.device_id).await;

        let mut ws = connect(&server, &dev).await;
        announce_ota(&mut ws, &dev.key, "0.1.0").await;

        // Deploy: online → pushed.
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = res.json();
        assert_eq!(body["pushed"], true);
        assert_eq!(body["state"], "pending");
        assert_eq!(body["target_version"], build_id.to_string());

        // Device side: receive ota_available.
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let cmd_id = match &msg {
            pnex_core::ServerMsg::OtaAvailable { cmd_id, version, sha256, .. } => {
                assert_eq!(version.as_str(), build_id.to_string());
                assert_eq!(sha256.len(), 64);
                cmd_id.clone()
            }
            other => panic!("OtaAvailable attendu, reçu : {other:?}"),
        };

        // Progress: downloading then flashing — the session processes the
        // frame asynchronously, so poll the status endpoint (bounded).
        for (phase, progress) in [("downloading", Some(50u8)), ("flashing", None)] {
            let mut st = serde_json::json!({
                "t": "ota_state", "cmd_id": cmd_id, "phase": phase,
            });
            if let Some(p) = progress {
                st["progress"] = serde_json::json!(p);
            }
            ws.send_text(encrypt(&st.to_string(), &dev.key)).await;
            let mut cur = serde_json::Value::Null;
            for _ in 0..80 {
                let status = server
                    .get(&format!("/api/v1/devices/{}/ota", dev.id))
                    .add_header("Authorization", format!("Bearer {auth}"))
                    .add_header("X-Org-Id", org.to_string())
                    .await;
                cur = status.json::<serde_json::Value>()["current"].clone();
                if cur["state"] == phase {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            assert_eq!(cur["state"], phase);
            if let Some(p) = progress {
                assert_eq!(cur["progress"], p);
            }
        }

        // Post-reboot announce with fw == target → succeeded (no further
        // OtaAvailable push for a terminal row).
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": build_id.to_string(),
            "caps": [{"id": "digital_state", "family": "state"}, {"id": "ota", "family": "maintenance"}]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        // Consume frames until the ProvisionAck (any stray re-push of the
        // assignment is tolerated, mirroring the firmware's tolerance).
        loop {
            let raw = ws.receive_text().await;
            let msg: pnex_core::ServerMsg =
                serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
            if matches!(msg, pnex_core::ServerMsg::ProvisionAck { .. }) {
                break;
            }
        }

        // The row goes terminal (succeeded) — poll the history head.
        let mut head = serde_json::Value::Null;
        for _ in 0..80 {
            let status = server
                .get(&format!("/api/v1/devices/{}/ota", dev.id))
                .add_header("Authorization", format!("Bearer {auth}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            head = status.json::<serde_json::Value>()["history"][0].clone();
            if head["state"] == "succeeded" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(head["state"], "succeeded");
        assert_eq!(head["progress"], 100);
        let status = server
            .get(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        let cur = status.json::<serde_json::Value>()["current"].clone();
        assert_eq!(cur, serde_json::json!(null), "terminal → pas de current");
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        res.json::<serde_json::Value>()["error"]
            .as_str()
            .unwrap_or_default();
        ws.close().await;
    })
    .await;
}

/// Offline create (201, pushed=false) → later announce delivers the
/// ota_available (desired state picked up at the announce, even though the
/// device lagged by 3 versions).
#[tokio::test]
#[serial]
async fn ota_offline_pickup_a_l_annonce() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-offline").await;
        let org = personal_org(&server, &auth).await;
        let _ = post_build(&ctx, &server, &auth, org, &dev.device_id).await;

        // First announce (online): ota_ready becomes known; then the device
        // leaves — the deploy below is offline.
        let mut ws = connect(&server, &dev).await;
        announce_ota(&mut ws, &dev.key, "0.1.0").await;
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;

        // Offline deploy → 201 pushed=false.
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = res.json();
        assert_eq!(body["pushed"], false);
        assert_eq!(body["state"], "pending");

        // Device connects and announces (fw "1.0.0" — legacy version).
        let mut ws = connect(&server, &dev).await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "1.0.0",
            "caps": [{"id": "digital_state", "family": "state"}, {"id": "ota", "family": "maintenance"}]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        // ProvisionAck first…
        let raw = ws.receive_text().await;
        let first: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        assert!(matches!(first, pnex_core::ServerMsg::ProvisionAck { .. }));
        // …then the picked-up OtaAvailable.
        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::OtaAvailable { .. }),
            "OtaAvailable attendu (pickup), reçu : {msg:?}"
        );
        ws.close().await;
    })
    .await;
}

/// Guards: 400 ota_not_ready without the cap; Ack{ok:false} fails the
/// active assignment; device download route is token-gated.
#[tokio::test]
#[serial]
async fn ota_gardes_et_refus_device() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-gardes").await;
        let org = personal_org(&server, &auth).await;
        let build_id = post_build(&ctx, &server, &auth, org, &dev.device_id).await;

        // Without an ota-capable announce → 400 ota_not_ready.
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);

        // Announce with the cap (fw legacy "1.0.0").
        let mut ws = connect(&server, &dev).await;
        announce_ota(&mut ws, &dev.key, "1.0.0").await;

        // Deploy online → pushed.
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let body: serde_json::Value = res.json();
        assert_eq!(body["pushed"], true);

        let raw = ws.receive_text().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&decrypt(&raw, &dev.key)).expect("ServerMsg");
        let cmd_id = match &msg {
            pnex_core::ServerMsg::OtaAvailable { cmd_id, .. } => cmd_id.clone(),
            other => panic!("OtaAvailable attendu, reçu : {other:?}"),
        };

        // Device refuses (Ack{ok:false}) → assignment failed.
        let ack =
            serde_json::json!({"t":"ack","cmd_id":cmd_id,"ok":false,"err":"ota not supported"})
                .to_string();
        ws.send_text(encrypt(&ack, &dev.key)).await;
        let mut head = serde_json::Value::Null;
        for _ in 0..80 {
            let status = server
                .get(&format!("/api/v1/devices/{}/ota", dev.id))
                .add_header("Authorization", format!("Bearer {auth}"))
                .add_header("X-Org-Id", org.to_string())
                .await;
            head = status.json::<serde_json::Value>()["history"][0].clone();
            if head["state"] == "failed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert_eq!(head["state"], "failed");
        assert_eq!(head["error"], "ota not supported");
        ws.close().await;

        // Download route: valid device-token auth + bad token rejected.
        let dl = server
            .get(&format!(
                "/api/v1/ota/firmware/{}/{}?token={}&device_id={}",
                dev.device_id,
                build_id,
                b64_param(&dev.token),
                b64_param(&dev.device_id),
            ))
            .await;
        dl.assert_status(axum_test::http::StatusCode::OK);
        assert!(!dl.as_bytes().is_empty());
        let dl_bad = server
            .get(&format!(
                "/api/v1/ota/firmware/{}/{}/{}",
                dev.device_id, build_id, "wrong-token"
            ))
            .await;
        dl_bad.assert_status(axum_test::http::StatusCode::NOT_FOUND);
    })
    .await;
}
/// Custom firmware (D87/D88): announced `metric` caps are ingested from
/// gpio-less StateReports (unannounced ids dropped), the manifest is
/// persisted, and an announced `command` cap is pushed as ServerMsg::Command
/// (unknown command = 400, never pushed).
#[tokio::test]
#[serial]
async fn custom_metrics_and_commands_roundtrip() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "custom-bme").await;
        let org = personal_org(&server, &auth).await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "7",
            "caps": [
                {"id": "digital_state", "family": "state"},
                {"id": "temp", "family": "metric", "unit": "°C"},
                {"id": "calibrate", "family": "command"}
            ]
        })
        .to_string();
        ws.send_text(encrypt(&announce, &dev.key)).await;
        let plain = decrypt(&ws.receive_text().await, &dev.key);
        assert!(plain.contains("provision_ack"), "{plain}");

        for (cap, value) in [
            ("temp", serde_json::json!(21.5)),
            ("ghost", serde_json::json!(1)),
        ] {
            let report =
                serde_json::json!({"t": "state_report", "cap_id": cap, "value": value}).to_string();
            ws.send_text(encrypt(&report, &dev.key)).await;
        }
        for _ in 0..80 {
            if sink
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.metric_name == "temp")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let pts = sink.0.lock().unwrap().clone();
        assert!(
            pts.iter().any(|p| p.metric_name == "temp"
                && p.source_type == "custom_metric"
                && p.value == "21.5"
                && p.device_id == "custom-bme"),
            "custom metric point expected, got: {pts:?}"
        );
        assert!(
            !pts.iter().any(|p| p.metric_name == "ghost"),
            "unannounced metric must be dropped"
        );

        // Unknown command: refused before any push.
        let res = server
            .post(&format!("/api/v1/devices/{}/custom-commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"name": "nope"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-command-unknown"
        );

        // Announced command: pushed with its args, cmd_id returned.
        let res = server
            .post(&format!("/api/v1/devices/{}/custom-commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"name": "calibrate", "args": {"offset": 1.5}}))
            .await;
        res.assert_status_ok();
        let cmd_id = res.json::<serde_json::Value>()["cmd_id"]
            .as_str()
            .expect("cmd_id")
            .to_string();
        let mut got = None;
        for _ in 0..10 {
            let plain = decrypt(&ws.receive_text().await, &dev.key);
            let msg: pnex_core::ServerMsg = serde_json::from_str(&plain).expect("ServerMsg");
            if let pnex_core::ServerMsg::Command {
                cmd_id: c,
                name,
                args,
            } = msg
            {
                got = Some((c, name, args));
                break;
            }
        }
        let (c, name, args) = got.expect("command pushed to the device");
        assert_eq!((c.as_str(), name.as_str()), (cmd_id.as_str(), "calibrate"));
        assert_eq!(args["offset"], serde_json::json!(1.5));
        ws.close().await;
    })
    .await;
}
