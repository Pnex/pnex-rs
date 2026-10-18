//! Browser agents (extension-collector.md D254–D269, lot L0): signed
//! pairing, credential rotation, signed single-use tickets, the Noise link
//! `/ws/extension`, the lease of a target (one agent per target), value
//! reports into telemetry, mission states, viewer read-only (R2) and org
//! isolation (R1).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and Valkey, database emptied
//! between tests.

mod common;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::telemetry::{self, TelemetryPoint, TelemetrySink};
use pnex_core::browser_agent::{self as ba, AgentMsg, MissionState, ServerMsg};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecSink(Mutex<Vec<TelemetryPoint>>);

impl TelemetrySink for RecSink {
    fn send(&self, point: TelemetryPoint) {
        self.0.lock().expect("sink").push(point);
    }
}

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let env = Env {
        alice: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
        bob: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        ),
    };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env).await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PATCH" => server.patch(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// A browser install: its non-extractable key, as WebCrypto holds it.
struct Install {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Install {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Self { pair, rng }
    }

    fn public(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    fn sign(&self, msg: &str) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.sign(&self.rng, msg.as_bytes()).unwrap())
    }

    fn pair_body(&self, org: i64) -> Value {
        let ts = chrono::Utc::now().timestamp_millis();
        json!({
            "install_key": self.public(),
            "fingerprint": { "browser": "Chrome", "os": "Linux", "timezone": "Europe/Paris" },
            "ts": ts,
            "signature": self.sign(&ba::pair_message(org, ts)),
        })
    }
}

struct Paired {
    id: i64,
    device_id: String,
    token: String,
    key: [u8; 32],
}

async fn pair(server: &axum_test::TestServer, user: &str, org: i64, install: &Install) -> Paired {
    let (s, p) = call(
        server,
        "POST",
        "/api/v1/extension/agents",
        user,
        org,
        Some(install.pair_body(org)),
    )
    .await;
    assert_eq!(s, 201, "{p}");
    Paired {
        id: p["agent"]["id"].as_i64().unwrap(),
        device_id: p["agent"]["device_id"].as_str().unwrap().into(),
        token: p["token"].as_str().unwrap().into(),
        key: pnex_core::frame::decode_key(p["encryption_key"].as_str().unwrap()).unwrap(),
    }
}

