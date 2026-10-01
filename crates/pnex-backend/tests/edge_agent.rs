//! Edge agent (D95): `/ws/device` agent session (announce → AgentConfig,
//! free-form Batch → BatchAck, dedup, key discovery, record toggle, quota),
//! enrollment (single-use code, token rotation) and agent guards.
//!
//! Harness of `ws_device.rs` (PG required — TEST_DATABASE_URL).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::telemetry::{self, TelemetryPoint, TelemetrySink};
use pnex_core::frame::{decrypt_frame, encrypt_frame};
use pnex_core::{BatchPoint, DeviceMsg, ServerMsg};
use serial_test::serial;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecSink(Mutex<Vec<TelemetryPoint>>);

impl TelemetrySink for RecSink {
    fn send(&self, point: TelemetryPoint) {
        self.0.lock().expect("sink").push(point);
    }
}

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

struct Agent {
    id: i64,
    org: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn create_device(
    server: &axum_test::TestServer,
    auth: &str,
    device_id: &str,
    predefined: &str,
) -> Agent {
    let org = personal_org(server, auth).await;
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    let dto: serde_json::Value = res.json();
    Agent {
        id: dto["id"].as_i64().expect("id"),
        org,
        device_id: device_id.into(),
        token: dto["device_token"]["token"].as_str().expect("token").into(),
        key: pnex_core::frame::decode_key(
            dto["device_token"]["encryption_key"].as_str().expect("key"),
        )
        .expect("32-byte key"),
    }
}

async fn connect(server: &axum_test::TestServer, a: &Agent) -> axum_test::TestWebSocket {
    server
        .get_websocket(&format!(
            "/ws/device?token={}&device_id={}",
            STANDARD.encode(&a.token),
            STANDARD.encode(&a.device_id),
        ))
        .await
        .into_websocket()
        .await
}

async fn send(ws: &mut axum_test::TestWebSocket, key: &[u8; 32], msg: &DeviceMsg) {
    let plain = serde_json::to_string(msg).expect("json");
    ws.send_text(encrypt_frame(&plain, key)).await;
}

async fn recv(ws: &mut axum_test::TestWebSocket, key: &[u8; 32]) -> ServerMsg {
    let raw = ws.receive_text().await;
    let plain = decrypt_frame(&raw, key).expect("decrypt");
    serde_json::from_str(&plain).expect("ServerMsg")
}

async fn announce(ws: &mut axum_test::TestWebSocket, key: &[u8; 32]) -> ServerMsg {
    send(
        ws,
        key,
        &DeviceMsg::Announce {
            chip: pnex_core::EDGE_AGENT_CHIP.into(),
            board: "x86_64-linux".into(),
            fw: "0.1.0".into(),
            caps: None,
            pins: None,
        },
    )
    .await;
    recv(ws, key).await
}

fn point(seq: u64, key: &str, value: serde_json::Value) -> BatchPoint {
    BatchPoint {
        seq,
        key: key.into(),
        value,
        ts_ms: 1_700_000_000_000 + seq as i64,
        unit: None,
        record: false,
    }
}

async fn batch_ack(
    ws: &mut axum_test::TestWebSocket,
    key: &[u8; 32],
    epoch: &str,
    points: Vec<BatchPoint>,
) -> u64 {
    send(
        ws,
        key,
        &DeviceMsg::Batch {
            epoch: epoch.into(),
            points,
        },
    )
    .await;
    match recv(ws, key).await {
        ServerMsg::BatchAck {
            epoch: e,
            up_to_seq,
        } => {
            assert_eq!(e, epoch);
            up_to_seq
        }
        other => panic!("BatchAck expected, got {other:?}"),
    }
}

async fn keys_of(
    server: &axum_test::TestServer,
    auth: &str,
    a: &Agent,
) -> Vec<pnex_core::agent::AgentKey> {
    let res = server
        .get(&format!("/api/v1/devices/{}/agent-keys", a.id))
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", a.org.to_string())
        .await;
    res.assert_status_ok();
    res.json()
}

/// Waits for the server-side teardown of a closed session (reconnect would
/// otherwise race the anti-clone 4003).
async fn wait_offline(server: &axum_test::TestServer, auth: &str, a: &Agent) {
    for _ in 0..80 {
        let res = server
            .get(&format!("/api/v1/devices/{}/pins", a.id))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", a.org.to_string())
            .await;
        if res.json::<serde_json::Value>()["connected"] == serde_json::json!(false) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("session never released after close()");
}

fn epoch() -> String {
    format!("test-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
#[serial]
async fn agent_free_form_batch_is_discovered_routed_and_deduplicated() {
    with_app(|server, alice, _ctx| async move {
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let a = create_device(&server, &alice, "agent-lab-1", "edge_agent").await;

        let mut ws = connect(&server, &a).await;
        match announce(&mut ws, &a.key).await {
            ServerMsg::AgentConfig {
                max_batch,
                max_keys,
            } => {
                assert_eq!(max_batch, 500);
                assert_eq!(max_keys, 1000);
            }
            other => panic!("AgentConfig expected, got {other:?}"),
        }

        let ep = epoch();
        let mut temp = point(1, "Temp Salon", serde_json::json!(21.5));
        temp.unit = Some("°C".into());
        let mut forced = point(4, "energy", serde_json::json!(12));
        forced.record = true;
        let points = vec![
            temp,
            point(2, "door_open", serde_json::json!(true)),
            point(3, "status", serde_json::json!("running")),
            forced,
        ];
        assert_eq!(batch_ack(&mut ws, &a.key, &ep, points.clone()).await, 4);

        {
            let got = sink.0.lock().expect("sink");
            // Numbers + booleans reach the telemetry pipeline, text does not.
            assert_eq!(got.len(), 3, "{got:?}");
            let temp = got
                .iter()
                .find(|p| p.metric_name == "temp_salon")
                .expect("temp");
            assert_eq!(temp.value, "21.5");
            assert_eq!(temp.ts_source, "device");
            assert_eq!(temp.source_type, "edge_agent");
            assert!(!temp.record, "live-only by default");
            assert_eq!(temp.timestamp.timestamp_millis(), 1_700_000_000_001);
            let door = got
                .iter()
                .find(|p| p.metric_name == "door_open")
                .expect("door");
            assert_eq!(door.value, "1");
            let energy = got
                .iter()
                .find(|p| p.metric_name == "energy")
                .expect("energy");
            assert!(energy.record, "point-level record flag");
        }

        let keys = keys_of(&server, &alice, &a).await;
        let names: Vec<&str> = keys.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(names, vec!["door_open", "energy", "status", "temp_salon"]);
        let temp_key = keys
            .iter()
            .find(|k| k.key == "temp_salon")
            .expect("temp key");
        assert_eq!(temp_key.unit.as_deref(), Some("°C"));
        assert_eq!(temp_key.kind, "number");
        assert!(!temp_key.record_o2);
        assert_eq!(
            keys.iter()
                .find(|k| k.key == "status")
                .expect("status")
                .kind,
            "text"
        );

        // Replay of the same batch (lost ack): acknowledged, not re-ingested.
        assert_eq!(batch_ack(&mut ws, &a.key, &ep, points).await, 4);
        assert_eq!(sink.0.lock().expect("sink").len(), 3);

        // Per-key O2 toggle, applied by the next session.
        let res = server
            .patch(&format!(
                "/api/v1/devices/{}/agent-keys/{}",
                a.id, temp_key.id
            ))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .json(&serde_json::json!({"record_o2": true}))
            .await;
        res.assert_status_ok();
        ws.close().await;
        wait_offline(&server, &alice, &a).await;

        let mut ws = connect(&server, &a).await;
        let _ = announce(&mut ws, &a.key).await;
        assert_eq!(
            batch_ack(
                &mut ws,
                &a.key,
                &ep,
                vec![point(5, "temp salon", serde_json::json!(22))]
            )
            .await,
            5
        );
        let got = sink.0.lock().expect("sink");
        let last = got.last().expect("point");
        assert_eq!(last.metric_name, "temp_salon");
        assert!(last.record, "record_o2 toggle applies");
        drop(got);
        telemetry::reset_sink();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn agent_distinct_keys_quota_drops_new_keys_only() {
    with_app(|server, alice, _ctx| async move {
        let sink = Arc::new(RecSink::default());
        telemetry::set_sink(sink.clone());
        let a = create_device(&server, &alice, "agent-quota", "edge_agent").await;
        let res = server
            .patch(&format!("/api/v1/devices/{}/agent", a.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .json(&serde_json::json!({"max_keys": 2}))
            .await;
        res.assert_status_ok();
        let res = server
            .patch(&format!("/api/v1/devices/{}/agent", a.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .json(&serde_json::json!({"max_keys": 0}))
            .await;
        res.assert_status_bad_request();

        let mut ws = connect(&server, &a).await;
        match announce(&mut ws, &a.key).await {
            ServerMsg::AgentConfig { max_keys, .. } => assert_eq!(max_keys, 2),
            other => panic!("AgentConfig expected, got {other:?}"),
        }
        let ep = epoch();
        let up = batch_ack(
            &mut ws,
            &a.key,
            &ep,
            vec![
                point(1, "a", serde_json::json!(1)),
                point(2, "b", serde_json::json!(2)),
                point(3, "c", serde_json::json!(3)),
                point(4, "a", serde_json::json!(4)),
            ],
        )
        .await;
        assert_eq!(up, 4, "dropped points are still acknowledged");
        let names: Vec<String> = sink
            .0
            .lock()
            .expect("sink")
            .iter()
            .map(|p| p.metric_name.clone())
            .collect();
        assert_eq!(names, vec!["a", "b", "a"]);
        assert_eq!(keys_of(&server, &alice, &a).await.len(), 2);
        telemetry::reset_sink();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn enrollment_is_single_use_and_rotates_credentials() {
    with_app(|server, alice, ctx| async move {
        let a = create_device(&server, &alice, "agent-enroll", "edge_agent").await;
        let res = server
            .post(&format!("/api/v1/devices/{}/agent-enrollment", a.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let created: pnex_core::agent::AgentEnrollmentCreated = res.json();
        assert_eq!(created.code.len(), 14);

        // Anonymous enrollment (the agent has no user session).
        let res = server
            .post("/api/v1/agent/enroll")
            .json(&serde_json::json!({
                "code": created.code.to_lowercase(),
                "hostname": "lab-pc",
                "os": "linux",
                "arch": "x86_64",
                "agent_version": "0.1.0"
            }))
            .await;
        res.assert_status_ok();
        let creds: pnex_core::agent::AgentEnrollResponse = res.json();
        assert_eq!(creds.device_id, "agent-enroll");
        assert_eq!(creds.ws_path, "/ws/device");
        assert_ne!(creds.token, a.token, "token rotated");

        // Single use.
        let res = server
            .post("/api/v1/agent/enroll")
            .json(&serde_json::json!({"code": created.code}))
            .await;
        res.assert_status_bad_request();
        assert!(res.text().contains("agent-enroll-code-invalid"));

        // Old credentials are dead, the new ones work.
        let mut old = connect(&server, &a).await;
        match old.receive_message().await {
            axum_test::WsMessage::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 4001),
            other => panic!("close 4001 expected, got {other:?}"),
        }
        let fresh = Agent {
            id: a.id,
            org: a.org,
            device_id: creds.device_id.clone(),
            token: creds.token.clone(),
            key: pnex_core::frame::decode_key(&creds.encryption_key).expect("key"),
        };
        let mut ws = connect(&server, &fresh).await;
        assert!(matches!(
            announce(&mut ws, &fresh.key).await,
            ServerMsg::AgentConfig { .. }
        ));
        ws.close().await;

        // Machine facts surface in the agent overview.
        let info: pnex_core::agent::AgentInfo = server
            .get(&format!("/api/v1/devices/{}/agent", a.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .await
            .json();
        assert_eq!(info.hostname.as_deref(), Some("lab-pc"));
        assert_eq!(info.agent_version.as_deref(), Some("0.1.0"));

        // Expired code.
        let res = server
            .post(&format!("/api/v1/devices/{}/agent-enrollment", a.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .await;
        let created: pnex_core::agent::AgentEnrollmentCreated = res.json();
        use sea_orm::{ConnectionTrait, Statement};
        ctx.db
            .execute_raw(Statement::from_string(
                ctx.db.get_database_backend(),
                "UPDATE agent_enrollments SET expires_at = expires_at - interval '1 hour' WHERE used_at IS NULL",
            ))
            .await
            .expect("expire code");
        let res = server
            .post("/api/v1/agent/enroll")
            .json(&serde_json::json!({"code": created.code}))
            .await;
        res.assert_status_bad_request();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn agent_guards_reject_firmware_actions_and_non_agents() {
    with_app(|server, alice, _ctx| async move {
        let a = create_device(&server, &alice, "agent-guard", "edge_agent").await;
        let res = server
            .post("/api/v1/build-firmware")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", a.org.to_string())
            .json(&serde_json::json!({
                "device_id": a.device_id,
                "predefined_device_name": "edge_agent",
                "wifi_ssid": "x",
                "wifi_password": "y",
                "pnex_host": "h"
            }))
            .await;
        res.assert_status_bad_request();
        assert!(
            res.text().contains("agent-unsupported-action"),
            "{}",
            res.text()
        );

        let g = create_device(&server, &alice, "esp-guard", "generic_esp8266").await;
        let res = server
            .post(&format!("/api/v1/devices/{}/agent-enrollment", g.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", g.org.to_string())
            .await;
        res.assert_status_bad_request();
        assert!(res.text().contains("agent-not-an-agent"));

        // Unknown download targets never touch the filesystem.
        let res = server
            .get("/api/v1/agent/download/..%2F..%2Fetc%2Fpasswd")
            .await;
        res.assert_status_not_found();
        let res = server.get("/api/v1/agent/install.sh").await;
        res.assert_status_ok();
        assert!(res.text().starts_with("#!/bin/sh"));

        // Shipped files are streamed from disk, byte-exact, with their length.
        let dist = std::env::temp_dir().join(format!("pnex-agent-dist-{}", std::process::id()));
        std::fs::create_dir_all(&dist).unwrap();
        let sums: String = (0..5000)
            .map(|i| format!("{i:064x}  pnex-agent-{i}\n"))
            .collect();
        std::fs::write(dist.join("SHA256SUMS"), &sums).unwrap();
        unsafe { std::env::set_var("PNEX_AGENT_DIST_DIR", &dist) };
        let res = server.get("/api/v1/agent/download/SHA256SUMS").await;
        unsafe { std::env::remove_var("PNEX_AGENT_DIST_DIR") };
        res.assert_status_ok();
        assert_eq!(
            res.header("content-length").to_str().unwrap(),
            sums.len().to_string()
        );
        assert_eq!(res.text(), sums, "streamed body must be byte-exact");
        let _ = std::fs::remove_dir_all(&dist);
    })
    .await;
}
