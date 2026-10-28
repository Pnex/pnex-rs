//! Tests for the device channel `/ws/device` + pins/commands endpoints (Brick 0).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL); the database is emptied between tests.
//! Mirror client: encrypts DeviceMsg / decrypts ServerMsg.

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::telemetry::{self, TelemetryPoint, TelemetrySink};
use serial_test::serial;
use std::sync::{Arc, Mutex};

// ─────────────────── Mirror client (generic firmware role) ───────────────────

fn key_bytes(b64: &str) -> [u8; 32] {
    STANDARD
        .decode(b64)
        .expect("b64 key")
        .try_into()
        .expect("32-byte key")
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

/// Registers a generic_esp8266 device through the API.
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

/// Announce → waits for the ProvisionAck, returns the received caps.
async fn announce_and_expect_provision(ws: &mut common::DevWs) -> Vec<pnex_core::PinSpec> {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "0.1.0"
    })
    .to_string();
    ws.send_plain(&announce).await;
    let raw = ws.recv_plain().await;
    let plain = raw.clone();
    let msg: pnex_core::ServerMsg = serde_json::from_str(&plain).expect("ServerMsg");
    match msg {
        pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
        other => panic!("ProvisionAck expected, got: {other:?}"),
    }
}

/// WS connection `/ws/device` (b64 query auth, like the firmware).
async fn connect(server: &axum_test::TestServer, d: &Dev) -> common::DevWs {
    common::DevWs::connect(
        server,
        &format!("/ws/device?token={}", d.token,),
        &d.key,
        &d.device_id,
    )
    .await
}

/// Waits for the device session to be actually released (GET /pins
/// connected=false) — the client close() → server teardown race is
/// asynchronous; without waiting, an "offline" POST can win the race and
/// answer 200 (flake seen 2026-09-13).
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
    panic!("session never released after close()");
}

// ─────────────────── Tests ───────────────────

/// Nominal cycle: Announce → ProvisionAck (NodeMCU pin map) → StateReport
/// (last_values memory + telemetry output) → GET /pins.
#[tokio::test]
#[serial]
async fn announce_provision_et_state_report() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-jardin").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let caps = announce_and_expect_provision(&mut ws).await;
        assert_eq!(caps.len(), 10, "NodeMCU : D0-D8 + A0");
        let d5 = caps.iter().find(|c| c.label == "D5").expect("D5");
        assert_eq!((d5.gpio, d5.mode), (14, pnex_core::Mode::DigitalIn));
        let a0 = caps.iter().find(|c| c.label == "A0").expect("A0");
        assert_eq!((a0.gpio, a0.mode), (17, pnex_core::Mode::AdcIn));

        // StateReport D5=HIGH → memory + telemetry (series d5, generic_gpio).
        let report = serde_json::json!({"t": "state_report", "gpio": 14, "value": 1}).to_string();
        ws.send_plain(&report).await;
        // Boolean StateReport D6 (the firmware sends true/false for digital
        // pins) → telemetry 1/0 (Prometheus has no booleans), the UI
        // keeps the raw boolean for the HIGH/LOW display. Before the fix, this
        // point was silently dropped by promwrite's f64 parse.
        let report =
            serde_json::json!({"t": "state_report", "gpio": 12, "value": true}).to_string();
        ws.send_plain(&report).await;
        // Brief active wait: the session processes frames in the background.
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
        // Final GET /pins: 10 sorted pins (A0, D0…D8 — the SQL order is
        // arbitrary), D5 numeric, D6 raw boolean, connected=true.
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
        // Telemetry: d5 numeric 1 AND d6 boolean converted to "1" (same output
        // as ingest).
        let pts = sink.0.lock().unwrap().clone();
        assert!(
            pts.iter().any(|p| p.metric_name == "d5"
                && p.source_type == "generic_gpio"
                && p.value == "1"
                && p.device_id == "gen-jardin"),
            "telemetry point d5 expected, got: {pts:?}"
        );
        assert!(
            pts.iter().any(|p| p.metric_name == "d6" && p.value == "1"),
            "telemetry point d6 (bool → 1) expected, got: {pts:?}"
        );
        ws.close().await;
    })
    .await;
}