/// Ticket request with the agent token; returns (status, body).
async fn ticket_with(server: &axum_test::TestServer, token: &str, body: Value) -> (u16, Value) {
    let res = server
        .post("/api/v1/extension/ws-ticket")
        .add_header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

fn ticket_body(install: &Install, device_id: &str) -> Value {
    let ts = chrono::Utc::now().timestamp_millis();
    json!({ "ts": ts, "signature": install.sign(&ba::ticket_message(device_id, ts)) })
}

async fn open(server: &axum_test::TestServer, install: &Install, a: &Paired) -> common::DevWs {
    let (s, t) = ticket_with(server, &a.token, ticket_body(install, &a.device_id)).await;
    assert_eq!(s, 200, "{t}");
    let url = format!("/ws/extension?ticket={}", t["ticket"].as_str().unwrap());
    common::DevWs::connect(server, &url, &a.key, &a.device_id).await
}

async fn send(ws: &mut common::DevWs, msg: &AgentMsg) {
    ws.send_plain(&serde_json::to_string(msg).unwrap()).await;
}

async fn recv(ws: &mut common::DevWs) -> ServerMsg {
    serde_json::from_str(&ws.recv_plain().await).expect("ServerMsg")
}

/// Round trip: every earlier message of the agent was handled.
async fn sync(ws: &mut common::DevWs) {
    send(ws, &AgentMsg::Ping).await;
    assert_eq!(recv(ws).await, ServerMsg::Pong);
}

fn mission_body(name: &str) -> Value {
    json!({
        "name": name,
        "spec": { "kind": "collect_value", "params": {
            "url": "https://example.org/prices#top", "selector": ".price", "metric": "price" } },
        "interval_secs": 60,
    })
}

#[tokio::test]
#[serial]
async fn pairing_is_signed_and_rotates_credentials() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();

        // Signed for another org: refused.
        let body = install.pair_body(org + 1);
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/extension/agents",
            &env.alice,
            org,
            Some(body),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["error"], "extension-pair-refused");

        let first = pair(&server, &env.alice, org, &install).await;
        assert!(first.device_id.starts_with("ext_"));
        // Same install again: same agent, new credentials; the old token
        // no longer gets a ticket.
        let second = pair(&server, &env.alice, org, &install).await;
        assert_eq!(second.id, first.id);
        assert_ne!(second.token, first.token);
        let (s, e) = ticket_with(
            &server,
            &first.token,
            ticket_body(&install, &first.device_id),
        )
        .await;
        assert_eq!(s, 401);
        assert_eq!(e["error"], "extension-ticket-refused");

        // The same install cannot join another org while paired here.
        let bob_org = personal_org(&server, &env.bob).await;
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/extension/agents",
            &env.bob,
            bob_org,
            Some(install.pair_body(bob_org)),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "extension-install-paired-elsewhere");

        // Never from the device wizard, and its token opens no device link.
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/devices",
            &env.alice,
            org,
            Some(json!({ "device_id": "ext_manual", "predefined_device_name": "browser_agent" })),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["error"], "extension-agent-pair-only");
        let mut ws = common::DevWs::connect_with_header(
            &server,
            "/ws/device",
            &second.token,
            &second.key,
            &second.device_id,
        )
        .await;
        match ws.receive_message().await {
            axum_test::WsMessage::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 4001),
            other => panic!("expected 4001, got {other:?}"),
        }

        // A signature opens one ticket only; a copied token alone none.
        let body = ticket_body(&install, &second.device_id);
        let (s, _) = ticket_with(&server, &second.token, body.clone()).await;
        assert_eq!(s, 200);
        let (s, _) = ticket_with(&server, &second.token, body).await;
        assert_eq!(s, 401);
        let thief = Install::new();
        let (s, _) = ticket_with(
            &server,
            &second.token,
            ticket_body(&thief, &second.device_id),
        )
        .await;
        assert_eq!(s, 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn link_holds_the_lease_and_reports_values() {
    with_app(|server, env| async move {
        let rec = Arc::new(RecSink::default());
        telemetry::set_sink(rec.clone());
        let org = personal_org(&server, &env.alice).await;
        let (install_a, install_b) = (Install::new(), Install::new());
        let a = pair(&server, &env.alice, org, &install_a).await;
        let b = pair(&server, &env.alice, org, &install_b).await;
        let (s, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(mission_body("Price")),
        )
        .await;
        assert_eq!(s, 201, "{m}");
        let mission_id = m["id"].as_str().unwrap().to_string();
        assert_eq!(m["target"], "web:example.org/prices");

        // A gets the mission; B, connected after, gets nothing (one agent
        // per target, D257).
        let mut ws_a = open(&server, &install_a, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws_a).await else {
            panic!("missions expected");
        };
        assert_eq!(missions.len(), 1);
        assert_eq!(missions[0].id, mission_id);
        let mut ws_b = open(&server, &install_b, &b).await;
        assert_eq!(
            recv(&mut ws_b).await,
            ServerMsg::Missions { missions: vec![] }
        );

        // B cannot report on a mission it does not hold.
        send(
            &mut ws_b,
            &AgentMsg::Report {
                mission_id: mission_id.clone(),
                value: "1".into(),
            },
        )
        .await;
        sync(&mut ws_b).await;
        send(
            &mut ws_a,
            &AgentMsg::Report {
                mission_id: mission_id.clone(),
                value: " 42.5 ".into(),
            },
        )
        .await;
        sync(&mut ws_a).await;
        let points = rec.0.lock().unwrap().clone();
        assert_eq!(points.len(), 1, "{points:?}");
        assert_eq!(points[0].metric_name, "price");
        assert_eq!(points[0].value, "42.5");
        assert_eq!(points[0].device_id, a.device_id);

        let path = format!("/api/v1/extension/missions/{mission_id}");
        send(
            &mut ws_a,
            &AgentMsg::Report {
                mission_id: mission_id.clone(),
                value: "n/a".into(),
            },
        )
        .await;
        sync(&mut ws_a).await;
        let (_, got) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(got["state"], "running");
        assert_eq!(got["state_code"], "ext-value-not-numeric");
        send(
            &mut ws_a,
            &AgentMsg::State {
                mission_id: mission_id.clone(),
                state: MissionState::Blocked,
                code: Some("free text from the agent".into()),
            },
        )
        .await;
        sync(&mut ws_a).await;
        let (_, got) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(got["state"], "blocked");
        assert!(
            got.get("state_code").is_none(),
            "unknown code stored: {got}"
        );

        let (_, agents) = call(
            &server,
            "GET",
            "/api/v1/extension/agents",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(agents["count"], 2);
        assert!(agents["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["online"] == true));

        // Optimistic concurrency on the version read.
        let (s, e) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(json!({ "expected_version": 0, "enabled": false })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "extension-mission-version-conflict");
        let (s, got) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(json!({ "expected_version": 1, "interval_secs": 120 })),
        )
        .await;
        assert_eq!(s, 200, "{got}");
        assert_eq!(got["state"], "idle");
        let (s, e) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(json!({ "expected_version": 2, "interval_secs": 10 })),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["errors"]["fields"]["interval_secs"], "invalid", "{e}");
        telemetry::reset_sink();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn viewer_reads_and_other_orgs_see_nothing() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let (s, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(mission_body("Price")),
        )
        .await;
        assert_eq!(s, 201);
        let path = format!("/api/v1/extension/missions/{}", m["id"].as_str().unwrap());

        let (s, e) = call(&server, "GET", &path, &env.bob, bob_org, None).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "extension-mission-not-found");
        let (_, list) = call(
            &server,
            "GET",
            "/api/v1/extension/missions",
            &env.bob,
            bob_org,
            None,
        )
        .await;
        assert_eq!(list["count"], 0);

        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({ "email": "bob@example.com", "role": "viewer" })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s}");
        let (s, _) = call(&server, "GET", &path, &env.bob, org, None).await;
        assert_eq!(s, 200);
        for (method, p, body) in [
            (
                "POST",
                "/api/v1/extension/missions".to_string(),
                Some(mission_body("V")),
            ),
            (
                "PATCH",
                path.clone(),
                Some(json!({ "expected_version": 1, "enabled": false })),
            ),
            ("DELETE", path.clone(), None),
            (
                "POST",
                "/api/v1/extension/agents".to_string(),
                Some(Install::new().pair_body(org)),
            ),
            (
                "PATCH",
                "/api/v1/extension/agents/1".to_string(),
                Some(json!({ "name": "x" })),
            ),
        ] {
            let (s, e) = call(&server, method, &p, &env.bob, org, body).await;
            assert_eq!(s, 403, "{method} {p}");
            assert_eq!(e["error"], "extension-write-forbidden");
        }
    })
    .await;
}

