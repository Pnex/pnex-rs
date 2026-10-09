//! Camera & video (camera-video.md D73–D79): `/ws/camera` uplink, live
//! viewer `/ws/camera/live`, settings API, snapshot, and segments written
//! through `/internal/flow/video-segment`.
//!
//! Same harness as `ws_device.rs` (PG required — TEST_DATABASE_URL).

mod common;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_core::camera::FrameHeader;
use serial_test::serial;
use std::time::Duration;

struct Dev {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
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
    org: i64,
    device_id: &str,
) -> Dev {
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
        key: STANDARD
            .decode(dto["device_token"]["encryption_key"].as_str().expect("key"))
            .expect("b64 key")
            .try_into()
            .expect("32-byte key"),
    }
}

/// Reads decrypted server messages until a `CameraConfig` shows up.
async fn next_camera_config(ws: &mut common::DevWs) -> (bool, u8) {
    for _ in 0..10 {
        let raw = tokio::time::timeout(Duration::from_secs(5), ws.recv_plain())
            .await
            .expect("server message");
        let msg: pnex_core::ServerMsg = serde_json::from_str(&raw).expect("ServerMsg");
        if let pnex_core::ServerMsg::CameraConfig { enabled, fps, .. } = msg {
            return (enabled, fps);
        }
    }
    panic!("no CameraConfig received");
}

/// `enabled` of the first `CameraConfig` carrying `fps` (earlier configs,
/// e.g. a heartbeat re-push, are skipped).
async fn config_with_fps(ws: &mut common::DevWs, fps: u8) -> bool {
    for _ in 0..5 {
        let (enabled, got) = next_camera_config(ws).await;
        if got == fps {
            return enabled;
        }
    }
    panic!("no CameraConfig with fps {fps}");
}

/// Removes every viewer presence field of a camera (all pods): database
/// ids are recycled between test processes, and a test runtime that ends
/// before the viewer cleanup task runs leaves its field until expiry.
async fn clear_viewer_presence(kv: &mut redis::aio::ConnectionManager, device: i64) {
    use redis::AsyncCommands;
    let all: std::collections::HashMap<String, String> = kv
        .hgetall(pnex_backend::services::camera::VIEWERS_KEY)
        .await
        .unwrap();
    let prefix = format!("{device}|");
    for field in all.keys().filter(|f| f.starts_with(&prefix)) {
        let _: () = kv
            .hdel(pnex_backend::services::camera::VIEWERS_KEY, field)
            .await
            .unwrap();
    }
}

fn frame(seq: u32) -> Vec<u8> {
    let mut plain = FrameHeader {
        seq,
        uptime_ms: seq * 100,
        width: 320,
        height: 240,
    }
    .encode()
    .to_vec();
    plain.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, seq as u8, 0xFF, 0xD9]);
    plain
}