/// set_mode → write: chip-caps validation BEFORE push (400 with a reason),
/// offline = 409 (never a server-side wait, D17), mode persisted even offline
/// (the next Announce will apply it on reconnect).
#[tokio::test]
#[serial]
async fn commandes_validation_puis_offline_409() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-relais").await;
        let org = personal_org(&server, &auth).await;
        // Prior Announce: the instances (pins) only exist afterwards.
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;
        // write on a pin in digital_in → 400.
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "write", "gpio": 5, "value": true}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        // Illegal set_mode: D8 = GPIO15 with safe_state high → 400 (strapping).
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
        // Legal set_mode but device offline → 409; mode persisted anyway.
        // (D5 = GPIO14 on NodeMCU — the D5 label never meant GPIO5.)
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
        // The mode is persisted: GET /pins shows D5 digital_out.
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
        // write now legal on D5 (gpio 14) but still offline → 409
        // (wait_offline already passed — the session stays closed).
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

/// Anti-clone: second session during the first → close 4003.
#[tokio::test]
#[serial]
async fn anti_clone_4003() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-clone").await;
        let mut ws1 = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws1).await;
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
        let _caps = announce_and_expect_provision(&mut ws1).await;
        // ws1 goes silent: its lease is stale after the 2 s test TTL.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws2 = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws2).await;
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
        let plain = ws2.recv_plain().await;
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
        ws.send_plain(&announce).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::ProvisionAck { .. }),
            "ProvisionAck expected, got: {msg:?}"
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
        let _ = announce_and_expect_provision(&mut ws).await;
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

/// Announce with a capability manifest (D47): admission unchanged
/// (overlay = pin_slave authority, attestation journaled) → ProvisionAck.
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
        ws.send_plain(&announce).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::ProvisionAck { ref caps, .. } if caps.len() == 10),
            "ProvisionAck expected, got: {msg:?}"
        );
        ws.close().await;
    })
    .await;
}

/// Extended StateReport WITHOUT cap_id (the pin_slave doesn't emit it — two
/// digital_in pins share `digital_state`): routing by overlay label
/// unchanged, D46 fields tolerated.
#[tokio::test]
#[serial]
async fn state_report_etendu_route_par_label() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-horloge").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        let report = serde_json::json!({
            "t": "state_report", "gpio": 14, "value": 1,
            "uptime_ms": 123456u64, "boot_id": "a1b2c3d4", "seq": 42u64
        })
        .to_string();
        ws.send_plain(&report).await;
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
            "label routing expected, got: {pts:?}"
        );
        ws.close().await;
    })
    .await;
}

/// cap_id present (F3: future per-capability firmware) → semantic series
/// `{org}/{device}/{capability}` (source_type generic_cap), LAST_VALUES stays
/// fed by gpio (UI /pins — soft switchover §7).
#[tokio::test]
#[serial]
async fn state_report_cap_route_le_sens() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-cap").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        let report = serde_json::json!({
            "t": "state_report", "gpio": 14, "value": 25.5,
            "cap_id": "temperature", "uptime_ms": 1000u64, "boot_id": "b1", "seq": 1u64
        })
        .to_string();
        ws.send_plain(&report).await;
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
            "per-capability series expected, got: {pts:?}"
        );
        assert!(
            !pts.iter().any(|p| p.metric_name == "d5"),
            "no series duplicated by label when cap_id is routed"
        );
        // UI: LAST_VALUES still per gpio (soft switchover).
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

