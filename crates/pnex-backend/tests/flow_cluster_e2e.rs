//! Multi-process E2E of the flow cluster (D106/D107): two REAL
//! `pnex-server` processes (the binary of this crate, ports 5251/5252),
//! the REAL `pnex-flow-runtime` (release build), a fresh PostgreSQL
//! database, an isolated Valkey db (index 9) and a mock IdP served by this
//! test process.
//!
//! Scenario: flows of four orgs deployed through BOTH servers are sharded
//! over the two workers (cross-pod apply over HTTP), runtime status is
//! answered cross-pod, pod B is SIGKILLed (its runtime child dies with it,
//! its orgs fail over to pod A which then runs every flow), pod A is
//! SIGTERMed (graceful drain: runtime stopped, worker deregistered).
//!
//! Ignored by default (needs the services and the release runtime):
//! `cargo build --release -p pnex-flow-runtime` then
//! `cargo test -p pnex-backend --test flow_cluster_e2e -- --ignored`.

mod common;

use std::collections::{HashMap, HashSet};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, EntityTrait};

const TOKEN: &str = "e2e-cluster-token";

struct Pod {
    id: &'static str,
    port: u16,
    dir: String,
    child: Child,
}

impl Pod {
    fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    fn runtime_pid(&self) -> Option<i32> {
        let raw = std::fs::read_to_string(format!("{}/runtime.json", self.dir)).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        v["pid"].as_i64().map(|p| p as i32)
    }

