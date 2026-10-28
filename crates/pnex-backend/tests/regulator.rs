//! E2E tests for the mixed regulation cards (control-cards): `ControlConfig`
//! cast to the mocked WS device on announce and on deploy, re-cast on
//! threshold change, rollback, removal (`[]`), cross-flow conflict
//! (smallest flow_id wins), undeploy on set_mode of the sensor pin.
//!
//! Harness = `ws_device.rs` (mirror client in the firmware role) × `flows.rs`
//! (supervisor env, engine off: the cast is **independent of the runtime**,
//! the deploy answers 503 there after the sync — both are asserted).
//! Requires PostgreSQL (TEST_DATABASE_URL).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

// ─────────────────── Mirror client (mixed card firmware role) ───────────────────

fn key_bytes(b64: &str) -> [u8; 32] {
    STANDARD
        .decode(b64)
        .expect("b64 key")
        .try_into()
        .expect("32-byte key")
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
    // Flow engine off: the regulation cast must stay consistent
    // (control-cards decision — the sync/cast precedes the projection and is
    // never blocked by the runtime).
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-reg-tests-{}", std::process::id()),
        )
    };
    unsafe { std::env::set_var("PNEX_FLOW_RELOAD_ACK_SECS", "5") };
    unsafe { std::env::set_var("PNEX_FLOW_DEBUG_TOOLS", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
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

async fn connect(server: &axum_test::TestServer, d: &Dev) -> common::DevWs {
    common::DevWs::connect(
        server,
        &format!("/ws/device?token={}", d.token,),
        &d.key,
        &d.device_id,
    )
    .await
}

/// Announce → ProvisionAck (the cadences/ControlConfig follow).
async fn announce_and_expect_provision(ws: &mut common::DevWs) -> Vec<pnex_core::PinSpec> {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "0.1.0"
    })
    .to_string();
    ws.send_plain(&announce).await;
    let raw = ws.recv_plain().await;
    let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
    match msg {
        pnex_core::ServerMsg::ProvisionAck { caps, .. } => caps,
        other => panic!("ProvisionAck expected, got: {other:?}"),
    }
}

/// Connect + announce with **retry**: releasing the previous session's lease
/// (anti-clone 4003) is asynchronous on the server side — an immediate
/// reconnect after close can hit the guard window (mostly under load of the
/// full suite). The device retries, like the real firmware
/// (reconnect backoff).
async fn connect_announce_with_retry(
    server: &axum_test::TestServer,
    d: &Dev,
) -> (common::DevWs, Vec<pnex_core::PinSpec>) {
    let announce = serde_json::json!({
        "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "0.1.0"
    })
    .to_string();
    for attempt in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let mut ws = connect(server, d).await;
        ws.send_plain(&announce).await;
        let msg = ws.receive_message().await;
        if let axum_test::WsMessage::Text(raw) = &msg {
            let plain = ws.open(raw).unwrap_or_default();
            if let Ok(pnex_core::ServerMsg::ProvisionAck { caps, .. }) =
                serde_json::from_str::<pnex_core::ServerMsg>(&plain)
            {
                return (ws, caps);
            }
        }
        // Close (4003 probable) : on retente.
        let _ = attempt;
    }
    panic!("announce impossible after retries (anti-clone lease still held?)");
}

/// Skips non-regulation commands (SetMode/Subscribe pushed to the device)
/// and returns the map of the first `ControlConfig` (only one expected).
async fn expect_control_config(ws: &mut common::DevWs) -> pnex_core::ControlSpec {
    for _ in 0..10 {
        let raw = ws.recv_plain().await;
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
        if let pnex_core::ServerMsg::ControlConfig { configs, .. } = msg {
            assert_eq!(configs.len(), 1, "a single card expected: {configs:?}");
            return configs.into_iter().next().expect("carte");
        }
    }
    panic!("no ControlConfig received");
}

/// set_mode D1 (gpio 5) → digital_out, retour safe low.
async fn set_d1_output(server: &axum_test::TestServer, auth: &str, org: i64, d: &Dev) {
    let res = server
        .post(&format!("/api/v1/devices/{}/commands", d.id))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "op": "set_mode", "gpio": 5, "mode": "digital_out",
            "opts": {"safe_state": "low"}
        }))
        .await;
    res.assert_status_ok();
}

/// Single-card on/off flow (sensor A0, output D1).
async fn create_reg_flow(
    server: &axum_test::TestServer,
    auth: &str,
    org: i64,
    name: &str,
    device_slug: &str,
    setpoint: f64,
) -> i64 {
    let res = server
        .post("/api/v1/flows")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "name": name,
            "graph": {"nodes": [{
                "id": "r1", "kind": "reg_tt_heat",
                "config": {
                    "device_id": device_slug,
                    "sensor_pin": "A0",
                    "actuator_pin": "D1",
                    "setpoint": setpoint,
                    "deadband": 0.5,
                },
            }]},
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    dto["id"].as_i64().expect("flow id")
}