/// RegState → O2 series (F2: lifts the "journaled, not yet routed"
/// limitation of control-cards.md §8). NaN measurement = no point (gap,
/// never invented); out/cycles always emitted.
#[tokio::test]
#[serial]
async fn reg_state_route_vers_o2() {
    telemetry::reset_sink();
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-regdiag").await;
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        let msg = serde_json::json!({
            "t": "reg_state", "entries": [
                {"node_id": "r1", "kind": "tt_heat", "setpoint": 19.0,
                 "sensor_value": 18.4, "output_pct": 100.0, "cycle_count": 3},
                {"node_id": "r2", "kind": "pid", "setpoint": 21.0,
                 "sensor_value": null, "output_pct": 0.0, "cycle_count": 0}
            ]
        })
        .to_string();
        ws.send_plain(&msg).await;
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

/// Generic ESP32-C3 (Seeed XIAO ESP32C3): D0–D10 overlay admission with
/// the C3 chip-caps — analog_in refused on D3 (GPIO5 = ADC2, broken with
/// WiFi), safe_state: low refused as digital_out on D8/D9 (boot-HIGH
/// strapping), analog_in admitted on D0 (GPIO2 = ADC1).
#[tokio::test]
#[serial]
async fn admission_et_chip_caps_esp32c3() {
    with_app(|server, auth, ctx| async move {
        // C3 board with the catalogue profile `xiao_esp32c3` + predefined
        // device.
        use pnex_backend::models::_entities::{device_types, mcu_boards, predefined_devices};
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let profile = (Some(&pnex_core::catalog::boards::XIAO_ESP32C3)
            .and_then(|b| b.profile)
            .expect("catalogue xiao_esp32c3 profile"))();
        let board = mcu_boards::ActiveModel {
            name: Set("esp32-c3".into()),
            soc: Set("esp32-c3".into()),
            details: Set(Some(serde_json::to_value(profile).expect("profile json"))),
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

        // Registration + announce: the ProvisionAck carries the D0–D10 card.
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
        // Announce with the real chip of the C3 firmware (mirrors main.cpp).
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32-c3", "board": "xiao_esp32c3", "fw": "1.0.0"
        })
        .to_string();
        ws.send_plain(&announce).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck expected, got: {other:?}"),
        };
        assert_eq!(caps.len(), 11, "11 pins D0–D10 attendus");
        // Profile order (physical header order), compared as a set.
        let mut labels: Vec<&str> = caps.iter().map(|p| p.label.as_str()).collect();
        labels.sort_by_key(|l| l[1..].parse::<u32>().unwrap_or(u32::MAX));
        assert_eq!(
            labels,
            vec!["D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10"]
        );
        ws.close().await;
        wait_offline(&server, &auth, org, dev.id).await;

        // analog_in on D3 (GPIO5 = ADC2) → 400 with the C3 chip-caps reason.
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

        // analog_in on D0 (GPIO2 = ADC1) → legal (persisted, offline = 409).
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"op": "set_mode", "gpio": 2, "mode": "analog_in"}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);

        // safe_state: low as digital_out on D9 (GPIO9, boot-HIGH strapping)
        // → 400; high → legal (409 offline, persisted).
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

        // ── Announce with declared pins (sketch = source) ──
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [
                {"gpio": 5, "label": "pump", "mode": "digital_out"},
                {"gpio": 4, "label": "door", "mode": "digital_in", "pullup": true},
                {"gpio": 34, "label": "photo", "mode": "adc_in"}
            ]
        })
        .to_string();
        ws.send_plain(&announce).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck expected, got: {other:?}"),
        };
        assert_eq!(caps.len(), 3, "3 declared pins admitted");
        let by_label = |l: &str| caps.iter().find(|p| p.label == l).expect("label");
        let pump = by_label("pump");
        assert_eq!(pump.gpio, 5);
        assert_eq!(pump.mode, pnex_core::Mode::DigitalOut);
        assert_eq!(pump.safe_state, Some(pnex_core::SafeState::Low));
        // Pullup round-trip: opts carries the declared pullup (fix).
        let door = by_label("door");
        assert_eq!(
            door.opts.as_ref().and_then(|o| o.pullup),
            Some(true),
            "declared pullup echoed in ProvisionAck.opts"
        );
        let photo = by_label("photo");
        assert_eq!(photo.mode, pnex_core::Mode::AdcIn);

        // ── The admitted pins are visible in GET /pins (sketch labels) ──
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
            "sketch labels expected in /pins: {labels:?}"
        );

        // ── Strict chip-caps: flash pin 6 refused ──
        let announce_flash = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [{"gpio": 6, "label": "x", "mode": "digital_in"}]
        })
        .to_string();
        ws.send_plain(&announce_flash).await;
        let raw = ws.recv_plain().await;
        let plain = raw.clone();
        assert!(
            plain.contains("flash SPI"),
            "chip-caps rejection expected for flash gpio 6: {plain}"
        );

        // ── Prune: re-announce WITHOUT "door" → 2 remaining pins ──
        let announce_prune = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "ma_carte_maison", "fw": "1.0.0",
            "pins": [
                {"gpio": 5, "label": "pump", "mode": "digital_out"},
                {"gpio": 34, "label": "photo", "mode": "adc_in"}
            ]
        })
        .to_string();
        ws.send_plain(&announce_prune).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        let caps = match msg {
            pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
            other => panic!("ProvisionAck expected, got: {other:?}"),
        };
        assert_eq!(caps.len(), 2, "door purged: 2 pins left");

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
        ws.send_plain(&announce).await;
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
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
            ws.send_plain(&announce).await;
            let raw = ws.recv_plain().await;
            match serde_json::from_str(&raw.clone()).expect("ServerMsg") {
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
async fn announce_ota(ws: &mut common::DevWs, fw: &str) {
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
    ws.send_plain(&announce).await;
    let raw = ws.recv_plain().await;
    let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
    assert!(
        matches!(msg, pnex_core::ServerMsg::ProvisionAck { .. }),
        "ProvisionAck expected, got: {msg:?}"
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
    let wifi = server
        .post("/api/v1/edge/wifi-credentials")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({ "ssid": "net", "password": { "value": "pw" } }))
        .await
        .json::<serde_json::Value>()["id"]
        .as_i64()
        .expect("wifi entry");
    let res = server
        .post("/api/v1/build-firmware")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "wifi_credential_id": wifi, "device_id": device_id, "pnex_host": "h"
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
/// fw == target → succeeded → POST again → 409 ota-same-version.
#[tokio::test]
#[serial]
async fn ota_cycle_complet_en_ligne() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-cycle").await;
        let org = personal_org(&server, &auth).await;
        let build_id = post_build(&ctx, &server, &auth, org, &dev.device_id).await;

        let mut ws = connect(&server, &dev).await;
        announce_ota(&mut ws, "0.1.0").await;

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
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&raw.clone()).expect("ServerMsg");
        let cmd_id = match &msg {
            pnex_core::ServerMsg::OtaAvailable { cmd_id, version, url, sha256, sig, .. } => {
                assert_eq!(version.as_str(), build_id.to_string());
                assert_eq!(url.as_str(), format!("/api/v1/ota/firmware/{build_id}"));
                assert_eq!(sha256.len(), 64);
                // SEC-18: signed by the instance key for this device + version.
                let ring = pnex_backend::services::secrets::Keyring::from_config(&ctx.config).unwrap();
                let key = pnex_backend::services::ota_signing::signing_key(&ctx.db, &ring)
                    .await
                    .unwrap();
                assert_eq!(
                    Some(sig.as_str()),
                    pnex_backend::services::ota_signing::sign_image(
                        &key,
                        &dev.device_id,
                        version,
                        sha256
                    )
                    .as_deref()
                );
                cmd_id.clone()
            }
            other => panic!("OtaAvailable expected, got: {other:?}"),
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
            ws.send_plain(&st.to_string()).await;
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
        ws.send_plain(&announce).await;
        // Consume frames until the ProvisionAck (any stray re-push of the
        // assignment is tolerated, mirroring the firmware's tolerance).
        loop {
            let raw = ws.recv_plain().await;
            let msg: pnex_core::ServerMsg =
                serde_json::from_str(&raw.clone()).expect("ServerMsg");
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
        assert_eq!(cur, serde_json::json!(null), "terminal → no current");
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            pnex_core::err_codes::OTA_SAME_VERSION
        );
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
        announce_ota(&mut ws, "0.1.0").await;
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
        ws.send_plain(&announce).await;
        // ProvisionAck first…
        let raw = ws.recv_plain().await;
        let first: pnex_core::ServerMsg =
            serde_json::from_str(&raw.clone()).expect("ServerMsg");
        assert!(matches!(first, pnex_core::ServerMsg::ProvisionAck { .. }));
        // …then the picked-up OtaAvailable.
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg =
            serde_json::from_str(&raw.clone()).expect("ServerMsg");
        assert!(
            matches!(msg, pnex_core::ServerMsg::OtaAvailable { .. }),
            "OtaAvailable expected (pickup), got: {msg:?}"
        );
        ws.close().await;
    })
    .await;
}