/// Signed page upload: (status, body).
async fn upload_page(
    server: &axum_test::TestServer,
    install: &Install,
    a: &Paired,
    body: &Value,
) -> (u16, Value, String) {
    use sha2::Digest as _;
    let raw = serde_json::to_vec(body).unwrap();
    let sha: String = sha2::Sha256::digest(&raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ts = chrono::Utc::now().timestamp_millis();
    let mission = body["mission_id"].as_str().unwrap();
    let signature = install.sign(&ba::upload_message(&a.device_id, ts, mission, &sha));
    let (status, got) = upload_signed(server, a, &raw, ts, &signature).await;
    (status, got, signature)
}

async fn upload_signed(
    server: &axum_test::TestServer,
    a: &Paired,
    raw: &[u8],
    ts: i64,
    signature: &str,
) -> (u16, Value) {
    let res = server
        .post("/api/v1/extension/uploads/page")
        .add_header("Authorization", format!("Bearer {}", a.token))
        .add_header("X-Pnex-Ts", ts.to_string())
        .add_header("X-Pnex-Signature", signature.to_string())
        .bytes(raw.to_vec().into())
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial]
async fn page_capture_adds_a_version_only_when_the_text_changes() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(json!({
                "name": "News front page",
                "spec": { "kind": "collect_page", "params": { "url": "https://example.org/news" } },
                "interval_secs": 300,
            })),
        )
        .await;
        assert_eq!(s, 201, "{m}");
        let mission_id = m["id"].as_str().unwrap().to_string();
        let page = |text: &str| {
            json!({ "mission_id": mission_id, "url": "https://example.org/news", "title": "News", "text": text })
        };

        let (s, first, signature) = upload_page(&server, &install, &a, &page("Headline one")).await;
        assert_eq!(s, 201, "{first}");
        assert_eq!(first["version"], 1);
        assert_eq!(first["changed"], true);
        let (s, same, _) = upload_page(&server, &install, &a, &page("Headline one")).await;
        assert_eq!(s, 201);
        assert_eq!(same["changed"], false);
        assert_eq!(same["asset_id"], first["asset_id"]);
        let (s, next, _) = upload_page(&server, &install, &a, &page("Headline two")).await;
        assert_eq!(s, 201);
        assert_eq!(next["version"], 2);

        // A replayed signature, or a body that is not the signed one, is refused.
        let raw = serde_json::to_vec(&page("Headline one")).unwrap();
        let ts = chrono::Utc::now().timestamp_millis();
        let (s, e) = upload_signed(&server, &a, &raw, ts, &signature).await;
        assert_eq!(s, 401);
        assert_eq!(e["error"], "extension-upload-refused");

        // The document is a Markdown version of the mission's asset.
        let asset = first["asset_id"].as_str().unwrap();
        let (s, media) = call(&server, "GET", &format!("/api/v1/media/{asset}"), &env.alice, org, None).await;
        assert_eq!(s, 200, "{media}");
        assert_eq!(media["kind"], "document");

        // Only a page mission takes pages.
        let (_, value) = call(&server, "POST", "/api/v1/extension/missions", &env.alice, org, Some(mission_body("Price"))).await;
        let mut other = page("x");
        other["mission_id"] = value["id"].clone();
        let (s, e, _) = upload_page(&server, &install, &a, &other).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "extension-mission-not-found");
    })
    .await;
}