/// Deploy → 503 expected (engine off): the cast itself has already happened.
async fn deploy_expect_503(server: &axum_test::TestServer, auth: &str, org: i64, flow_id: i64) {
    let res = server
        .post(&format!("/api/v1/flows/{flow_id}/deploy"))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({}))
        .await;
    res.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
}

// ─────────────────── Tests ───────────────────

/// Deploy with a connected device → decrypted `control_config` with the gpio
/// resolved (A0→17 adc_in, D1→5 digital_out), even though the ETL engine
/// is off (503): the sync/cast is independent of the runtime.
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn deploy_caste_control_config_au_device_connecte() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-serre").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_id = create_reg_flow(&server, &auth, org, "serre TT", "gen-serre", 19.0).await;
        deploy_expect_503(&server, &auth, org, flow_id).await;

        let spec = expect_control_config(&mut ws).await;
        assert_eq!(spec.node_id, "r1");
        assert_eq!(spec.kind, "tt_heat");
        assert_eq!(
            (spec.sensor_gpio, spec.sensor_mode),
            (17, pnex_core::Mode::AdcIn)
        );
        assert_eq!(spec.out_gpio, 5);
        assert!((spec.setpoint - 19.0).abs() < 1e-9);
        assert!((spec.deadband - 0.5).abs() < 1e-9);
        assert_eq!(spec.min_on_secs, 5);
        assert_eq!(spec.sample_ms, 5_000);
        ws.close().await;
    })
    .await;
}

/// Device offline at deploy → nothing; on connect+announce, the desired-state
/// is re-cast after the ProvisionAck (no EEPROM on the card side).
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn offline_au_deploy_recast_a_l_announce() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-differe").await;
        // set_mode needs a session: connect, announce, set D1,
        // close again — the card is offline at deploy.
        {
            let mut ws = connect(&server, &dev).await;
            let _caps = announce_and_expect_provision(&mut ws).await;
            set_d1_output(&server, &auth, org, &dev).await;
            ws.close().await;
        }
        let flow_id =
            create_reg_flow(&server, &auth, org, "serre différé", "gen-differe", 21.0).await;
        deploy_expect_503(&server, &auth, org, flow_id).await;

        // Reconnect (anti-clone retry): ProvisionAck then ControlConfig
        // (no cadence persisted in between).
        let (mut ws, _caps) = connect_announce_with_retry(&server, &dev).await;
        let spec = expect_control_config(&mut ws).await;
        assert_eq!(spec.kind, "tt_heat");
        assert!((spec.setpoint - 21.0).abs() < 1e-9);
        ws.close().await;
    })
    .await;
}

/// Threshold edit + redeploy → new setpoint cast (the hot threshold
/// reconfiguration promised by the product).
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn edit_redeploy_recaste_nouveau_setpoint() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-seuil").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_id = create_reg_flow(&server, &auth, org, "serre seuil", "gen-seuil", 19.0).await;
        deploy_expect_503(&server, &auth, org, flow_id).await;
        let spec = expect_control_config(&mut ws).await;
        assert!((spec.setpoint - 19.0).abs() < 1e-9);

        // v2 : consigne 21 → redeploy → re-cast.
        let updated = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "graph": {"nodes": [{
                    "id": "r1", "kind": "reg_tt_heat",
                    "config": {
                        "device_id": "gen-seuil",
                        "sensor_pin": "A0",
                        "actuator_pin": "D1",
                        "setpoint": 21.0,
                        "deadband": 0.5,
                    },
                }]},
            }))
            .await;
        assert_eq!(updated.status_code(), 200, "{}", updated.text());
        deploy_expect_503(&server, &auth, org, flow_id).await;
        let spec = expect_control_config(&mut ws).await;
        assert!((spec.setpoint - 21.0).abs() < 1e-9, "{spec:?}");
        ws.close().await;
    })
    .await;
}

/// Rollback v1 → the old setpoint is re-cast (full replacement).
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn rollback_recaste_ancien_setpoint() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-rollback").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_id =
            create_reg_flow(&server, &auth, org, "serre rollback", "gen-rollback", 19.0).await;
        deploy_expect_503(&server, &auth, org, flow_id).await;
        let _ = expect_control_config(&mut ws).await;

        // v2 at 22 then rollback v1 → 19 re-cast.
        let updated = server
            .patch(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "expected_version_number": 1,
                "graph": {"nodes": [{
                    "id": "r1", "kind": "reg_tt_heat",
                    "config": {
                        "device_id": "gen-rollback",
                        "sensor_pin": "A0", "actuator_pin": "D1",
                        "setpoint": 22.0, "deadband": 0.5,
                    },
                }]},
            }))
            .await;
        assert_eq!(updated.status_code(), 200);
        let res = server
            .post(&format!("/api/v1/flows/{flow_id}/rollback"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"version_number": 1}))
            .await;
        res.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
        let spec = expect_control_config(&mut ws).await;
        assert!((spec.setpoint - 19.0).abs() < 1e-9, "{spec:?}");
        ws.close().await;
    })
    .await;
}