/// Guards: 400 ota-not-ready without the cap; Ack{ok:false} fails the
/// active assignment; device download route is token-gated.
#[tokio::test]
#[serial]
async fn ota_gardes_et_refus_device() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-ota-gardes").await;
        let org = personal_org(&server, &auth).await;
        let build_id = post_build(&ctx, &server, &auth, org, &dev.device_id).await;

        // Without an ota-capable announce → 400 ota-not-ready.
        let res = server
            .post(&format!("/api/v1/devices/{}/ota", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        res.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            pnex_core::err_codes::OTA_NOT_READY
        );

        // Announce with the cap (fw legacy "1.0.0").
        let mut ws = connect(&server, &dev).await;
        announce_ota(&mut ws, "1.0.0").await;

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

        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        let cmd_id = match &msg {
            pnex_core::ServerMsg::OtaAvailable { cmd_id, .. } => cmd_id.clone(),
            other => panic!("OtaAvailable expected, got: {other:?}"),
        };

        // Device refuses (Ack{ok:false}) → assignment failed.
        let ack =
            serde_json::json!({"t":"ack","cmd_id":cmd_id,"ok":false,"err":"ota not supported"})
                .to_string();
        ws.send_plain(&ack).await;
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
            .get(&format!("/api/v1/ota/firmware/{build_id}"))
            .add_header("Authorization", format!("Bearer {}", dev.token))
            .await;
        dl.assert_status(axum_test::http::StatusCode::OK);
        // D154: a token in the URL is no longer read.
        let in_url = server
            .get(&format!(
                "/api/v1/ota/firmware/{}?token={}",
                build_id, dev.token,
            ))
            .await;
        in_url.assert_status(axum_test::http::StatusCode::NOT_FOUND);
        dl.assert_status(axum_test::http::StatusCode::OK);
        assert!(!dl.as_bytes().is_empty());
        let dl_bad = server
            .get(&format!("/api/v1/ota/firmware/{build_id}"))
            .add_header("Authorization", "Bearer wrong-token")
            .await;
        dl_bad.assert_status(axum_test::http::StatusCode::NOT_FOUND);
        assert_eq!(
            dl_bad.json::<serde_json::Value>()["error"],
            pnex_core::err_codes::OTA_AUTH_FAILED
        );
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
        ws.send_plain(&announce).await;
        let plain = ws.recv_plain().await;
        assert!(plain.contains("provision_ack"), "{plain}");

        for (cap, value) in [
            ("temp", serde_json::json!(21.5)),
            ("ghost", serde_json::json!(1)),
        ] {
            let report =
                serde_json::json!({"t": "state_report", "cap_id": cap, "value": value}).to_string();
            ws.send_plain(&report).await;
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
            let plain = ws.recv_plain().await;
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

/// D146: a flow's Device (write) sends custom-firmware commands through the
/// internal route — an announced command is pushed as
/// `ServerMsg::Command { args: {"value": …} }`, an unannounced one is
/// reported in the results and never pushed.
#[tokio::test]
#[serial]
async fn flow_device_write_sends_announced_commands() {
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "test-flow-token") };
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_custom(&server, &auth, "custom-rgb").await;
        let mut ws = connect(&server, &dev).await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp32", "board": "esp32_c6_zero", "fw": "1",
            "caps": [
                {"id": "temperature", "family": "metric", "unit": "°C"},
                {"id": "color", "family": "command"}
            ]
        })
        .to_string();
        ws.send_plain(&announce).await;
        let plain = ws.recv_plain().await;
        assert!(plain.contains("provision_ack"), "{plain}");

        // The pinout lists the announced commands for the flow editor.
        let res = server
            .get(&format!("/api/v1/devices/{}/pinout", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status_ok();
        assert_eq!(
            res.json::<serde_json::Value>()["commands"],
            serde_json::json!(["color"])
        );

        let res = server
            .post("/internal/flow/device-write")
            .add_header("x-pnex-flow-token", "test-flow-token")
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "org_id": org,
                "device_id": dev.device_id,
                "commands": {"color": 16711680, "ghost": 1, "big": "x".repeat(600)},
            }))
            .await;
        res.assert_status_ok();
        let out = res.json::<serde_json::Value>();
        let results = out["results"].as_array().expect("results");
        assert!(
            results
                .iter()
                .any(|r| r["command"] == "color" && r["ok"] == true),
            "{out}"
        );
        assert!(
            results
                .iter()
                .any(|r| r["command"] == "ghost" && r["ok"] == false),
            "{out}"
        );
        assert!(
            results
                .iter()
                .any(|r| r["command"] == "big" && r["err"] == "command value too large"),
            "{out}"
        );

        let mut pushed = Vec::new();
        for _ in 0..10 {
            let plain = ws.recv_plain().await;
            let msg: pnex_core::ServerMsg = serde_json::from_str(&plain).expect("ServerMsg");
            if let pnex_core::ServerMsg::Command { name, args, .. } = msg {
                pushed.push((name, args));
                break;
            }
        }
        assert_eq!(
            pushed,
            vec![("color".to_string(), serde_json::json!({"value": 16711680}))]
        );
        ws.close().await;
    })
    .await;
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
}

