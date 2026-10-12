//! Video understanding nodes on the real engine (media-vision-studio.md
//! D195–D197): `vision-caption`, `media-fuse` and `text-decide` against a
//! canned backend (`/internal/flow/ai/{caption,fuse,decide}`) — the runtime side only: the
//! backend reads the frame and calls the org model.

mod common;

use std::io::{Read, Write};
use std::time::Duration;

/// Canned backend: `/caption` → a caption, `/fuse` → one sourced sentence.
fn start_ai_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = [0u8; 65536];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                let body = if head.starts_with("POST /internal/flow/ai/decide") {
                    serde_json::json!({
                        "answers": {"incident": {"best": "routine", "probability": 0.7,
                            "probabilities": [["incident", 0.3], ["routine", 0.7]], "confidence": 0.12}},
                        "model": "ml", "took_ms": 5
                    })
                } else if head.starts_with("POST /internal/flow/ai/caption") {
                    serde_json::json!({"caption": "a cat on a chair", "model": "m", "took_ms": 3})
                } else {
                    serde_json::json!({
                        "sentences": [{"text": "A cat sat on the chair.", "sources": ["cap-1"]}],
                        "rejected": 0, "model": "m", "took_ms": 4
                    })
                }
                .to_string();
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
            });
        }
    });
    addr
}

fn flows() -> String {
    let fake_frame = r#"
msg.payload = { device_id: "cam-1", ts_ms: Date.now(), frame_key: "pnex:cam:v1:5:cam-1:f:1" };
return msg;
"#;
    serde_json::json!([
        { "id": "pnexflow8", "type": "tab", "label": "ai", "pnex_flow_id": 8 },
        {
            "id": "tick", "type": "inject", "z": "pnexflow8",
            "props": [{ "p": "payload" }], "repeat": "0.5", "once": true,
            "payload": "x", "payloadType": "str", "wires": [["frame"]]
        },
        {
            "id": "frame", "type": "function", "z": "pnexflow8",
            "outputs": 1, "func": fake_frame, "wires": [["cap"]]
        },
        {
            "id": "cap", "type": "pnex-vision-caption", "z": "pnexflow8",
            "prompt": "What is there?", "min_interval_secs": 1.0, "crop_best": false,
            "pnex_org_id": 5, "pnex_node_id": "cap", "wires": [["fuse", "dbg"]]
        },
        {
            "id": "fuse", "type": "pnex-media-fuse", "z": "pnexflow8",
            "window_secs": 5.0, "language": "en",
            "pnex_org_id": 5, "pnex_node_id": "fuse", "wires": [["dbg", "dec"]]
        },
        {
            "id": "dec", "type": "pnex-text-decide", "z": "pnexflow8",
            "questions": [{"id": "incident", "kind": "choice", "instructions": "Incident?",
                           "options": ["incident", "routine"]}],
            "text_field": "text", "pnex_org_id": 5, "pnex_node_id": "dec", "wires": [["dbg"]]
        },
        {
            "id": "dbg", "type": "debug", "z": "pnexflow8",
            "active": true, "tosidebar": true, "console": false,
            "complete": "payload", "wires": []
        }
    ])
    .to_string()
}

#[test]
fn caption_then_fused_description() {
    let addr = start_ai_server();
    let home = common::tmp_home("vai");
    let path = home.join("flows.json");
    std::fs::write(&path, flows()).unwrap();
    let ai_url = format!("http://{addr}/internal/flow/ai");
    let rt = common::RuntimeProc::spawn_with_env(
        &path,
        &home,
        [
            ("PNEX_FLOW_AI_URL", ai_url.as_str()),
            ("PNEX_FLOW_WRITE_TOKEN", "t"),
        ],
    );
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
    rt.wait_for(
        |v| {
            common::is_debug_with(v, "a cat on a chair") && common::is_debug_with(v, "ai_generated")
        },
        Duration::from_secs(15),
    );
    rt.wait_for(
        |v| {
            common::is_debug_with(v, "A cat sat on the chair.") && common::is_debug_with(v, "t0_ms")
        },
        Duration::from_secs(20),
    );
    // The fused text goes on to the decision node, which keeps the payload.
    rt.wait_for(
        |v| common::is_debug_with(v, "decisions") && common::is_debug_with(v, "A cat sat"),
        Duration::from_secs(20),
    );
}
