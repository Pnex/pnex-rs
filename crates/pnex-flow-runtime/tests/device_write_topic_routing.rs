//! Acceptance `pnex-device-write` topic routing: wires drawn on the node's
//! declared input anchors must get topic taggers at deploy (real projection),
//! and the node must unit-write the pin named by the topic. A canned HTTP
//! server records the POSTs of the internal device-write route.

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pnex_core::{to_red_flows_json, FlowArtifactMeta, FlowGraph};

/// One request recorded by the canned server (for assertions).
#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    body: String,
}

/// Minimal canned HTTP server: request line + Content-Length parsing, canned
/// `{"results":[]}` response (valid per-pin result list for the write node),
/// records each request. Zero deps (school: tests/http_fetch_node.rs).
fn start_canned_server() -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let shared = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let shared = shared.clone();
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
                let header_end = raw
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(raw.len());
                let text = String::from_utf8_lossy(&raw).into_owned();
                let request_line = text.lines().next().unwrap_or_default();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let content_length =
                    text.to_ascii_lowercase()
                        .find("content-length:")
                        .and_then(|i| {
                            text[i + 15..]
                                .trim_start()
                                .chars()
                                .take_while(|c| c.is_ascii_digit())
                                .collect::<String>()
                                .parse::<usize>()
                                .ok()
                        });
                let mut body = raw[header_end..].to_vec();
                while let Some(target) = content_length {
                    if body.len() >= target {
                        break;
                    }
                    let Ok(n) = stream.read(&mut buf) else { break };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&buf[..n]);
                }
                let rec = RecordedRequest {
                    method,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                shared.lock().expect("lock").push(rec.clone());
                let resp_body = r#"{"results":[]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    (addr, requests)
}

/// Typed flow graph (the user's exact shape): inject → value(static map) →
/// json_split(test/test2) → device_write(G13/G14, input anchors wired) →
/// debug. The artifact is produced by the REAL projection so the routing
/// gate in `routed_wires` stays covered by this test.
fn topic_routing_graph() -> serde_json::Value {
    serde_json::json!({
        "nodes": [
            {
                "id": "i1",
                "outputs": [{ "port": 0, "targets": ["v1"] }],
                "kind": "inject",
                "config": { "repeat_secs": 0.1 }
            },
            {
                "id": "v1",
                "outputs": [{ "port": 0, "targets": ["s1"] }],
                "kind": "value",
                "config": { "mode": "static", "value": { "test": 1, "test2": 100, "test3": 16711680 } }
            },
            {
                "id": "s1",
                "outputs": [
                    { "port": 0, "targets": ["w1"] },
                    { "port": 1, "targets": ["w1"] },
                    { "port": 2, "targets": ["w1"] }
                ],
                "kind": "json_split",
                "config": { "keys": ["test", "test2", "test3"] }
            },
            {
                "id": "w1",
                "outputs": [{ "port": 0, "targets": ["d1"] }],
                "inputs": [
                    { "pin": "G13", "from": "s1", "from_port": 0 },
                    { "pin": "G14", "from": "s1", "from_port": 1 },
                    { "pin": "color", "from": "s1", "from_port": 2 }
                ],
                "kind": "device_write",
                "config": { "device_id": "dev-a", "pins": ["G13", "G14"], "commands": ["color"] }
            },
            {
                "id": "d1",
                "outputs": [],
                "kind": "debug",
                "config": { "active": true, "complete": "true" }
            }
        ]
    })
}

#[test]
#[ignore = "requires a live server plus flow runtime; covered by the live E2E harness, not CI"]
fn split_topic_routed_writes_reach_the_write_route() {
    let (addr, requests) = start_canned_server();
    let home = common::tmp_home("devwrite");
    let flows = home.join("flows.json");

    let graph: FlowGraph = serde_json::from_value(topic_routing_graph()).expect("valid flow graph");
    let meta = FlowArtifactMeta {
        flow_id: 22,
        version_number: 1,
        org_id: 7,
        o2_org: String::new(),
    };
    let artifact = to_red_flows_json(&graph, &meta);
    std::fs::write(&flows, artifact.to_string()).expect("write flows.json");

    let url = format!("http://{addr}");
    let rt = common::RuntimeProc::spawn_with_env(
        &flows,
        &home,
        [
            ("PNEX_FLOW_WRITE_URL", url.as_str()),
            ("PNEX_FLOW_WRITE_TOKEN", "test-token"),
        ],
    );

    wait_started(&rt);
    // The debug node captures the whole msg (complete: "true") so the tagger
    // topics travel to the debug events; both pins must show up there.
    // Match on the unescaped msg field (school: the other runtime tests) — a quoted
    // needle against the re-serialized event never matches (inner quotes
    // come back escaped).
    rt.wait_for(
        |v| {
            v["msg"]
                .as_str()
                .is_some_and(|m| m.contains("\"topic\": \"G13\""))
        },
        Duration::from_secs(30),
    );
    rt.wait_for(
        |v| {
            v["msg"]
                .as_str()
                .is_some_and(|m| m.contains("\"topic\": \"G14\""))
        },
        Duration::from_secs(30),
    );

    // Both unit writes must have landed on the internal route (deadline loop:
    // each POST lands right after its debug event).
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if requests.lock().expect("lock").len() >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let recorded = requests.lock().expect("lock").clone();
    assert!(
        !recorded.is_empty(),
        "at least one POST per inject tick expected"
    );
    // The inject re-fires every 100 ms, so more than one round may land —
    // assert on the DISTINCT bodies: exactly one unit write per wired pin.
    let mut bodies: Vec<serde_json::Value> = recorded
        .iter()
        .map(|r| serde_json::from_str(&r.body).expect("POST body is JSON"))
        .collect();
    bodies.sort_by_key(|b| b.to_string());
    bodies.dedup();
    assert_eq!(
        bodies,
        vec![
            // Custom-firmware command anchor (D146): same routing, sent
            // under `commands`.
            serde_json::json!({
                "org_id": 7,
                "device_id": "dev-a",
                "values": {},
                "commands": { "color": 16711680 }
            }),
            serde_json::json!({
                "org_id": 7,
                "device_id": "dev-a",
                "values": { "G13": 1 }
            }),
            serde_json::json!({
                "org_id": 7,
                "device_id": "dev-a",
                "values": { "G14": 100 }
            }),
        ],
        "exactly one distinct unit write per wired pin or command"
    );
    for rec in &recorded {
        assert_eq!(rec.method, "POST", "only POSTs expected: {rec:?}");
    }
}

/// Wait for the `started` event of the runtime.
fn wait_started(rt: &common::RuntimeProc) {
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
}