/// D154: the device token travels, raw, in the `Authorization` header;
/// the URL carries nothing.
#[tokio::test]
#[serial]
async fn token_in_the_authorization_header_opens_the_link() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-hdr-auth").await;
        let mut ws = common::DevWs::connect_with_header(
            &server,
            "/ws/device",
            &dev.token,
            &dev.key,
            &dev.device_id,
        )
        .await;
        // Handshake + ProvisionAck over the header-authenticated link (the
        // helper panics on anything else).
        announce_and_expect_provision(&mut ws).await;
    })
    .await;
}

// ─────────────────── Link security (auth, Noise, lease) ───────────────────

/// Link with any key (refusal tests).
async fn connect_keyed(server: &axum_test::TestServer, d: &Dev, key: &[u8; 32]) -> common::DevWs {
    common::DevWs::connect(
        server,
        &format!("/ws/device?token={}", d.token),
        key,
        &d.device_id,
    )
    .await
}

/// Noise transport (D156): a replayed or tampered frame is dropped without
/// breaking the link, and a frame captured on one connection is useless on
/// the next one.
#[tokio::test]
#[serial]
async fn noise_link_drops_replay_tampering_and_cross_connection() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-noise").await;
        let mut ws = connect(&server, &dev).await;
        let ping = ws.seal("PING");
        ws.send_text(ping.clone()).await;
        assert_eq!(ws.recv_plain().await, "PONG");

        // Same frame again, then one flipped bit: both dropped silently.
        ws.send_text(ping.clone()).await;
        let mut wire = STANDARD.decode(&ping).unwrap();
        wire[2] ^= 0x01;
        ws.send_text(STANDARD.encode(wire)).await;

        // The link still works after the refusals (counters untouched): the
        // next answer is the PONG of a fresh PING, nothing came before it.
        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");
        ws.close().await;

        // A frame captured on the first connection, replayed on a new one.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let mut ws2 = connect(&server, &dev).await;
        ws2.send_text(ping).await;
        ws2.send_plain("PING").await;
        assert_eq!(ws2.recv_plain().await, "PONG");
        ws2.close().await;
    })
    .await;
}

