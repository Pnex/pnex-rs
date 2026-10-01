//! Acceptance nœud `pnex-http-fetch` (C1a) : `inject → pnex-http-fetch →
//! debug` contre un mini-serveur HTTP canned (zéro dep, TcpListener std).
//! GET JSON parsé, POST avec corps enregistré côté serveur, 404 passthrough.

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Une requête reçue par le serveur canned (pour assertions).
#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    /// Raw request head (request line + headers).
    head: String,
    body: String,
}

/// Mini serveur HTTP canned : parse request line + Content-Length, répond
/// selon `responder`, enregistre chaque requête. Aucune dep.
fn start_canned_server(
    responder: impl Fn(&RecordedRequest) -> (u16, &'static str, String) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<RecordedRequest>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let responder = Arc::new(responder);
    let shared = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let shared = shared.clone();
            let responder = responder.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut raw = Vec::new();
                // En-têtes jusqu'à \r\n\r\n.
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
                let path = parts.next().unwrap_or_default().to_string();
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
                // Corps restant selon Content-Length.
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
                    path,
                    head: text[..header_end.min(text.len())].to_string(),
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                shared.lock().expect("lock").push(rec.clone());
                let (status, content_type, resp_body) = responder(&rec);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    _ => "Error",
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    (addr, requests)
}

/// flows JSON projeté (miroir de `to_red_flows_json`) : inject →
/// pnex-http-fetch → debug.
fn http_fetch_flows(
    url: &str,
    method: &str,
    body: Option<&str>,
    on_error: &str,
    debug_complete: &str,
) -> String {
    serde_json::json!([
        { "id": "pnexflow1", "type": "tab", "label": "http-fetch", "pnex_flow_id": 1 },
        {
            "id": "i1", "type": "inject", "z": "pnexflow1",
            "props": [{ "p": "payload" }],
            "repeat": "", "once": true, "onceDelay": 0.1,
            "payloadType": "json",
            "payload": "{\"entree\":1}",
            "wires": [["h1"]]
        },
        {
            "id": "h1", "type": "pnex-http-fetch", "z": "pnexflow1",
            "url": url,
            "method": method,
            "headers": [],
            "auth": { "mode": "none" },
            "proxy": { "mode": "none" },
            "timeout_secs": 10,
            "body": body,
            "on_error": on_error,
            "wires": [["d1"]]
        },
        {
            "id": "d1", "type": "debug", "z": "pnexflow1",
            "active": true, "tosidebar": true, "console": false,
            "complete": debug_complete, "wires": []
        }
    ])
    .to_string()
}

/// Attend l'événement `started` du runtime.
fn wait_started(rt: &common::RuntimeProc) {
    rt.wait_for(
        |v| v.get("event").and_then(|e| e.as_str()) == Some("started"),
        Duration::from_secs(30),
    );
}

/// Attend l'événement debug contenant `needle`, rend son champ msg.
fn wait_debug(rt: &common::RuntimeProc, needle: &str) -> String {
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.to_string().contains(needle)
        },
        Duration::from_secs(30),
    );
    line["msg"].as_str().unwrap_or_default().to_string()
}

