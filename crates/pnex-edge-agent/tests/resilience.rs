//! End-to-end agent behavior against an in-process fake `/ws/device` server:
//! concurrent producers, a connection killed mid-stream (unacked batches are
//! resent, the server dedups by seq), and a server that is down while points
//! are accepted then shows up after an agent restart (durable queue).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use pnex_core::frame::{decrypt_frame, encrypt_frame};
use pnex_core::{DeviceMsg, ServerMsg};
use tokio_tungstenite::tungstenite::Message;

const KEY: [u8; 32] = [9u8; 32];

#[derive(Default)]
struct FakeServer {
    /// seq → (key, value) of every point accepted exactly once.
    points: Mutex<BTreeMap<u64, (String, serde_json::Value)>>,
    hwm: Mutex<u64>,
    duplicates: AtomicUsize,
    batches: AtomicUsize,
    connections: AtomicUsize,
    /// Kill the first connection after this many batches (0 = never).
    kill_after: usize,
}

async fn serve(listener: tokio::net::TcpListener, srv: Arc<FakeServer>) {
    loop {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let srv = srv.clone();
        tokio::spawn(async move {
            let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else {
                return;
            };
            let conn_no = srv.connections.fetch_add(1, Ordering::SeqCst) + 1;
            let (mut tx, mut rx) = ws.split();
            let mut batches_here = 0usize;
            while let Some(Ok(msg)) = rx.next().await {
                let Message::Text(text) = msg else { continue };
                let plain = decrypt_frame(text.as_str(), &KEY).expect("decrypt");
                if plain == "PING" {
                    let _ = tx
                        .send(Message::Text(encrypt_frame("PONG", &KEY).into()))
                        .await;
                    continue;
                }
                let reply = match serde_json::from_str::<DeviceMsg>(&plain).expect("DeviceMsg") {
                    DeviceMsg::Announce { chip, .. } => {
                        assert_eq!(chip, "agent");
                        ServerMsg::AgentConfig {
                            max_batch: 50,
                            max_keys: 1000,
                        }
                    }
                    DeviceMsg::Batch { epoch, points } => {
                        batches_here += 1;
                        srv.batches.fetch_add(1, Ordering::SeqCst);
                        let mut hwm = srv.hwm.lock().unwrap();
                        let mut store = srv.points.lock().unwrap();
                        let mut up_to = *hwm;
                        for p in points {
                            if p.seq <= *hwm {
                                srv.duplicates.fetch_add(1, Ordering::SeqCst);
                            } else {
                                store.insert(p.seq, (p.key, p.value));
                            }
                            up_to = up_to.max(p.seq);
                        }
                        *hwm = up_to;
                        drop((hwm, store));
                        if conn_no == 1 && srv.kill_after > 0 && batches_here == srv.kill_after {
                            // Processed but never acknowledged: the agent must resend.
                            return;
                        }
                        ServerMsg::BatchAck {
                            epoch,
                            up_to_seq: up_to,
                        }
                    }
                    _ => continue,
                };
                let plain = serde_json::to_string(&reply).unwrap();
                if tx
                    .send(Message::Text(encrypt_frame(&plain, &KEY).into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn write_config(dir: &Path, server_port: u16, api_port: u16) {
    std::fs::write(
        dir.join("config.toml"),
        format!("server = \"http://127.0.0.1:{server_port}\"\nlisten = \"127.0.0.1:{api_port}\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("secrets.json"),
        serde_json::json!({
            "device_id": "agent-test",
            "token": "tok",
            "encryption_key": STANDARD.encode(KEY),
            "ws_path": "/ws/device"
        })
        .to_string(),
    )
    .unwrap();
}

struct Running {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn start_agent(dir: &Path, api_port: u16) -> Running {
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let dir = dir.to_path_buf();
    let task = tokio::spawn(async move {
        pnex_edge_agent::run::run(&dir, async move {
            let _ = rx.await;
        })
        .await
    });
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if client
            .get(format!("http://127.0.0.1:{api_port}/healthz"))
            .send()
            .await
            .is_ok()
        {
            return Running { stop, task };
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("agent API never came up");
}

async fn stop_agent(r: Running) {
    let _ = r.stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), r.task).await;
}

async fn status(api_port: u16) -> serde_json::Value {
    reqwest::get(format!("http://127.0.0.1:{api_port}/v1/status"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn wait_delivered(srv: &FakeServer, api_port: u16, expected: usize) {
    for _ in 0..600 {
        let n = srv.points.lock().unwrap().len();
        if n == expected && status(api_port).await["queue_depth"] == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "delivered {} / {expected}, status {}",
        srv.points.lock().unwrap().len(),
        status(api_port).await
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_producers_survive_a_killed_connection_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_port = listener.local_addr().unwrap().port();
    let srv = Arc::new(FakeServer {
        kill_after: 3,
        ..Default::default()
    });
    tokio::spawn(serve(listener, srv.clone()));
    let api_port = free_port();
    write_config(dir.path(), server_port, api_port);
    let agent = start_agent(dir.path(), api_port).await;

    const PRODUCERS: usize = 8;
    const PER_PRODUCER: usize = 250;
    let client = reqwest::Client::new();
    let mut tasks = Vec::new();
    for t in 0..PRODUCERS {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            for chunk in 0..(PER_PRODUCER / 25) {
                let body: Vec<serde_json::Value> = (0..25)
                    .map(|i| serde_json::json!({"key": format!("k{t}"), "value": chunk * 25 + i}))
                    .collect();
                let res = client
                    .post(format!("http://127.0.0.1:{api_port}/v1/points"))
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(res.status(), 202, "{}", res.text().await.unwrap());
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let total = PRODUCERS * PER_PRODUCER;
    wait_delivered(&srv, api_port, total).await;

    // Exactly once per (producer, value), whatever the resends.
    let points = srv.points.lock().unwrap().clone();
    for t in 0..PRODUCERS {
        let mut values: Vec<u64> = points
            .values()
            .filter(|(k, _)| *k == format!("k{t}"))
            .map(|(_, v)| v.as_u64().unwrap())
            .collect();
        values.sort_unstable();
        assert_eq!(
            values,
            (0..PER_PRODUCER as u64).collect::<Vec<_>>(),
            "producer {t}"
        );
    }
    assert!(
        srv.connections.load(Ordering::SeqCst) >= 2,
        "the kill forced a reconnect"
    );
    assert!(
        srv.duplicates.load(Ordering::SeqCst) > 0,
        "unacked batch was resent"
    );
    stop_agent(agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn points_accepted_while_server_is_down_survive_an_agent_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server_port = free_port();
    let api_port = free_port();
    write_config(dir.path(), server_port, api_port);

    // Server down: the API still answers 202 once points are on disk.
    let agent = start_agent(dir.path(), api_port).await;
    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://127.0.0.1:{api_port}/v1/points"))
        .header("content-type", "text/plain")
        .body(
            (0..300)
                .map(|i| format!("offline=value{i}\n"))
                .collect::<String>(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    let st = status(api_port).await;
    assert_eq!(st["queue_depth"], 300);
    assert_ne!(st["link"], "connected");
    stop_agent(agent).await;

    // Server comes up, agent restarts: the durable queue drains.
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], server_port)))
        .await
        .unwrap();
    let srv = Arc::new(FakeServer::default());
    tokio::spawn(serve(listener, srv.clone()));
    let agent = start_agent(dir.path(), api_port).await;
    wait_delivered(&srv, api_port, 300).await;
    let first = srv.points.lock().unwrap().values().next().cloned().unwrap();
    assert_eq!(first, ("offline".to_string(), serde_json::json!("value0")));
    stop_agent(agent).await;
}