fn post(id: &str, video: bool) -> ba::SeenPost {
    ba::SeenPost {
        platform_id: id.into(),
        url: format!("https://x.com/some_account/status/{id}"),
        author: "some_account".into(),
        text: format!("post {id}"),
        published_at: Some("2026-10-18T08:00:00Z".into()),
        video_url: video.then(|| format!("https://video.example/{id}.mp4")),
        duration_ms: video.then_some(12_000),
    }
}

async fn upload_video(
    server: &axum_test::TestServer,
    install: &Install,
    a: &Paired,
    mission: &str,
    post_id: &str,
    raw: &[u8],
) -> (u16, Value) {
    use sha2::Digest as _;
    let sha: String = sha2::Sha256::digest(raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ts = chrono::Utc::now().timestamp_millis();
    let signature = install.sign(&ba::video_upload_message(
        &a.device_id,
        ts,
        mission,
        post_id,
        &sha,
    ));
    let res = server
        .post("/api/v1/extension/uploads/video")
        .add_header("Authorization", format!("Bearer {}", a.token))
        .add_header("X-Pnex-Ts", ts.to_string())
        .add_header("X-Pnex-Signature", signature)
        .add_header("X-Pnex-Mission", mission.to_string())
        .add_header("X-Pnex-Post", post_id.to_string())
        .bytes(raw.to_vec().into())
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// Signed JSON upload bound to a post (captions).
async fn upload_json(
    server: &axum_test::TestServer,
    install: &Install,
    a: &Paired,
    path: &str,
    body: &Value,
    post_id: &str,
) -> (u16, Value) {
    use sha2::Digest as _;
    let raw = serde_json::to_vec(body).unwrap();
    let sha: String = sha2::Sha256::digest(&raw)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let ts = chrono::Utc::now().timestamp_millis();
    let mission = body["mission_id"].as_str().unwrap();
    let signature = install.sign(&ba::captions_upload_message(
        &a.device_id,
        ts,
        mission,
        post_id,
        &sha,
    ));
    let (status, got) = upload_signed_to(server, path, a, &raw, ts, &signature).await;
    (status, got)
}

async fn upload_signed_to(
    server: &axum_test::TestServer,
    path: &str,
    a: &Paired,
    raw: &[u8],
    ts: i64,
    signature: &str,
) -> (u16, Value) {
    let res = server
        .post(path)
        .add_header("Authorization", format!("Bearer {}", a.token))
        .add_header("X-Pnex-Ts", ts.to_string())
        .add_header("X-Pnex-Signature", signature.to_string())
        .bytes(raw.to_vec().into())
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial]
async fn followed_account_videos_are_captured_once() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(json!({
                "name": "Account",
                "spec": { "kind": "follow_account", "params": { "platform": "x", "handle": "@Some_Account", "force_video": true } },
                "interval_secs": 600,
            })),
        )
        .await;
        assert_eq!(s, 201, "{m}");
        assert_eq!(m["target"], "x:@some_account");
        let mission = m["id"].as_str().unwrap().to_string();

        let mut ws = open(&server, &install, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws).await else {
            panic!("missions expected");
        };
        assert_eq!(missions.len(), 1);
        let seen = AgentMsg::Seen {
            mission_id: mission.clone(),
            posts: vec![post("101", true), post("102", false), post("../x", true)],
            depth: 20,
            end_reached: false,
        };
        send(&mut ws, &seen).await;
        assert_eq!(
            recv(&mut ws).await,
            ServerMsg::Capture {
                mission_id: mission.clone(),
                platform_ids: vec!["101".into()],
                captions: vec!["101".into()],
            },
            "only the new video, never the malformed id"
        );
        // Seen again (by any agent): nothing to capture twice.
        send(&mut ws, &seen).await;
        assert_eq!(
            recv(&mut ws).await,
            ServerMsg::Capture {
                mission_id: mission.clone(),
                platform_ids: vec![],
                captions: vec![],
            }
        );

        // D283: the next visit goes deeper; once the list ended, recent only.
        ws.close().await;
        let mut ws = open(&server, &install, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws).await else {
            panic!("missions expected");
        };
        assert_eq!(missions[0].scroll_depth, 2 * ba::HISTORY_STEP);
        send(
            &mut ws,
            &AgentMsg::Seen {
                mission_id: mission.clone(),
                posts: vec![],
                depth: 23,
                end_reached: true,
            },
        )
        .await;
        recv(&mut ws).await;
        ws.close().await;
        let mut ws = open(&server, &install, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws).await else {
            panic!("missions expected");
        };
        assert_eq!(missions[0].scroll_depth, ba::RECENT_DEPTH);
        ws.close().await;

        // Captions of a post: no text store in tests, so a coded 503 and
        // nothing recorded; a post without video has no captions to take.
        let captions = |id: &str| {
            json!({ "mission_id": mission, "platform_id": id, "lang": "fr",
                    "cues": [{ "start_ms": 0, "end_ms": 1500, "text": "bonjour" }] })
        };
        let (s, e) = upload_json(&server, &install, &a, "/api/v1/extension/uploads/captions", &captions("101"), "101").await;
        assert_eq!(s, 503, "{e}");
        assert_eq!(e["error"], "media-transcripts-unavailable");
        let (s, _) = upload_json(&server, &install, &a, "/api/v1/extension/uploads/captions", &captions("102"), "102").await;
        assert_eq!(s, 404);

        let mp4 = b"\0\0\0\x18ftypisom\0\0\x02\0isommp41";
        let (s, v) = upload_video(&server, &install, &a, &mission, "101", mp4).await;
        assert_eq!(s, 201, "{v}");
        assert_eq!(v["state"], "captured");
        let asset = v["media_asset_id"].as_str().unwrap().to_string();
        let (s, media) = call(&server, "GET", &format!("/api/v1/media/{asset}"), &env.alice, org, None).await;
        assert_eq!(s, 200, "{media}");
        assert_eq!(media["kind"], "video");
        // Captured once: a second upload, or a post without video, is refused.
        let (s, e) = upload_video(&server, &install, &a, &mission, "101", mp4).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "extension-video-not-found");
        let (s, _) = upload_video(&server, &install, &a, &mission, "102", mp4).await;
        assert_eq!(s, 404);

        let (_, list) = call(&server, "GET", "/api/v1/extension/videos", &env.alice, org, None).await;
        assert_eq!(list["count"], 2, "{list}");
        // Another org sees no registry.
        let bob_org = personal_org(&server, &env.bob).await;
        let (_, other) = call(&server, "GET", "/api/v1/extension/videos", &env.bob, bob_org, None).await;
        assert_eq!(other["count"], 0);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn live_parts_land_as_video_media() {
    with_app(|server, env| async move {
        use sha2::Digest as _;
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(json!({
                "name": "Morning show",
                "spec": { "kind": "watch_live", "params": { "url": "https://www.youtube.com/watch?v=live1234567" } },
                "interval_secs": 300,
            })),
        )
        .await;
        assert_eq!(s, 201, "{m}");
        assert!(m["target"].as_str().unwrap().starts_with("youtube:live:"));
        let mission = m["id"].as_str().unwrap().to_string();
        let send = |raw: Vec<u8>, part: &str| {
            let sha: String = sha2::Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
            let ts = chrono::Utc::now().timestamp_millis();
            let signature = install.sign(&ba::live_upload_message(
                &a.device_id,
                ts,
                &mission,
                part.parse().unwrap_or(0),
                &sha,
            ));
            server
                .post("/api/v1/extension/uploads/live")
                .add_header("Authorization", format!("Bearer {}", a.token))
                .add_header("X-Pnex-Ts", ts.to_string())
                .add_header("X-Pnex-Signature", signature)
                .add_header("X-Pnex-Mission", mission.clone())
                .add_header("X-Pnex-Part", part.to_string())
                .add_header("X-Pnex-Started", "2026-10-18T07:00:00Z")
                .bytes(raw.into())
        };
        let webm = b"\x1A\x45\xDF\xA3\x9F\x42\x86\x81\x01\x42\x82\x84webm\x42\x87".to_vec();
        let res = send(webm.clone(), "1").await;
        assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
        let stored: Value = res.json();
        assert_eq!(stored["part"], 1);
        let asset = stored["asset_id"].as_str().unwrap();
        let (_, media) = call(&server, "GET", &format!("/api/v1/media/{asset}"), &env.alice, org, None).await;
        assert_eq!(media["kind"], "video", "{media}");
        assert_eq!(media["metadata"]["live"], true);
        // An mp4, or a part of another kind of mission, is refused.
        let res = send(b"\0\0\0\x18ftypisom\0\0\x02\0isommp41".to_vec(), "2").await;
        assert_eq!(res.status_code().as_u16(), 400);
    })
    .await;
}