    fn artifact_flows(&self) -> HashSet<i64> {
        let raw = std::fs::read_to_string(format!("{}/flows.json", self.dir))
            .unwrap_or_else(|_| "[]".into());
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
        v.as_array()
            .into_iter()
            .flatten()
            .filter(|n| n["type"] == "tab")
            .filter_map(|n| n["pnex_flow_id"].as_i64())
            .collect()
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn runtime_bin() -> String {
    let p = std::env::var("PNEX_E2E_RUNTIME").unwrap_or_else(|_| {
        format!(
            "{}/../../target/release/pnex-flow-runtime",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    std::fs::canonicalize(&p)
        .unwrap_or_else(|_| {
            panic!("runtime binary not found at {p} (cargo build --release -p pnex-flow-runtime)")
        })
        .display()
        .to_string()
}

fn spawn_pod(id: &'static str, port: u16, db_url: &str, rauthy: &str, tag: &str) -> Pod {
    let dir = format!("/tmp/pnex-e2e-cluster-{tag}-{id}");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let log = std::fs::File::create(format!("{dir}.log")).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_pnex-server"))
        .arg("start")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("LOCO_ENV", "test")
        .env("PNEX_TEST_LOG", "true")
        .env("PNEX_TEST_PORT", port.to_string())
        .env("PNEX_TEST_TRUNCATE", "false")
        .env("PNEX_TEST_APP_VALKEY_URL", "redis://127.0.0.1:6379/9")
        .env("TEST_DATABASE_URL", db_url)
        .env("RAUTHY_URL", rauthy)
        .env("PNEX_FLOW_ENABLED", "true")
        .env("PNEX_FLOW_RUNTIME_CMD", runtime_bin())
        .env("PNEX_FLOW_STATE_DIR", &dir)
        .env("PNEX_FLOW_RELOAD_ACK_SECS", "10")
        .env("PNEX_FLOW_DEBUG_TOOLS", "true")
        .env("PNEX_FLOW_WORKER_ID", id)
        .env(
            "PNEX_FLOW_ADVERTISE_URL",
            format!("http://localhost:{port}"),
        )
        .env("PNEX_FLOW_CLUSTER_TOKEN", TOKEN)
        .env("PNEX_FLOW_RUNTIME_TOKEN", "e2e-runtime-token")
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn pnex-server");
    Pod {
        id,
        port,
        dir,
        child,
    }
}

async fn wait_ready(pod: &Pod) {
    let http = reqwest::Client::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(r) = http.get(format!("{}/health/live", pod.url())).send().await {
            if r.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("{} not ready (see {}.log)", pod.id, pod.dir);
}

async fn wait_for<F, Fut>(what: &str, timeout: Duration, mut cond: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("timed out after {timeout:?} waiting for: {what}");
}

async fn placements(db: &DatabaseConnection) -> HashMap<i64, String> {
    pnex_backend::models::_entities::flow_placements::Entity::find()
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|p| (p.org_id, p.worker_id))
        .collect()
}

#[tokio::test]
#[ignore = "multi-process E2E: postgres + valkey + release runtime; run with --ignored"]
async fn two_real_servers_shard_fail_over_and_drain() {
    let tag = std::process::id().to_string();
    let admin_url = std::env::var("PNEX_E2E_ADMIN_DB")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/postgres".into());
    let db_name = format!("pnex_e2e_cluster_{tag}");
    let admin = Database::connect(&admin_url).await.expect("postgres admin");
    admin
        .execute_unprepared(&format!("CREATE DATABASE {db_name}"))
        .await
        .unwrap();
    let db_url = admin_url.rsplit_once('/').unwrap().0.to_string() + "/" + &db_name;
    let rauthy = common::spawn_mock_rauthy().await;

    // Pod A first: it migrates the fresh database.
    let mut a = spawn_pod("e2e-a", 5251, &db_url, &rauthy, &tag);
    wait_ready(&a).await;
    let db = Database::connect(&db_url).await.unwrap();
    common::seed_catalogue(&db).await;
    let mut b = spawn_pod("e2e-b", 5252, &db_url, &rauthy, &tag);
    wait_ready(&b).await;

    let http = reqwest::Client::new();
    let pods = [a.url(), b.url()];
    let mut flows: Vec<(String, i64, i64)> = Vec::new();
    for i in 0..4usize {
        let token = common::valid_token(
            &rauthy,
            &format!("00000000-0000-0000-0000-{:012}", 500 + i),
            &format!("e2e{i}"),
            &format!("e2e{i}@example.com"),
        );
        // Alternate the entry pod: an API call never needs to land on the
        // owner of the org.
        let api = &pods[i % 2];
        let info: serde_json::Value = http
            .get(format!("{api}/api/v1/user-info"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let org = info["orgs"][0]["id"].as_i64().expect("personal org");
        let created: serde_json::Value = http
            .post(format!("{api}/api/v1/flows"))
            .bearer_auth(&token)
            .header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": format!("e2e-{i}"),
                "graph": { "nodes": [
                    { "id": "n1", "kind": "inject",
                      "config": { "repeat_secs": 1.0, "payload": {"i": i} },
                      "outputs": [{ "port": 0, "targets": ["n2"] }] },
                    { "id": "n2", "kind": "debug", "config": {} }
                ]},
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = created["id"]
            .as_i64()
            .unwrap_or_else(|| panic!("{created}"));
        let dep = http
            .post(format!("{api}/api/v1/flows/{id}/deploy"))
            .bearer_auth(&token)
            .header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "version_number": 1 }))
            .send()
            .await
            .unwrap();
        let status = dep.status();
        assert!(
            status.is_success(),
            "deploy {id} via {api}: {status} {}",
            dep.text().await.unwrap()
        );
        flows.push((token, org, id));
    }

    // Sharding: two orgs per worker, each runtime runs exactly its tabs.
    let p = placements(&db).await;
    let on = |w: &str| -> HashSet<i64> {
        flows
            .iter()
            .filter(|(_, org, _)| p.get(org).map(String::as_str) == Some(w))
            .map(|(_, _, id)| *id)
            .collect()
    };
    let (want_a, want_b) = (on("e2e-a"), on("e2e-b"));
    assert_eq!((want_a.len(), want_b.len()), (2, 2), "{p:?}");
    assert_eq!(a.artifact_flows(), want_a);
    assert_eq!(b.artifact_flows(), want_b);
    let (pid_a, pid_b) = (
        a.runtime_pid().expect("runtime a"),
        b.runtime_pid().expect("runtime b"),
    );
    assert!(alive(pid_a) && alive(pid_b));

    // Runtime status answered cross-pod by the owner (real runtime).
    for (token, org, id) in &flows {
        for api in &pods {
            let rt: serde_json::Value = http
                .get(format!("{api}/api/v1/flows/{id}/runtime"))
                .bearer_auth(token)
                .header("X-Org-Id", org.to_string())
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(rt["running"], true, "flow {id} via {api}: {rt}");
            assert_eq!(rt["engine_status"], "running", "flow {id} via {api}: {rt}");
        }
    }

    // SIGKILL pod B: its runtime dies with it (no orphan runtime running
    // flows that are about to move).
    unsafe { libc::kill(b.child.id() as i32, libc::SIGKILL) };
    let _ = b.child.wait();
    wait_for(
        "runtime of pod B gone",
        Duration::from_secs(5),
        || async move { !alive(pid_b) },
    )
    .await;

    // Failover: every org on pod A, which runs every flow.
    let all: HashSet<i64> = flows.iter().map(|(_, _, id)| *id).collect();
    wait_for("orgs of pod B failed over", Duration::from_secs(20), || {
        let db = db.clone();
        async move { placements(&db).await.values().all(|w| w == "e2e-a") }
    })
    .await;
    wait_for("pod A runs every flow", Duration::from_secs(20), || {
        let flows_a = a.artifact_flows();
        let all = all.clone();
        async move { flows_a == all }
    })
    .await;
    for (token, org, id) in &flows {
        let rt: serde_json::Value = http
            .get(format!("{}/api/v1/flows/{id}/runtime", a.url()))
            .bearer_auth(token)
            .header("X-Org-Id", org.to_string())
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            rt["engine_status"], "running",
            "flow {id} after failover: {rt}"
        );
    }

    // SIGTERM pod A: graceful shutdown (no other worker: drain ends at
    // once), runtime stopped, worker row removed.
    let pid_a = a.runtime_pid().expect("runtime a");
    unsafe { libc::kill(a.child.id() as i32, libc::SIGTERM) };
    let start = Instant::now();
    loop {
        if let Some(status) = a.child.try_wait().unwrap() {
            eprintln!("pod A exited: {status}");
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "pod A did not stop"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    wait_for(
        "runtime of pod A gone",
        Duration::from_secs(5),
        || async move { !alive(pid_a) },
    )
    .await;
    let row =
        pnex_backend::models::_entities::flow_workers::Entity::find_by_id("e2e-a".to_string())
            .one(&db)
            .await
            .unwrap();
    assert!(row.is_none(), "pod A deregistered on graceful shutdown");

    drop(db);
    drop(a);
    drop(b);
    let _ = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {db_name} WITH (FORCE)"))
        .await;
}
