//! Camera E2E (camera-video.md D78): frames published on the Valkey frame
//! bus (exactly like the backend CameraHub) → `camera-source` →
//! `video-record` → segment POSTed to the internal video route (canned
//! server) as a readable MJPEG-AVI. Requires a live Valkey (`VALKEY_URL`,
//! default redis://127.0.0.1:6379): run with `--ignored`.

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pnex_core::{to_red_flows_json, FlowArtifactMeta, FlowGraph};

struct Upload {
    request_line: String,
    body: Vec<u8>,
}

/// Canned HTTP server keeping raw request bodies (binary AVI).
fn start_canned_server() -> (String, Arc<Mutex<Vec<Upload>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let uploads: Arc<Mutex<Vec<Upload>>> = Arc::new(Mutex::new(Vec::new()));
    let shared = uploads.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let shared = shared.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                let mut raw = Vec::new();
                let header_end = loop {
                    let Ok(n) = stream.read(&mut buf) else { return };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(p) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
                let len = head
                    .lines()
                    .find_map(|l| {
                        let l = l.to_ascii_lowercase();
                        l.strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                let mut body = raw[header_end..].to_vec();
                while body.len() < len {
                    let Ok(n) = stream.read(&mut buf) else { break };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..n]);
                }
                shared.lock().expect("lock").push(Upload {
                    request_line: head.lines().next().unwrap_or_default().to_string(),
                    body,
                });
                let resp = r#"{"id":"seg-1"}"#;
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp}",
                        resp.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    (addr, uploads)
}

fn fake_jpeg(seq: u32) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
    v.extend_from_slice(&seq.to_le_bytes());
    v.extend_from_slice(&[0xFF, 0xD9]);
    v
}

#[test]
#[ignore = "requires a live Valkey (VALKEY_URL, default redis://127.0.0.1:6379)"]
fn camera_frames_are_recorded_into_an_avi_segment() {
    let valkey = std::env::var("VALKEY_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let (addr, uploads) = start_canned_server();
    let home = common::tmp_home("camera");
    let flows = home.join("flows.json");
    let org_id = 424_242;
    let device = format!("cam-e2e-{}", std::process::id());

    let graph: FlowGraph = serde_json::from_value(serde_json::json!({
        "nodes": [
            {"id": "src", "kind": "camera_source", "config": {"device_id": device},
             "outputs": [{"port": 0, "targets": ["rec"]}]},
            {"id": "rec", "kind": "video_record",
             "config": {"segment_secs": 60, "gap_secs": 1, "retention_days": 3, "stream": "front"}}
        ]
    }))
    .expect("graph");
    let meta = FlowArtifactMeta {
        flow_id: 5,
        version_number: 1,
        org_id,
        o2_org: String::new(),
    };
    std::fs::write(&flows, to_red_flows_json(&graph, &meta).to_string()).expect("flows.json");
    let url = format!("http://{addr}/internal/flow/video-segment");
    let rt = common::RuntimeProc::spawn_with_env(
        &flows,
        &home,
        [
            ("VALKEY_URL", valkey.as_str()),
            ("PNEX_FLOW_VIDEO_URL", url.as_str()),
            ("PNEX_FLOW_WRITE_TOKEN", "tok"),
        ],
    );
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // Publish like the backend bus loop, for ~2 s at 10 fps (the source
    // subscribes asynchronously: early frames may be missed, that's fine).
    let client = redis::Client::open(valkey.as_str()).expect("valkey url");
    let mut conn = client.get_connection().expect("valkey connection");
    let start_ms = 1_790_000_000_000i64;
    for seq in 0..20u32 {
        let key = pnex_core::camera::frame_key(org_id, &device, seq);
        let meta = pnex_core::camera::BusFrameMeta {
            seq,
            ts_ms: start_ms + i64::from(seq) * 100,
            width: 320,
            height: 240,
            size: 10,
            key: key.clone(),
        };
        let _: () = redis::pipe()
            .set_ex(&key, fake_jpeg(seq), 15)
            .ignore()
            .publish(
                pnex_core::camera::bus_channel(org_id, &device),
                serde_json::to_string(&meta).unwrap(),
            )
            .ignore()
            .query(&mut conn)
            .expect("publish");
        std::thread::sleep(Duration::from_millis(100));
    }

    // gap_secs = 1 → the recorder flushes shortly after the last frame.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && uploads.lock().expect("lock").is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let uploads = uploads.lock().expect("lock");
    assert_eq!(uploads.len(), 1, "one segment expected");
    let up = &uploads[0];
    assert!(
        up.request_line
            .starts_with("POST /internal/flow/video-segment?"),
        "{}",
        up.request_line
    );
    assert!(
        up.request_line.contains(&format!("device_id={device}")),
        "{}",
        up.request_line
    );
    assert!(
        up.request_line.contains("stream=front"),
        "{}",
        up.request_line
    );
    assert!(
        up.request_line.contains("retention_days=3"),
        "{}",
        up.request_line
    );
    let idx = pnex_core::avi::read_avi(&up.body).expect("valid AVI");
    assert!(
        idx.frames.len() >= 5,
        "frames recorded: {}",
        idx.frames.len()
    );
    assert_eq!((idx.width, idx.height), (320, 240));
    // ~10 fps playback rate derived from the frame timestamps.
    assert!((idx.fps - 10.0).abs() < 0.5, "fps {}", idx.fps);
    let (s, l) = idx.frames[0];
    assert_eq!(&up.body[s..s + 2], &[0xFF, 0xD8]);
    assert!(l > 4);
}
