//! A real server process killed (SIGKILL) in the middle of an import, then a new one:
//! nothing half written, the chained run starts it again. Then a real SIGTERM. The 10
//! minutes without a heartbeat are skipped in SQL, the rest is what a lost pod does.
//!
//! Needs PNEX_SERVER_BIN, PostgreSQL (TEST_DATABASE_URL), the compose `rustfs` and `valkey`.

mod common;

use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Value};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const PORT: u16 = 5197;
const ROWS: usize = 150_000;
const MAPPING: &str = r#"
key: certs
name: Certifications
objects:
  - as: cert
    type: certification
    key: code
    title: { from: title }
    properties:
      code: { from: code, convert: [trim, upper] }
"#;

/// Never outlives a failed assertion: it holds the port and the leases.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start(bin: &str, rauthy: &str, truncate: bool) -> Server {
    let child = Command::new(bin)
        .args([
            "start",
            "--binding",
            "127.0.0.1",
            "--port",
            &PORT.to_string(),
        ])
        .env("LOCO_ENV", "test")
        .env("RAUTHY_URL", rauthy)
        .env("PNEX_FLOW_ENABLED", "false")
        .env("PNEX_TEST_TRUNCATE", truncate.to_string())
        .spawn()
        .unwrap();
    let server = Server(child);
    for _ in 0..240 {
        if reqwest::get(format!("http://127.0.0.1:{PORT}/_ping"))
            .await
            .is_ok()
        {
            return server;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("server never answered");
}

struct Api {
    token: String,
    org: i64,
}

impl Api {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, format!("http://127.0.0.1:{PORT}{path}"))
            .bearer_auth(&self.token)
            .header("X-Org-Id", self.org.to_string())
    }

    async fn post(&self, path: &str, body: Option<Value>) -> Value {
        let mut request = self.request(reqwest::Method::POST, path);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "POST {path}: {status} {body}");
        serde_json::from_str(&body).unwrap_or(Value::Null)
    }

    /// A new server may not answer yet, or answer 503 while it starts.
    async fn build(&self, id: &str) -> Option<Value> {
        let response = self
            .request(reqwest::Method::GET, &format!("/api/v1/data-builds/{id}"))
            .send()
            .await
            .ok()?;
        response.status().is_success().then_some(())?;
        response.json().await.ok()
    }
}

async fn count(db: &DatabaseConnection, query: String) -> i64 {
    let row = db
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, query))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "n").unwrap()
}

#[tokio::test]
#[ignore = "needs the server binary in PNEX_SERVER_BIN"]
async fn a_killed_server_loses_nothing_and_a_sigterm_stops_it() {
    let bin = std::env::var("PNEX_SERVER_BIN").expect("PNEX_SERVER_BIN");
    let rauthy = common::spawn_mock_rauthy().await;
    let mut server = start(&bin, &rauthy, true).await;
    let db = Database::connect(std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    common::seed_catalogue(&db).await;

    let token = common::valid_token(
        &rauthy,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let info: Value = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{PORT}/api/v1/user-info"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let org = info["orgs"][0]["id"].as_i64().unwrap();
    let api = Api { token, org };
    api.post("/api/v1/ontology/packs/quali_watch/install", None)
        .await;
    let table = api
        .post("/api/v1/datasets", Some(json!({ "name": "certs_table" })))
        .await;
    let table = table["id"].as_str().unwrap().to_string();
    let csv = (0..ROWS).fold(String::from("code;title\n"), |mut all, n| {
        all.push_str(&format!("RS{n};Title {n}\n"));
        all
    });
    let response = api
        .request(
            reqwest::Method::POST,
            &format!("/api/v1/datasets/{table}/transactions/csv"),
        )
        .body(csv)
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let import = api
        .post(
            "/api/v1/ontology/imports",
            Some(
                json!({ "name": "certs", "mapping": MAPPING, "source": "dataset",
                         "source_url": table, "mode": "incremental" }),
            ),
        )
        .await;
    let objects = format!("SELECT count(*) AS n FROM objects WHERE org_id = {org}");
    let before = count(&db, objects.clone()).await;
    let build = api
        .post(
            "/api/v1/data-builds",
            Some(json!({ "target_kind": "import", "target_id": import["id"] })),
        )
        .await;
    let build = build["id"].as_str().unwrap().to_string();

    // The server's own loop takes the build.
    let started = Instant::now();
    let lost = loop {
        let state = api.build(&build).await.unwrap();
        if let Some(run) = state["steps"][0]["run_id"].as_str() {
            break run.to_string();
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "never started: {state}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    let running = format!(
        "SELECT count(*) AS n FROM object_import_runs WHERE id = '{lost}' AND state = 'running'"
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    assert_eq!(count(&db, running.clone()).await, 1, "killed too late");
    assert_eq!(
        count(&db, objects.clone()).await,
        before,
        "half an import is written"
    );

    // What 10 minutes of silence would do, and the new pod takes over.
    db.execute_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        format!(
            "UPDATE object_import_runs SET heartbeat_at = now() - interval '11 minutes' \
             WHERE id = '{lost}'"
        ),
    ))
    .await
    .unwrap();
    let mut server = start(&bin, &rauthy, false).await;
    let started = Instant::now();
    let done = loop {
        if let Some(state) = api.build(&build).await {
            if state["state"] != "running" && state["state"] != "queued" {
                break state;
            }
            assert!(
                started.elapsed() < Duration::from_secs(400),
                "never ended: {state}"
            );
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["steps"][0]["retries"], 1, "{done}");
    assert_ne!(done["steps"][0]["run_id"], lost.as_str(), "{done}");
    assert_eq!(count(&db, objects).await, before + ROWS as i64);
    assert_eq!(
        count(
            &db,
            format!(
                "SELECT count(*) AS n FROM object_import_runs WHERE id = '{lost}' \
                 AND state = 'failed' AND state_code = 'import-run-interrupted'"
            )
        )
        .await,
        1
    );

    // SIGTERM with nothing running: a clean exit, the leases given back.
    let pid = server.0.id().to_string();
    assert!(Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap()
        .success());
    let asked = Instant::now();
    let status = loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(60),
            "SIGTERM not honoured"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(status.success(), "{status}");
}