/// Handshake refused (4011): wrong key, and a firmware speaking the removed
/// D8 framing (its first frame is not a Noise message, D157).
#[tokio::test]
#[serial]
async fn noise_handshake_failures_close_4011() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-4011").await;
        let mut ws = connect_keyed(&server, &dev, &[1u8; 32]).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4011));

        let mut old = server
            .get_websocket("/ws/device")
            .add_header("Authorization", format!("Bearer {}", dev.token))
            .await
            .into_websocket()
            .await;
        // A D8 frame: base64(nonce ‖ ChaCha20("PING")), 16 bytes.
        old.send_text(STANDARD.encode([5u8; 16])).await;
        assert_eq!(close_code(old.receive_message().await), Some(4011));
    })
    .await;
}

/// Auth close codes: 4002 without a token, 4001 for an unknown token. The
/// device is the token's own: no claimed id to mismatch.
#[tokio::test]
#[serial]
async fn auth_close_codes() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-auth").await;

        let mut ws = server
            .get_websocket("/ws/device")
            .await
            .into_websocket()
            .await;
        assert_eq!(close_code(ws.receive_message().await), Some(4002));

        let unknown = Dev {
            id: dev.id,
            device_id: dev.device_id.clone(),
            token: "unknown".into(),
            key: dev.key,
        };
        let mut ws = connect(&server, &unknown).await;
        assert_eq!(close_code(ws.receive_message().await), Some(4001));
    })
    .await;
}