/// Announce with the `camera` cap → settings row + CameraConfig (disabled,
/// on_demand) ; a live viewer wakes the camera ; uplink frames reach the
/// viewer as raw JPEG ; snapshot serves the last frame ; settings PATCH is
/// validated and pushed live.
#[tokio::test]
#[serial]
async fn camera_uplink_live_and_settings() {
    with_app(|server, alice, _ctx| async move {
        let org = personal_org(&server, &alice).await;
        let dev = create_device(&server, &alice, org, "cam-one").await;
        // When a previous test enabled the cluster state, stale viewer
        // presence of a recycled device id must not wake the camera.
        if let Ok(url) = std::env::var("PNEX_TEST_VALKEY_URL") {
            let mut kv =
                redis::aio::ConnectionManager::new(redis::Client::open(url.as_str()).unwrap())
                    .await
                    .unwrap();
            clear_viewer_presence(&mut kv, dev.id).await;
        }

        let mut ctl = common::DevWs::connect(
            &server,
            &format!("/ws/device?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "7",
            "caps": [{"id": "camera", "family": "video"}]
        })
        .to_string();
        ctl.send_plain(&announce).await;
        assert_eq!(next_camera_config(&mut ctl).await, (false, 5));

        // Listed as a camera.
        let list: serde_json::Value = server
            .get("/api/v1/cameras")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(list[0]["device_id"], "cam-one");
        assert_eq!(list[0]["settings"]["capture_mode"], "on_demand");

        // No frame yet → 404 with the machine code.
        let snap = server
            .get(&format!("/api/v1/cameras/{}/snapshot", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        snap.assert_status(axum_test::http::StatusCode::NOT_FOUND);

        // A live viewer wakes the on_demand camera.
        let mut viewer = server
            .get_websocket(&format!(
                "/ws/camera/live?ticket={}&device={}",
                common::ws_ticket(&server, &alice, org).await,
                dev.id
            ))
            .await
            .into_websocket()
            .await;
        assert_eq!(next_camera_config(&mut ctl).await, (true, 5));

        // Uplink: an encrypted frame reaches the viewer as raw JPEG.
        let mut cam = common::DevWs::connect_binary(
            &server,
            &format!("/ws/camera?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        cam.send_sealed_bytes(&frame(1)).await;
        let got = tokio::time::timeout(Duration::from_secs(5), viewer.receive_bytes())
            .await
            .expect("live frame");
        assert_eq!(&got[..], &frame(1)[16..]);

        // A forged frame is dropped, never forwarded (the link stays up).
        cam.send_message(axum_test::WsMessage::Binary(vec![7u8; 64].into()))
            .await;

        let snap = server
            .get(&format!("/api/v1/cameras/{}/snapshot", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        snap.assert_status_ok();
        assert_eq!(&snap.as_bytes()[..], &frame(1)[16..]);

        // A second uplink for the same camera is refused (anti-clone).
        let mut clone = common::DevWs::connect_binary(
            &server,
            &format!("/ws/camera?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        match clone.receive_message().await {
            axum_test::WsMessage::Close(Some(f)) => assert_eq!(u16::from(f.code), 4003),
            other => panic!("expected close 4003, got {other:?}"),
        }

        // Settings: invalid → field errors ; valid → pushed live.
        let bad = server
            .patch(&format!("/api/v1/cameras/{}/settings", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"fps": 99, "framesize": "8k"}))
            .await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = bad.json();
        assert_eq!(body["fps"], "range:1..25");
        assert_eq!(body["framesize"], "invalid");
        let ok = server
            .patch(&format!("/api/v1/cameras/{}/settings", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({"fps": 10, "capture_mode": "continuous"}))
            .await;
        ok.assert_status_ok();
        assert_eq!(next_camera_config(&mut ctl).await, (true, 10));

        cam.close().await;
        viewer.close().await;
        ctl.close().await;
    })
    .await;
}

/// Segments: internal upload (flow token) → listed, served, deleted.
#[tokio::test]
#[serial]
async fn segments_written_listed_and_deleted() {
    with_app(|server, alice, _ctx| async move {
        let dir = tempfile::tempdir().expect("tmp");
        unsafe { std::env::set_var("PNEX_MEDIA_DIR", dir.path()) };
        unsafe { std::env::set_var("PNEX_MEDIA_BACKEND", "fs") };
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-cam") };
        let org = personal_org(&server, &alice).await;
        let dev = create_device(&server, &alice, org, "cam-rec").await;

        let mut w = pnex_core::avi::AviWriter::new(320, 240);
        w.push(vec![0xFF, 0xD8, 1, 2, 0xFF, 0xD9]);
        w.push(vec![0xFF, 0xD8, 3, 4, 0xFF, 0xD9]);
        let avi = w.finish(2.0);
        let path = format!(
            "/internal/flow/video-segment?org_id={org}&device_id=cam-rec&node_id=n1&stream=front\
             &started_ms=1790000000000&ended_ms=1790000001000&frames=2&width=320&height=240&retention_days=7"
        );
        let denied = server.post(&path).bytes(avi.clone().into()).await;
        denied.assert_status(axum_test::http::StatusCode::UNAUTHORIZED);
        let res = server
            .post(&path)
            .add_header("x-pnex-flow-token", "tok-cam")
            .bytes(avi.clone().into())
            .await;
        res.assert_status_ok();
        let seg: serde_json::Value = res.json();
        assert_eq!(seg["device"], dev.id);
        assert!(seg["expires_at"].is_string());

        let list: serde_json::Value = server
            .get(&format!("/api/v1/cameras/segments?device={}", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(list["count"], 1);
        let id = list["results"][0]["id"].as_str().expect("id").to_string();

        let content = server
            .get(&format!("/api/v1/cameras/segments/{id}/content"))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        content.assert_status_ok();
        let idx = pnex_core::avi::read_avi(&content.as_bytes()).expect("avi");
        assert_eq!(idx.frames.len(), 2);

        server
            .delete(&format!("/api/v1/cameras/segments/{id}"))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .assert_status(axum_test::http::StatusCode::NO_CONTENT);
        let list: serde_json::Value = server
            .get("/api/v1/cameras/segments")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(list["count"], 0);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
        unsafe { std::env::remove_var("PNEX_MEDIA_DIR") };
        unsafe { std::env::remove_var("PNEX_MEDIA_BACKEND") };
    })
    .await;
}

/// Query-safe RFC 3339 (`:` and `+` escaped).
fn enc(ts: &str) -> String {
    ts.replace(':', "%3A").replace('+', "%2B")
}

#[tokio::test]
#[serial]
async fn recording_timeline_export_and_day_delete() {
    with_app(|server, alice, _ctx| async move {
        let dir = tempfile::tempdir().expect("tmp");
        unsafe { std::env::set_var("PNEX_MEDIA_DIR", dir.path()) };
        unsafe { std::env::set_var("PNEX_MEDIA_BACKEND", "fs") };
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-cam") };
        let org = personal_org(&server, &alice).await;
        let dev = create_device(&server, &alice, org, "cam-tl").await;
        let auth = format!("Bearer {alice}");

        // Two back-to-back segments (node n1) + one overlapping the second
        // (a second recorder, node n2) — the export must skip the overlap.
        let base = 1_790_000_000_000i64;
        for (node, start, end, frames) in [
            ("n1", base, base + 60_000, 3u8),
            ("n1", base + 60_100, base + 120_000, 2),
            ("n2", base + 60_300, base + 120_500, 4),
        ] {
            let mut w = pnex_core::avi::AviWriter::new(320, 240);
            for i in 0..frames {
                w.push(vec![0xFF, 0xD8, i, 0xFF, 0xD9]);
            }
            server
                .post(&format!(
                    "/internal/flow/video-segment?org_id={org}&device_id=cam-tl&node_id={node}&stream=front\
                     &started_ms={start}&ended_ms={end}&frames={frames}&width=320&height=240&retention_days=7"
                ))
                .add_header("x-pnex-flow-token", "tok-cam")
                .bytes(w.finish(1.0).into())
                .await
                .assert_status_ok();
        }
        let from = chrono::DateTime::from_timestamp_millis(base - 3_600_000).unwrap().to_rfc3339();
        let to = chrono::DateTime::from_timestamp_millis(base + 3_600_000).unwrap().to_rfc3339();
        let range = format!(
            "from={}&to={}",
            enc(&from),
            enc(&to)
        );

        let tl: serde_json::Value = server
            .get(&format!("/api/v1/cameras/{}/recordings?{range}", dev.id))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(tl["segments"].as_array().expect("segments").len(), 3);
        assert_eq!(tl["truncated"], false);

        let export = server
            .get(&format!("/api/v1/cameras/{}/recordings/export?{range}", dev.id))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await;
        export.assert_status_ok();
        let idx = pnex_core::avi::read_avi(&export.as_bytes()).expect("avi");
        assert_eq!(idx.frames.len(), 5, "3 + 2 frames, the overlapping n2 segment skipped");
        assert_eq!((idx.width, idx.height), (320, 240));

        // Invalid window → coded 400.
        let bad = server
            .get(&format!("/api/v1/cameras/{}/recordings?from={}&to={}", dev.id, enc(&to), enc(&from)))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await;
        bad.assert_status(axum_test::http::StatusCode::BAD_REQUEST);
        assert!(bad.text().contains(pnex_core::err_codes::CAMERA_RANGE_INVALID));

        let deleted: serde_json::Value = server
            .delete(&format!("/api/v1/cameras/{}/recordings?{range}", dev.id))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(deleted["deleted"], 3);
        let empty = server
            .get(&format!("/api/v1/cameras/{}/recordings/export?{range}", dev.id))
            .add_header("Authorization", auth.clone())
            .add_header("X-Org-Id", org.to_string())
            .await;
        empty.assert_status(axum_test::http::StatusCode::NOT_FOUND);
        assert!(empty.text().contains(pnex_core::err_codes::CAMERA_EXPORT_EMPTY));
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
        unsafe { std::env::remove_var("PNEX_MEDIA_DIR") };
        unsafe { std::env::remove_var("PNEX_MEDIA_BACKEND") };
    })
    .await;
}

/// Cluster (several API pods): another pod is simulated through Valkey
/// (`PNEX_TEST_VALKEY_URL`, skipped otherwise) — its uplink claim, its
/// viewers, its flow demand and the frames it publishes on the bus.
/// Checks: cluster-wide demand at announce and on re-push, cross-pod
/// anti-clone (4003), list fields, snapshot from the bus, live relay of a
/// remote uplink, and token revalidation of the uplink (4005).
#[tokio::test]
#[serial]
async fn camera_cluster_state_crosses_pods() {
    let Some(url) = std::env::var("PNEX_TEST_VALKEY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("PNEX_TEST_VALKEY_URL unset — camera cluster test skipped");
        return;
    };
    unsafe { std::env::set_var("PNEX_TEST_APP_VALKEY_URL", &url) };
    with_app(|server, alice, ctx| async move {
        use pnex_backend::services::camera;
        use redis::AsyncCommands;
        let mut kv = redis::aio::ConnectionManager::new(redis::Client::open(url.as_str()).unwrap())
            .await
            .unwrap();
        let org = personal_org(&server, &alice).await;
        let slug = format!("cam-cluster-{}", std::process::id());
        let dev = create_device(&server, &alice, org, &slug).await;
        // Device ids are recycled: drop what earlier tests left in the hub.
        camera::forget_local(dev.id);
        clear_viewer_presence(&mut kv, dev.id).await;
        let other = "test-other-pod";
        let viewer_field = camera::viewer_field(dev.id, other);
        let flow_field = format!("test-other-worker-{}", std::process::id());
        let _: () = kv.del(camera::uplink_key(dev.id)).await.unwrap();
        let _: () = kv.hdel(camera::VIEWERS_KEY, &viewer_field).await.unwrap();
        let _: () = kv.hdel(camera::FLOW_KEY, &flow_field).await.unwrap();
        let future = chrono::Utc::now().timestamp_millis() + 60_000;

        // Two viewers on another pod: the announce enables the camera.
        let _: () = kv
            .hset(camera::VIEWERS_KEY, &viewer_field, format!("2|{future}"))
            .await
            .unwrap();
        let mut ctl = common::DevWs::connect(
            &server,
            &format!("/ws/device?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        let announce = serde_json::json!({
            "t": "announce", "chip": "esp8266", "board": "nodemcu", "fw": "7",
            "caps": [{"id": "camera", "family": "video"}]
        })
        .to_string();
        ctl.send_plain(&announce).await;
        assert_eq!(next_camera_config(&mut ctl).await, (true, 5));

        // Remote viewers gone, a flow worker of another pod wants it.
        let _: () = kv.hdel(camera::VIEWERS_KEY, &viewer_field).await.unwrap();
        let demand = camera::FlowDemandEntry {
            exp: future,
            c: vec![(org, slug.clone())],
        };
        let _: () = kv
            .hset(
                camera::FLOW_KEY,
                &flow_field,
                serde_json::to_string(&demand).unwrap(),
            )
            .await
            .unwrap();
        let patch = |fps: u8| {
            server
                .patch(&format!("/api/v1/cameras/{}/settings", dev.id))
                .add_header("Authorization", format!("Bearer {alice}"))
                .add_header("X-Org-Id", org.to_string())
                .json(&serde_json::json!({ "fps": fps }))
        };
        patch(6).await.assert_status_ok();
        assert!(config_with_fps(&mut ctl, 6).await);
        // An expired demand field no longer counts.
        let expired = camera::FlowDemandEntry {
            exp: 1,
            c: vec![(org, slug.clone())],
        };
        let _: () = kv
            .hset(
                camera::FLOW_KEY,
                &flow_field,
                serde_json::to_string(&expired).unwrap(),
            )
            .await
            .unwrap();
        patch(7).await.assert_status_ok();
        assert!(!config_with_fps(&mut ctl, 7).await);
        let _: () = kv.hdel(camera::FLOW_KEY, &flow_field).await.unwrap();

        // The uplink is held by another pod: a local uplink is refused.
        let _: () = kv
            .set_ex(camera::uplink_key(dev.id), format!("{other}|s1"), 30)
            .await
            .unwrap();
        let mut clone = common::DevWs::connect_binary(
            &server,
            &format!("/ws/camera?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        match clone.receive_message().await {
            axum_test::WsMessage::Close(Some(f)) => assert_eq!(u16::from(f.code), 4003),
            other => panic!("expected close 4003, got {other:?}"),
        }

        // The remote uplink publishes a frame on the bus.
        let kv_pub = kv.clone();
        let slug_pub = slug.clone();
        let remote_frame = move |seq: u32| {
            let slug = slug_pub.clone();
            let channel = pnex_core::camera::bus_channel(org, &slug);
            let key = pnex_core::camera::frame_key(org, &slug, seq);
            let meta = pnex_core::camera::BusFrameMeta {
                seq,
                ts_ms: chrono::Utc::now().timestamp_millis(),
                width: 320,
                height: 240,
                size: 7,
                key: key.clone(),
            };
            let meta = serde_json::to_string(&meta).unwrap();
            let latest = camera::latest_key(org, &slug);
            let mut kv = kv_pub.clone();
            async move {
                let _: () = redis::pipe()
                    .set_ex(&key, frame(seq)[16..].to_vec(), 15)
                    .ignore()
                    .set_ex(&latest, &meta, 60)
                    .ignore()
                    .publish(&channel, &meta)
                    .ignore()
                    .query_async(&mut kv)
                    .await
                    .unwrap();
            }
        };
        remote_frame(1).await;

        // Snapshot and list are served from the cluster state.
        let snap = server
            .get(&format!("/api/v1/cameras/{}/snapshot", dev.id))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        snap.assert_status_ok();
        assert_eq!(&snap.as_bytes()[..], &frame(1)[16..]);
        let _: () = kv
            .hset(camera::VIEWERS_KEY, &viewer_field, format!("2|{future}"))
            .await
            .unwrap();
        let list: serde_json::Value = server
            .get("/api/v1/cameras")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        let me = list
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["device_id"] == slug.as_str())
            .expect("listed")
            .clone();
        assert_eq!(me["streaming"], true);
        assert_eq!(me["viewers"], 2);
        assert!(me["last_frame_ms"].as_i64().is_some());

        // A local viewer is fed by the bus relay.
        let mut viewer = server
            .get_websocket(&format!(
                "/ws/camera/live?ticket={}&device={}",
                common::ws_ticket(&server, &alice, org).await,
                dev.id
            ))
            .await
            .into_websocket()
            .await;
        // Latest local frame first (none yet on this pod: the relay brings
        // them), so wait until the relay subscribed, then publish.
        let channel = pnex_core::camera::bus_channel(org, &slug);
        let mut subscribed = false;
        for _ in 0..100 {
            let n: Vec<(String, i64)> = redis::cmd("PUBSUB")
                .arg("NUMSUB")
                .arg(&channel)
                .query_async(&mut kv)
                .await
                .unwrap();
            if n.first().is_some_and(|(_, n)| *n > 0) {
                subscribed = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(subscribed, "relay subscribed to the camera bus");
        remote_frame(2).await;
        // The relayed frame (after the latest frame this pod already had).
        let want = frame(2)[16..].to_vec();
        tokio::time::timeout(Duration::from_secs(5), async {
            while viewer.receive_bytes().await.to_vec() != want {}
        })
        .await
        .expect("relayed frame");
        let list: serde_json::Value = server
            .get("/api/v1/cameras")
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        let me = list
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["device_id"] == slug.as_str())
            .unwrap()
            .clone();
        assert_eq!(me["viewers"], 3, "2 remote + 1 local");
        viewer.close().await;

        // Remote uplink gone: a local uplink takes the claim, then its
        // token is revoked → 4005 at the next message.
        let _: () = kv.del(camera::uplink_key(dev.id)).await.unwrap();
        let mut cam = common::DevWs::connect_binary(
            &server,
            &format!("/ws/camera?token={}", &dev.token,),
            &dev.key,
            &dev.device_id,
        )
        .await;
        cam.send_sealed_bytes(&frame(3)).await;
        let mut holder: Option<String> = None;
        for _ in 0..50 {
            holder = kv.get(camera::uplink_key(dev.id)).await.unwrap();
            if holder.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            holder.is_some_and(|h| !h.starts_with(other)),
            "claimed by this pod"
        );
        {
            use pnex_backend::models::_entities::device_tokens;
            use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
            device_tokens::Entity::update_many()
                .col_expr(
                    device_tokens::Column::IsActive,
                    sea_orm::sea_query::Expr::value(false),
                )
                .filter(device_tokens::Column::Token.eq(dev.token.as_str()))
                .exec(&ctx.db)
                .await
                .unwrap();
        }
        cam.send_sealed_bytes(&frame(4)).await;
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match cam.receive_message().await {
                    axum_test::WsMessage::Close(Some(f)) => return u16::from(f.code),
                    axum_test::WsMessage::Close(None) => return 0,
                    _ => {}
                }
            }
        })
        .await
        .expect("uplink closed");
        assert_eq!(closed, 4005);
        // The claim is released with the session.
        let mut released = false;
        for _ in 0..50 {
            let v: Option<String> = kv.get(camera::uplink_key(dev.id)).await.unwrap();
            if v.is_none() {
                released = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(released, "claim released on close");

        let _: () = kv.hdel(camera::VIEWERS_KEY, &viewer_field).await.unwrap();
        ctl.close().await;
        // Device ids are recycled by the next tests of this process.
        camera::forget_local(dev.id);
        clear_viewer_presence(&mut kv, dev.id).await;
    })
    .await;
    unsafe { std::env::remove_var("PNEX_TEST_APP_VALKEY_URL") };
}