#[test]
fn http_fetch_get_json_parse_le_payload() {
    let home = common::tmp_home("httpfetch-get");
    let flows = home.join("flows.json");
    let (addr, _requests) =
        start_canned_server(|_req| (200, "application/json", r#"{"temp": 21.5}"#.to_string()));

    std::fs::write(
        &flows,
        http_fetch_flows(
            &format!("http://{addr}/collect"),
            "get",
            None,
            "reject",
            "payload",
        ),
    )
    .unwrap();
    let rt = common::RuntimeProc::spawn(&flows, &home);
    wait_started(&rt);
    let msg = wait_debug(&rt, "temp");
    assert!(msg.contains("21.5"), "{msg}");
}

#[test]
fn http_fetch_post_envoie_le_corps_et_enregistre() {
    let home = common::tmp_home("httpfetch-post");
    let flows = home.join("flows.json");
    let (addr, requests) = start_canned_server(|_req| (200, "text/plain", "ok-post".to_string()));

    std::fs::write(
        &flows,
        http_fetch_flows(
            &format!("http://{addr}/ingest"),
            "post",
            Some(r#"{"cible":"x"}"#),
            "reject",
            "payload",
        ),
    )
    .unwrap();
    let rt = common::RuntimeProc::spawn(&flows, &home);
    wait_started(&rt);
    let msg = wait_debug(&rt, "ok-post");
    assert!(msg.contains("ok-post"), "{msg}");

    let recs = requests.lock().expect("lock");
    assert!(!recs.is_empty(), "aucune requête enregistrée");
    let last = recs.last().expect("non vide");
    assert_eq!(last.method, "POST");
    assert_eq!(last.path, "/ingest");
    assert_eq!(last.body, r#"{"cible":"x"}"#);
}

#[test]
fn http_fetch_404_passthrough_expose_status_code() {
    let home = common::tmp_home("httpfetch-404");
    let flows = home.join("flows.json");
    let (addr, _requests) = start_canned_server(|_req| (404, "text/plain", "absent".to_string()));

    std::fs::write(
        &flows,
        http_fetch_flows(
            &format!("http://{addr}/ghost"),
            "get",
            None,
            "passthrough",
            "true",
        ),
    )
    .unwrap();
    let rt = common::RuntimeProc::spawn(&flows, &home);
    wait_started(&rt);
    // complete "true" = message entier → statusCode + http_error présents.
    let line = rt.wait_for(
        |v| {
            v.get("event").and_then(|e| e.as_str()) == Some("debug")
                && v.to_string().contains("statusCode")
        },
        Duration::from_secs(30),
    );
    let full = line.to_string();
    assert!(full.contains("404"), "{full}");
    assert!(full.contains("http_error"), "{full}");
}

/// Vault reference (secrets.md S5): the node fetches the token from the
/// secret endpoint on its first request and sends it as the bearer; the
/// artifact itself never holds the value.
#[test]
fn http_fetch_resolves_a_vault_bearer_token() {
    let home = common::tmp_home("httpfetch-vault");
    let flows = home.join("flows.json");
    let secret_id = "7f1c2a4e-0000-4000-8000-000000000001";
    let (addr, requests) = start_canned_server(move |req| {
        if req.path.starts_with("/internal/flow/secret/") {
            let ok = req.path.contains(secret_id)
                && req.path.contains("org_id=42")
                && req
                    .head
                    .to_ascii_lowercase()
                    .contains("x-pnex-flow-token: rt-token");
            return if ok {
                (
                    200,
                    "application/json",
                    r#"{"value":"tok-vault"}"#.to_string(),
                )
            } else {
                (404, "application/json", "{}".to_string())
            };
        }
        if req.head.contains("Bearer tok-vault") || req.head.contains("bearer tok-vault") {
            (200, "text/plain", "ok-auth".to_string())
        } else {
            (401, "text/plain", "no-auth".to_string())
        }
    });
    let mut entries: serde_json::Value = serde_json::from_str(&http_fetch_flows(
        &format!("http://{addr}/protected"),
        "get",
        None,
        "passthrough",
        "true",
    ))
    .unwrap();
    entries[2]["auth"] =
        serde_json::json!({ "mode": "bearer", "token": { "secret_id": secret_id } });
    entries[2]["pnex_org_id"] = serde_json::json!(42);
    let artifact = entries.to_string();
    assert!(!artifact.contains("tok-vault"));
    std::fs::write(&flows, artifact).unwrap();

    let secret_url = format!("http://{addr}/internal/flow/secret");
    let rt = common::RuntimeProc::spawn_with_env(
        &flows,
        &home,
        [
            ("PNEX_FLOW_SECRET_URL", secret_url.as_str()),
            ("PNEX_FLOW_WRITE_TOKEN", "rt-token"),
        ],
    );
    wait_started(&rt);
    let msg = wait_debug(&rt, "ok-auth");
    assert!(msg.contains("ok-auth"), "{msg}");
    let seen = requests.lock().expect("lock");
    assert!(seen
        .iter()
        .any(|r| r.path.starts_with("/internal/flow/secret/")));
}