/// Lease: a clean disconnect releases it (immediate reconnect accepted),
/// and the stale lease of a crashed session no longer holds the device.
#[tokio::test]
#[serial]
async fn lease_released_on_close_and_expired_after_a_crash() {
    with_app(|server, auth, _ctx| async move {
        let dev = create_generic(&server, &auth, "gen-lease").await;
        let mut ws1 = connect(&server, &dev).await;
        ws1.send_plain("PING").await;
        assert_eq!(ws1.recv_plain().await, "PONG");
        ws1.close().await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut ws2 = connect(&server, &dev).await;
        ws2.send_plain("PING").await;
        assert_eq!(ws2.recv_plain().await, "PONG");
        // Crash: the session is never closed; the 2 s test TTL elapses.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        let mut ws3 = connect(&server, &dev).await;
        ws3.send_plain("PING").await;
        assert_eq!(ws3.recv_plain().await, "PONG");
        drop(ws2);
        ws3.close().await;
    })
    .await;
}

/// 4005: token deactivated during the session (test cache = 0 s → the next
/// frame revalidates and closes).
#[tokio::test]
#[serial]
async fn token_deactivated_during_the_session_closes_4005() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "gen-revoked").await;
        let mut ws = connect(&server, &dev).await;
        ws.send_plain("PING").await;
        assert_eq!(ws.recv_plain().await, "PONG");

        use pnex_backend::models::_entities::device_tokens;
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let mut row: device_tokens::ActiveModel = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("token")
            .expect("token")
            .into();
        row.is_active = Set(false);
        row.update(&ctx.db).await.expect("deactivate");

        ws.send_plain("PING").await;
        assert_eq!(close_code(ws.receive_message().await), Some(4005));
    })
    .await;
}

