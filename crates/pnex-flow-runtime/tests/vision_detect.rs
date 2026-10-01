//! Vision E2E (camera-video.md D83): camera frames on the Valkey bus →
//! `camera-source` → `vision-detect` (registry model served by a canned
//! backend) → debug event carrying the detections. Requires a live Valkey
//! and `PNEX_VISION_TEST_DIR` (yolox_nano.onnx + dog.jpg): `--ignored`.

mod common;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Canned backend: `/…/ml-model/{id}` → spec JSON, `/…/content` → ONNX.
fn start_model_server(onnx: Vec<u8>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let onnx = onnx.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut raw = Vec::new();
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&raw).into_owned();
                let line = head.lines().next().unwrap_or_default().to_string();
                let (ctype, body): (&str, Vec<u8>) = if line.contains("/content") {
                    ("application/octet-stream", onnx)
                } else {
                    let model = serde_json::json!({
                        "model": {
                            "id": "m1", "name": "yolox-nano", "description": "",
                            "task": "detection", "asset_id": "a1",
                            "spec": pnex_core::vision::ModelSpec::default(),
                            "created_at": "", "updated_at": ""
                        },
                        "sha256": "x"
                    });
                    ("application/json", model.to_string().into_bytes())
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    addr
}

#[test]
#[ignore = "requires a live Valkey + PNEX_VISION_TEST_DIR (yolox_nano.onnx, dog.jpg)"]
fn camera_frames_are_detected() {
    let dir = std::path::PathBuf::from(std::env::var("PNEX_VISION_TEST_DIR").expect("dir"));
    let onnx = std::fs::read(dir.join("yolox_nano.onnx")).expect("model");
    let jpeg = std::fs::read(dir.join("dog.jpg")).expect("image");
    let valkey = std::env::var("VALKEY_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let addr = start_model_server(onnx);
    let home = common::tmp_home("vision");
    let flows = home.join("flows.json");
    let org_id = 434_343;
    let device = format!("cam-vis-{}", std::process::id());
    let tab = "pnexflow9";
    let artifact = serde_json::json!([
        {"id": tab, "type": "tab", "label": "vision", "pnex_flow_id": 9, "pnex_version": 1,
         "pnex_org_id": org_id, "pnex_o2_org": ""},
        {"id": "src", "z": tab, "type": "pnex-camera-source", "device_id": device, "max_fps": 0,
         "pnex_org_id": org_id, "wires": [["det"]]},
        {"id": "det", "z": tab, "type": "pnex-vision-detect", "model_id": "m1",
         "labels": ["dog", "bicycle"], "min_score": 0.5, "emit": "on_detection", "max_fps": 0,
         "pnex_org_id": org_id, "wires": [["dbg"]]},
        {"id": "dbg", "z": tab, "type": "debug", "active": true, "complete": "payload",
         "tosidebar": true, "wires": []}
    ]);
    std::fs::write(&flows, artifact.to_string()).expect("flows.json");
    let model_url = format!("http://{addr}/internal/flow/ml-model");
    let rt = common::RuntimeProc::spawn_with_env(
        &flows,
        &home,
        [
            ("VALKEY_URL", valkey.as_str()),
            ("PNEX_FLOW_MODEL_URL", model_url.as_str()),
            ("PNEX_FLOW_WRITE_TOKEN", "tok"),
        ],
    );
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );

    // Publish the reference scene every 500 ms (fresh timestamps: the node
    // skips stale frames) until a detection event shows up.
    let client = redis::Client::open(valkey.as_str()).expect("valkey url");
    let mut conn = client.get_connection().expect("valkey connection");
    let publisher = {
        let device = device.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(40);
            let mut seq = 0u32;
            while Instant::now() < deadline {
                let key = pnex_core::camera::frame_key(org_id, &device, seq);
                let meta = pnex_core::camera::BusFrameMeta {
                    seq,
                    ts_ms: chrono_now_ms(),
                    width: 768,
                    height: 576,
                    size: jpeg.len() as u32,
                    key: key.clone(),
                };
                let _: redis::RedisResult<()> = redis::pipe()
                    .set_ex(&key, jpeg.as_slice(), 15)
                    .ignore()
                    .publish(
                        pnex_core::camera::bus_channel(org_id, &device),
                        serde_json::to_string(&meta).unwrap(),
                    )
                    .ignore()
                    .query(&mut conn);
                seq += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    };
    rt.wait_for(
        |v| {
            v["msg"].as_str().is_some_and(|m| {
                m.contains("\"dog\"") && m.contains("\"bicycle\"") && m.contains("detections")
            })
        },
        Duration::from_secs(40),
    );
    drop(publisher);
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64
}