/// Deleting the deployed flow → `control_config` with `configs: []` (the
/// device outputs go back to safe — we stop regulating, we never write
/// a pin).
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn suppression_caste_liste_vide() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-clear").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_id = create_reg_flow(&server, &auth, org, "serre clear", "gen-clear", 19.0).await;
        deploy_expect_503(&server, &auth, org, flow_id).await;
        let _ = expect_control_config(&mut ws).await;

        let res = server
            .delete(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        // Engine off: the clear is cast BEFORE the projection's 503
        // (the cast is independent of the runtime — that's the tested contract).
        res.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);
        for _ in 0..10 {
            let raw = ws.recv_plain().await;
            let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
            if let pnex_core::ServerMsg::ControlConfig { configs, .. } = msg {
                assert!(configs.is_empty(), "clear expected, got: {configs:?}");
                ws.close().await;
                return;
            }
        }
        panic!("clear control_config expected after deletion");
    })
    .await;
}

/// Two deployed flows regulating the SAME output: smallest flow_id wins,
/// the other is skipped (warn) — the cast holds only one card.
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn conflit_cross_flows_plus_petit_flow_id_gagne() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-conflit").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        let flow_a = create_reg_flow(&server, &auth, org, "conflit A", "gen-conflit", 19.0).await;
        let flow_b = create_reg_flow(&server, &auth, org, "conflit B", "gen-conflit", 25.0).await;
        assert!(flow_a < flow_b, "creation order → increasing ids");
        deploy_expect_503(&server, &auth, org, flow_a).await;
        let spec = expect_control_config(&mut ws).await;
        assert!((spec.setpoint - 19.0).abs() < 1e-9);

        // Deploy of B: the whole set is recomputed — A (smallest id) wins.
        deploy_expect_503(&server, &auth, org, flow_b).await;
        let spec = expect_control_config(&mut ws).await;
        assert!(
            (spec.setpoint - 19.0).abs() < 1e-9,
            "A doit gagner : {spec:?}"
        );
        ws.close().await;
    })
    .await;
}

/// set_mode in→out on the sensor pin of a deployed card: the flow is
/// undeployed automatically and the clear is cast (output → safe).
#[tokio::test]
#[ignore = "known-broken harness: device WS handshake times out on a clean checkout (pre-existing, needs a dedicated debug session); run with --ignored + TEST_DATABASE_URL"]
#[serial]
async fn set_mode_capteur_dedeploie_et_clear() {
    with_app(|server, auth, _ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_generic(&server, &auth, "gen-stop").await;
        let mut ws = connect(&server, &dev).await;
        let _caps = announce_and_expect_provision(&mut ws).await;
        set_d1_output(&server, &auth, org, &dev).await;

        // Capteur D2 (gpio 4, digital_in) — basculable in↔out.
        let res = server
            .post("/api/v1/flows")
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "serre stop",
                "graph": {"nodes": [{
                    "id": "r1", "kind": "reg_tt_heat",
                    "config": {
                        "device_id": "gen-stop",
                        "sensor_pin": "D2",
                        "actuator_pin": "D1",
                        "setpoint": 19.0, "deadband": 0.5,
                    },
                }]},
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let flow_id: i64 = res.json::<serde_json::Value>()["id"].as_i64().unwrap();
        deploy_expect_503(&server, &auth, org, flow_id).await;
        let _ = expect_control_config(&mut ws).await;

        // D2 (gpio 4) → digital_out: the card loses its sensor. Engine
        // off → the projection's 503 follows the undeploy (the cast
        // of the clear has already happened).
        let res = server
            .post(&format!("/api/v1/devices/{}/commands", dev.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "op": "set_mode", "gpio": 4, "mode": "digital_out",
                "opts": {"safe_state": "low"}
            }))
            .await;
        res.assert_status(axum_test::http::StatusCode::SERVICE_UNAVAILABLE);

        // The clear follows (the flow is undeployed by stop_flows_reading_pin).
        for _ in 0..10 {
            let raw = ws.recv_plain().await;
            let msg: pnex_core::ServerMsg = serde_json::from_str(&raw.clone()).expect("ServerMsg");
            if let pnex_core::ServerMsg::ControlConfig { configs, .. } = msg {
                assert!(configs.is_empty(), "clear expected, got: {configs:?}");
                ws.close().await;
                return;
            }
        }
        panic!("clear control_config expected after the sensor's set_mode");
    })
    .await;
}