/// Reaper: `active` follows the lease freshness — fresh → true, silence or
/// no state → false (sole writer).
#[tokio::test]
#[serial]
async fn reaper_active_follows_freshness() {
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "reaper-target").await;
        let active_of = |db: &sea_orm::DatabaseConnection, id: i64| {
            let db = db.clone();
            async move {
                use pnex_backend::models::_entities::device_registries;
                use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
                device_registries::Entity::find()
                    .filter(device_registries::Column::Id.eq(id))
                    .one(&db)
                    .await
                    .expect("dev")
                    .expect("dev")
                    .active
            }
        };

        use pnex_backend::services::device_liveness::{deactivate_stale, forget_epoch, mark_seen};
        let now = chrono::Utc::now;

        // Created inactive, never seen: the reaper leaves it inactive.
        deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert!(!active_of(&ctx.db, dev.id).await);

        // Fresh sign of life → activated.
        mark_seen(dev.id, now()).await.expect("seen");
        deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert!(active_of(&ctx.db, dev.id).await);

        // Valkey restart (epoch lost) + stale score: deactivation is held
        // back during the restart grace — the fleet does not flap offline.
        forget_epoch().await.expect("epoch");
        let old = now() - chrono::TimeDelta::seconds(60);
        mark_seen(dev.id, old).await.expect("seen");
        let (_, off) = deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert_eq!(off, 0, "restart grace");
        assert!(active_of(&ctx.db, dev.id).await);

        // Grace over (silence TTL elapsed): deactivated, last seen moved to
        // the Postgres cold record, no lease.
        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
        mark_seen(dev.id, old).await.expect("seen");
        let (on, off) = deactivate_stale(&ctx.db, 2).await.expect("reaper");
        assert_eq!((on, off), (0, 1));
        assert!(!active_of(&ctx.db, dev.id).await);
        use pnex_backend::models::_entities::device_states;
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let cold = device_states::Entity::find()
            .filter(device_states::Column::DeviceRegistryId.eq(dev.id))
            .one(&ctx.db)
            .await
            .expect("state")
            .expect("cold record");
        assert_eq!(cold.last_seen_at.timestamp(), old.timestamp());
        let st = state_of(&ctx.db, dev.id).await;
        assert!(!st.connected);
        assert_eq!(st.last_seen.map(|t| t.timestamp()), Some(old.timestamp()));
    })
    .await;
}

/// Liveness of a device as seen by the API.
async fn state_of(
    db: &sea_orm::DatabaseConnection,
    id: i64,
) -> pnex_backend::services::device_liveness::Seen {
    pnex_backend::services::device_liveness::seen_of(db, id)
        .await
        .expect("state")
}

/// Lease claim is atomic across pods (concurrent claims: exactly one
/// winner), refused while a live session holds it, granted when released
/// or stale; touch and release are owner-checked — a superseded session can
/// neither refresh nor release the newer session's lease.
#[tokio::test]
#[serial]
async fn lease_claim_is_atomic_and_release_is_owner_checked() {
    use pnex_backend::services::device_liveness::{claim, release, touch_owned};
    with_app(|server, auth, ctx| async move {
        let dev = create_generic(&server, &auth, "lease-owner").await;
        let db = &ctx.db;

        // Concurrent admissions on a free lease: exactly one wins.
        let mut tasks = Vec::new();
        for i in 0..8 {
            tasks.push(tokio::spawn(async move {
                let session = format!("race-{i}");
                let won = claim(dev.id, &session, 60).await.expect("claim");
                (session, won)
            }));
        }
        let mut winners = Vec::new();
        for t in tasks {
            let (session, won) = t.await.expect("join");
            if won {
                winners.push(session);
            }
        }
        assert_eq!(winners.len(), 1, "one admission only");
        let holder = winners.remove(0);
        assert!(state_of(db, dev.id).await.connected);

        // Fresh live lease: another session is refused, the holder re-claims.
        assert!(!claim(dev.id, "other", 60).await.expect("claim"));
        assert!(claim(dev.id, &holder, 1).await.expect("same session"));

        // Expired lease (1 s TTL elapsed): a new session takes over.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        assert!(claim(dev.id, "newer", 60).await.expect("claim"));
        // The superseded session can neither touch nor release.
        assert!(!touch_owned(dev.id, &holder, 60).await.expect("touch"));
        assert!(!release(db, dev.id, &holder).await.expect("release"));
        assert!(
            state_of(db, dev.id).await.connected,
            "late close of the old session keeps the new lease"
        );
        assert!(!claim(dev.id, &holder, 60).await.expect("claim"));

        // The owner refreshes, then releases: immediate reconnect accepted.
        assert!(touch_owned(dev.id, "newer", 60).await.expect("touch"));
        assert!(release(db, dev.id, "newer").await.expect("release"));
        assert!(!state_of(db, dev.id).await.connected);
        assert!(claim(dev.id, "after-release", 60).await.expect("claim"));
    })
    .await;
}
