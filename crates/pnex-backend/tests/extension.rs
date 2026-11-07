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
    db: sea_orm::DatabaseConnection,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let (alice, bob) = (
        common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
        common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        ),
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            let db = ctx.db.clone();
            f(server, Env { alice, bob, db }).await;
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

        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
        .await;
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
            assert_eq!(e["error"], "access-denied");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_caps_on_agents_and_missions() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        for _ in 0..pnex_core::browser_agent::AGENTS_MAX {
            pair(&server, &env.alice, org, &Install::new()).await;
        }
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/extension/agents",
            &env.alice,
            org,
            Some(Install::new().pair_body(org)),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "extension-limit-reached");
        assert_eq!(e["errors"]["args"]["max"], "20");

        for i in 0..pnex_core::browser_agent::MISSIONS_MAX {
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/extension/missions",
                &env.alice,
                org,
                Some(mission_body(&format!("M{i}"))),
            )
            .await;
            assert_eq!(s, 201);
        }
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(mission_body("One too many")),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "extension-limit-reached");
    })
    .await;
}

/// The other valid encoding `(r, n−s)` of a P-256 signature `r‖s`.
fn negated_s(sig: &str) -> String {
    const N: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63,
        0x25, 0x51,
    ];
    let mut raw = URL_SAFE_NO_PAD.decode(sig).unwrap();
    let mut borrow = 0i16;
    for i in (0..32).rev() {
        let d = i16::from(N[i]) - i16::from(raw[32 + i]) - borrow;
        borrow = i16::from(d < 0);
        raw[32 + i] = (d + 256 * borrow) as u8;
    }
    URL_SAFE_NO_PAD.encode(raw)
}

#[tokio::test]
#[serial]
async fn a_full_registry_blocks_account_missions() {
    use sea_orm::ConnectionTrait as _;
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (_, m) = call(
            &server,
            "POST",
            "/api/v1/extension/missions",
            &env.alice,
            org,
            Some(json!({
                "name": "Account",
                "spec": { "kind": "follow_account", "params": { "platform": "x", "handle": "@registry_full" } },
                "interval_secs": 600,
            })),
        )
        .await;
        let mission = m["id"].as_str().unwrap().to_string();
        env.db
            .execute_unprepared(&format!(
                "INSERT INTO social_videos (id, org_id, platform, platform_id, url, author, text)
                 SELECT gen_random_uuid(), {org}, 'x', g::text, 'https://x.com/a', 'a', ''
                 FROM generate_series(1, {}) g",
                ba::VIDEOS_MAX
            ))
            .await
            .unwrap();

        let mut ws = open(&server, &install, &a).await;
        recv(&mut ws).await;
        send(
            &mut ws,
            &AgentMsg::Seen {
                mission_id: mission.clone(),
                posts: vec![post("999999999", true)],
                depth: 20,
                end_reached: false,
            },
        )
        .await;
        assert_eq!(
            recv(&mut ws).await,
            ServerMsg::Capture {
                mission_id: mission.clone(),
                platform_ids: vec![],
                captions: vec![],
            }
        );
        let (_, got) = call(&server, "GET", &format!("/api/v1/extension/missions/{mission}"), &env.alice, org, None).await;
        assert_eq!(got["state"], "blocked");
        assert_eq!(got["state_code"], "ext-registry-full");
        // The database outlives the test: leave the registry empty.
        env.db
            .execute_unprepared(&format!("DELETE FROM social_videos WHERE org_id = {org}"))
            .await
            .unwrap();
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

        // Same request again, as sent then with its signature in the other
        // valid form (r, n−s): the second use is refused either way.
        use sha2::Digest as _;
        let raw = serde_json::to_vec(&page("Headline three")).unwrap();
        let sha: String = sha2::Sha256::digest(&raw).iter().map(|b| format!("{b:02x}")).collect();
        let ts = chrono::Utc::now().timestamp_millis();
        let sig = install.sign(&ba::upload_message(&a.device_id, ts, &mission_id, &sha));
        let (s, _) = upload_signed(&server, &a, &raw, ts, &sig).await;
        assert_eq!(s, 201);
        for replay in [sig.clone(), format!("{sig} "), negated_s(&sig)] {
            let (s, _) = upload_signed(&server, &a, &raw, ts, &replay).await;
            assert_eq!(s, 401, "{replay}");
        }

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
    upload_video_part(server, install, a, mission, post_id, None, raw).await
}

/// Video upload with `X-Pnex-Part` when `part` is set (signed as 1 when not).
async fn upload_video_part(
    server: &axum_test::TestServer,
    install: &Install,
    a: &Paired,
    mission: &str,
    post_id: &str,
    part: Option<u32>,
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
        part.unwrap_or(1),
        &sha,
    ));
    let mut req = server.post("/api/v1/extension/uploads/video");
    if let Some(part) = part {
        req = req.add_header("X-Pnex-Part", part.to_string());
    }
    let res = req
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

        // O92: a long recording goes on in parts, each its own media.
        let webm = b"\x1A\x45\xDF\xA3\x9F\x42\x86\x81\x01\x42\x82\x84webm\x42\x87";
        let (s, v2) = upload_video_part(&server, &install, &a, &mission, "101", Some(2), webm).await;
        assert_eq!(s, 201, "{v2}");
        assert_eq!(v2["media_asset_id"], v["media_asset_id"], "the registry keeps part 1");
        let (_, media) = call(&server, "GET", "/api/v1/media?kind=video&limit=100", &env.alice, org, None).await;
        let part2 = media["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["metadata"]["part"] == "2")
            .expect("part 2 media");
        assert_eq!(part2["metadata"]["part_of"], asset.as_str());
        let (s, _) = upload_video_part(&server, &install, &a, &mission, "101", Some(2), webm).await;
        assert_eq!(s, 404, "a part is stored once");
        let (s, _) = upload_video_part(&server, &install, &a, &mission, "102", Some(2), webm).await;
        assert_eq!(s, 404, "no part before part 1");
        let (s, _) = upload_video_part(&server, &install, &a, &mission, "101", Some(17), webm).await;
        assert_eq!(s, 400);

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

#[tokio::test]
#[serial]
async fn exclusive_platforms_go_to_dedicated_agents_only() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let mission = |name: &str, platform: &str| {
            json!({
                "name": name,
                "spec": { "kind": "follow_account", "params": { "platform": platform, "handle": "acc" } },
                "interval_secs": 600,
            })
        };
        for (name, platform) in [("tt", "tiktok"), ("xx", "x")] {
            let (s, m) = call(&server, "POST", "/api/v1/extension/missions", &env.alice, org, Some(mission(name, platform))).await;
            assert_eq!(s, 201, "{m}");
        }
        let held = |ws_missions: Vec<ba::AssignedMission>| {
            let mut t: Vec<String> = ws_missions.into_iter().map(|m| m.target).collect();
            t.sort();
            t
        };
        // Shared (default): X, never TikTok.
        let mut ws = open(&server, &install, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws).await else {
            panic!("missions expected");
        };
        assert_eq!(held(missions), ["x:@acc"]);
        ws.close().await;

        let path = format!("/api/v1/extension/agents/{}", a.id);
        let (s, e) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({ "mode": "dedicated:web" }))).await;
        assert_eq!(s, 400);
        assert_eq!(e["errors"]["fields"]["mode"], "invalid", "{e}");
        let (s, agent) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({ "mode": "dedicated:tiktok" }))).await;
        assert_eq!(s, 200, "{agent}");
        assert_eq!(agent["mode"], "dedicated:tiktok");
        let mut ws = open(&server, &install, &a).await;
        let ServerMsg::Missions { missions } = recv(&mut ws).await else {
            panic!("missions expected");
        };
        assert_eq!(held(missions), ["tiktok:@acc"]);
        ws.close().await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_profile_capture_lands_on_the_person_once_and_respects_objection() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/investigation/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200);
        // The layer that reads captured profiles (account-mapping.md D538).
        let (s, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/prospecting_b2b/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200);
        let (s, ada) = call(
            &server,
            "POST",
            "/api/v1/ontology/objects",
            &env.alice,
            org,
            Some(json!({ "type_key": "person", "title": "Ada", "properties": {} })),
        )
        .await;
        assert_eq!(s, 201, "{ada}");
        let ada = ada["id"].as_str().unwrap().to_string();
        let captures = format!("/api/v1/extension/people/{ada}/profile-captures");
        let ask = |url: &str| json!({ "url": url });

        // Discovery pages and unknown sites are refused.
        let (s, e) = call(
            &server,
            "POST",
            &captures,
            &env.alice,
            org,
            Some(ask(
                "https://www.linkedin.com/search/results/people/?keywords=ada",
            )),
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (400, Some("ext-target-not-profile")),
            "{e}"
        );
        let (s, _) = call(
            &server,
            "POST",
            &captures,
            &env.alice,
            org,
            Some(ask("https://example.org/ada")),
        )
        .await;
        assert_eq!(s, 400);

        // One capture, normalised; asking again returns the open one.
        let (s, m) = call(
            &server,
            "POST",
            &captures,
            &env.alice,
            org,
            Some(ask("https://fr.linkedin.com/in/Ada-L/")),
        )
        .await;
        assert_eq!(s, 201, "{m}");
        assert_eq!(m["url"], "https://www.linkedin.com/in/ada-l");
        let (_, again) = call(
            &server,
            "POST",
            &captures,
            &env.alice,
            org,
            Some(ask("https://www.linkedin.com/in/ada-l")),
        )
        .await;
        assert_eq!(again["id"], m["id"]);

        let (s, listed) = call(&server, "GET", &captures, &env.alice, org, None).await;
        assert_eq!(s, 200, "{listed}");
        assert_eq!(listed["count"], 1);
        assert_eq!(
            (
                &listed["results"][0]["open"],
                &listed["results"][0]["captured"]
            ),
            (&json!(true), &json!(false))
        );
        let (s, _) = call(
            &server,
            "GET",
            "/api/v1/extension/people/00000000-0000-0000-0000-000000000000/profile-captures",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 404);

        // The agent plays the default rule of the network (D780): a document
        // linked to Ada, the address an account of Ada.
        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else {
            panic!("the capture is a rule run")
        };
        assert_eq!(runs[0].run_id, m["id"].as_str().unwrap());
        assert!(runs[0].recipe.needs_session());
        assert_eq!(runs[0].params["url"], "https://www.linkedin.com/in/ada-l");
        assert_eq!(runs[0].params["path"], "/in/ada-l");
        play(
            &mut ws,
            &runs[0].run_id,
            "profile",
            "Ada L — CTO at ACME",
            "https://www.linkedin.com/in/ada-l",
        )
        .await;
        let (_, links) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ada}/links"),
            &env.alice,
            org,
            None,
        )
        .await;
        let links = links.to_string();
        assert!(
            links.contains("uses_identifier") && links.contains("published_by"),
            "{links}"
        );
        let (_, person) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ada}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            person["properties"]["profile_to_read"]
                .as_str()
                .map(str::len),
            Some(36),
            "the document to read: {person}"
        );
        let (_, listed) = call(&server, "GET", &captures, &env.alice, org, None).await;
        assert_eq!(
            (
                &listed["results"][0]["open"],
                &listed["results"][0]["captured"]
            ),
            (&json!(false), &json!(true))
        );
        drop(ws);

        // A viewer may not capture nor record an objection (R2).
        personal_org(&server, &env.bob).await;
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
        .await;
        let opposition = format!("/api/v1/extension/people/{ada}/opposition");
        for p in [captures.clone(), opposition.clone()] {
            let (s, e) = call(
                &server,
                "POST",
                &p,
                &env.bob,
                org,
                Some(ask("https://x.com/ada")),
            )
            .await;
            assert_eq!(
                (s, e["error"].as_str()),
                (403, Some("access-denied")),
                "{p}"
            );
        }

        // The objection covers the person and its known profile addresses.
        let (s, o) = call(&server, "POST", &opposition, &env.alice, org, None).await;
        assert_eq!((s, o["opposed"].as_bool()), (200, Some(true)));
        let (_, o) = call(&server, "GET", &opposition, &env.bob, org, None).await;
        assert_eq!(o["opposed"], true);
        let (s, e) = call(
            &server,
            "POST",
            &captures,
            &env.alice,
            org,
            Some(ask("https://x.com/ada")),
        )
        .await;
        assert_eq!((s, e["error"].as_str()), (409, Some("prospecting-opposed")));
        let (s, eve) = call(
            &server,
            "POST",
            "/api/v1/ontology/objects",
            &env.alice,
            org,
            Some(json!({ "type_key": "person", "title": "Eve", "properties": {} })),
        )
        .await;
        assert_eq!(s, 201);
        let (s, e) = call(
            &server,
            "POST",
            &format!(
                "/api/v1/extension/people/{}/profile-captures",
                eve["id"].as_str().unwrap()
            ),
            &env.alice,
            org,
            Some(ask("https://www.linkedin.com/in/ada-l")),
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (409, Some("prospecting-opposed")),
            "Ada's address stays refused"
        );

        // Daily cap per network.
        let eve_captures = format!(
            "/api/v1/extension/people/{}/profile-captures",
            eve["id"].as_str().unwrap()
        );
        for i in 0..ba::PROFILE_CAPTURES_PER_DAY - 1 {
            let (s, e) = call(
                &server,
                "POST",
                &eve_captures,
                &env.alice,
                org,
                Some(ask(&format!("https://www.linkedin.com/in/eve-{i}"))),
            )
            .await;
            assert_eq!(s, 201, "{i} {e}");
        }
        let (s, e) = call(
            &server,
            "POST",
            &eve_captures,
            &env.alice,
            org,
            Some(ask("https://www.linkedin.com/in/eve-last")),
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (409, Some("ext-profile-daily-cap")),
            "{e}"
        );
        let (s, _) = call(
            &server,
            "POST",
            &eve_captures,
            &env.alice,
            org,
            Some(ask("https://x.com/eve")),
        )
        .await;
        assert_eq!(s, 201, "the cap is per network");
    })
    .await;
}

/// D779: an employee list is asked from an organization, lands on it to be
/// read, and its people never bring back someone who objected there.
#[tokio::test]
#[serial]
async fn an_employee_list_lands_on_its_company_and_skips_objections() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, _) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert_eq!(s, 200);
        let (s, _) = call(&server, "POST", "/api/v1/ontology/packs/prospecting_b2b/install", &env.alice, org, None).await;
        assert_eq!(s, 200);
        let new = |type_key: &str, title: &str| json!({ "type_key": type_key, "title": title, "properties": {} });
        let (s, acme) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(new("organization", "ACME"))).await;
        assert_eq!(s, 201, "{acme}");
        let acme = acme["id"].as_str().unwrap().to_string();
        let captures = format!("/api/v1/extension/organizations/{acme}/people-captures");
        let ask = |url: &str| json!({ "url": url });

        for url in ["https://www.linkedin.com/search/results/people/?keywords=acme", "https://www.linkedin.com/company/acme/", "https://www.linkedin.com/in/jane"] {
            let (s, e) = call(&server, "POST", &captures, &env.alice, org, Some(ask(url))).await;
            assert_eq!((s, e["error"].as_str()), (400, Some("ext-target-not-people-page")), "{url} {e}");
        }
        let (s, _) = call(&server, "POST", &captures, &env.alice, org, Some(ask("https://example.org/company/acme/people/"))).await;
        assert_eq!(s, 400);
        let (s, jane) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(new("person", "Jane Doe"))).await;
        assert_eq!(s, 201);
        let jane = jane["id"].as_str().unwrap().to_string();
        let (s, _) = call(&server, "POST", &format!("/api/v1/extension/organizations/{jane}/people-captures"), &env.alice, org, Some(ask("https://www.linkedin.com/company/acme/people/"))).await;
        assert_eq!(s, 404, "a person has no employee list");

        let (s, m) = call(&server, "POST", &captures, &env.alice, org, Some(ask("https://fr.linkedin.com/company/ACME/people/?viewAsMember=true"))).await;
        assert_eq!(s, 201, "{m}");
        assert_eq!(m["url"], "https://www.linkedin.com/company/acme/people/");
        let (_, again) = call(&server, "POST", &captures, &env.alice, org, Some(ask("https://www.linkedin.com/company/acme/people/"))).await;
        assert_eq!(again["id"], m["id"]);

        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else { panic!("the capture is a rule run") };
        assert_eq!(runs[0].params["url"], "https://www.linkedin.com/company/acme/people/");
        play(&mut ws, &runs[0].run_id, "people", "Jane Doe\nHead of purchasing\nJohn Roe\nBuyer", "https://www.linkedin.com/company/acme/people/").await;
        drop(ws);
        let (_, company) = call(&server, "GET", &format!("/api/v1/ontology/objects/{acme}"), &env.alice, org, None).await;
        assert_eq!(company["properties"]["people_to_read"].as_str().map(str::len), Some(36), "{company}");
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{acme}/links"), &env.alice, org, None).await;
        assert!(links.to_string().contains("published_by"), "{links}");
        let (_, listed) = call(&server, "GET", &captures, &env.alice, org, None).await;
        assert_eq!((&listed["results"][0]["open"], &listed["results"][0]["captured"]), (&json!(false), &json!(true)), "{listed}");

        // A viewer may not ask one (R2).
        personal_org(&server, &env.bob).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "viewer").await;
        let (s, e) = call(&server, "POST", &captures, &env.bob, org, Some(ask("https://www.linkedin.com/company/acme/people/"))).await;
        assert_eq!((s, e["error"].as_str()), (403, Some("access-denied")));

        // Jane objected while at ACME: the flow never creates her again.
        let (s, l) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org, Some(json!({"link_type": "employs", "source_id": acme, "target_id": jane}))).await;
        assert_eq!(s, 201, "{l}");
        let (s, _) = call(&server, "POST", &format!("/api/v1/extension/people/{jane}/opposition"), &env.alice, org, None).await;
        assert_eq!(s, 200);
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-ext-1") };
        let flow = deploy_contact_upsert(&env.db, org).await;
        let node = json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": "u"});
        let upsert = |title: &str| json!({"node": node, "title": title, "about": acme,
            "properties": {"ref": format!("linkedin|{acme}|{}", title.to_lowercase()), "company": "ACME"}});
        let (st, r) = internal_flow(&server, "upsert", upsert("JANE  doe")).await;
        assert_eq!((st, r["opposed"].clone(), r["object"].clone()), (200, json!(true), Value::Null), "{r}");
        let (st, r) = internal_flow(&server, "upsert", upsert("John Roe")).await;
        assert_eq!((st, r["created"].clone()), (200, json!(true)), "{r}");

        // Daily cap.
        for i in 1..ba::PEOPLE_CAPTURES_PER_DAY {
            let (s, o) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(new("organization", &format!("Co {i}")))).await;
            assert_eq!(s, 201);
            let (s, e) = call(&server, "POST", &format!("/api/v1/extension/organizations/{}/people-captures", o["id"].as_str().unwrap()), &env.alice, org, Some(ask(&format!("https://www.linkedin.com/company/co-{i}/people/")))).await;
            assert_eq!(s, 201, "{i} {e}");
        }
        let (s, e) = call(&server, "POST", &captures, &env.alice, org, Some(ask("https://www.linkedin.com/company/acme-2/people/"))).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("ext-people-daily-cap")), "{e}");
    })
    .await;
}

/// The agent side of a one-page rule run: the page opened, its text sent as
/// `name`, the run over (the agent is then told it holds nothing).
async fn play(ws: &mut common::DevWs, run_id: &str, name: &str, text: &str, url: &str) {
    send(
        ws,
        &AgentMsg::RunStep {
            run_id: run_id.into(),
            step: serde_json::from_value(json!({
            "index": 0, "action": "open", "label": url, "status": "ok", "url": url, "ms": 5 }))
            .unwrap(),
        },
    )
    .await;
    send(
        ws,
        &AgentMsg::RunExtract {
            run_id: run_id.into(),
            extract: serde_json::from_value(json!({
            "name": name, "kind": "text", "text": text, "chars": text.chars().count(),
            "sha256": "x", "from": url }))
            .unwrap(),
        },
    )
    .await;
    send(
        ws,
        &AgentMsg::RunDone {
            run_id: run_id.into(),
            ok: true,
            failed_step: None,
            reason: None,
        },
    )
    .await;
    assert!(matches!(recv(ws).await, ServerMsg::Runs { .. }));
    sync(ws).await;
}

/// A deployed flow of `org` whose node `u` upserts list contacts.
async fn deploy_contact_upsert(db: &sea_orm::DatabaseConnection, org: i64) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("people".into()),
        status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
        org_id: Set(org),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(json!({ "nodes": [
            {"id": "u", "kind": "object_upsert", "config": {"object_type": "list_contact", "key": "ref", "skip_existing": true}}
        ] })),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let id = flow.id;
    let mut am: flows::ActiveModel = flow.into();
    am.deployed_version_id = Set(Some(version.id));
    am.update(db).await.unwrap();
    id
}

async fn internal_flow(server: &axum_test::TestServer, path: &str, body: Value) -> (u16, Value) {
    let res = server
        .post(&format!("/internal/flow/ontology/{path}"))
        .add_header("x-pnex-flow-token", "tok-ext-1")
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// D550, SEC-56: a personal browser runs only its owner's missions, an admin
/// shares only their own, and neither edits nor deletion reach a colleague's.
#[tokio::test]
#[serial]
async fn a_personal_browser_runs_only_its_owners_missions() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        personal_org(&server, &env.bob).await;
        common::join_org(&server, &env.alice, &env.bob, org, "bob@example.com", "member").await;
        let (alice_install, bob_install) = (Install::new(), Install::new());
        let alice_agent = pair(&server, &env.alice, org, &alice_install).await;
        let bob_agent = pair(&server, &env.bob, org, &bob_install).await;
        let auth = |a: &Paired| a.id;
        let held = |id: i64| {
            let db = env.db.clone();
            async move {
                let agent = pnex_backend::services::extension::agent_by_id(&db, id).await.unwrap().unwrap();
                let mut names: Vec<String> = pnex_backend::services::extension::candidates(&db, &agent).await.unwrap().into_iter().map(|m| m.id).collect();
                names.sort();
                names
            }
        };
        let mission = |token: String, name: &'static str, agent: Option<i64>| {
            let server = &server;
            async move {
                let mut body = mission_body(name);
                body["agent_id"] = json!(agent);
                call(server, "POST", "/api/v1/extension/missions", &token, org, Some(body)).await
            }
        };

        // Each browser runs its owner's mission and nothing else.
        let (s, by_alice) = mission(env.alice.clone(), "Of Alice", None).await;
        assert_eq!(s, 201, "{by_alice}");
        let by_alice = by_alice["id"].as_str().unwrap().to_string();
        let (s, by_bob) = mission(env.bob.clone(), "Of Bob", None).await;
        assert_eq!(s, 201, "{by_bob}");
        let by_bob = by_bob["id"].as_str().unwrap().to_string();
        assert_eq!(held(auth(&alice_agent)).await, vec![by_alice.clone()]);
        assert_eq!(held(auth(&bob_agent)).await, vec![by_bob.clone()]);

        // Bob cannot pin a mission on Alice's browser nor upload to her mission.
        let (s, e) = mission(env.bob.clone(), "Pinned", Some(alice_agent.id)).await;
        assert_eq!(s, 400, "{e}");
        let page = json!({ "mission_id": by_alice, "url": "https://example.org/", "title": "t", "text": "x" });
        let (s, _, _) = upload_page(&server, &bob_install, &bob_agent, &page).await;
        assert_eq!(s, 404, "another member's mission");

        // Bob sees his browser only; Alice, an admin, sees both.
        let (_, list) = call(&server, "GET", "/api/v1/extension/agents", &env.bob, org, None).await;
        assert_eq!(list["count"], 1, "{list}");
        assert_eq!(list["results"][0]["mine"], true);
        let (_, list) = call(&server, "GET", "/api/v1/extension/agents", &env.alice, org, None).await;
        assert_eq!(list["count"], 2, "{list}");

        // Only an admin shares, and only their own browser.
        let share = json!({ "common": true });
        let (s, e) = call(&server, "PATCH", &format!("/api/v1/extension/agents/{}", bob_agent.id), &env.bob, org, Some(share.clone())).await;
        assert_eq!((s, e["error"].as_str()), (403, Some("access-denied")), "{e}");
        let (s, _) = call(&server, "PATCH", &format!("/api/v1/extension/agents/{}", bob_agent.id), &env.alice, org, Some(share.clone())).await;
        assert_eq!(s, 403, "an admin does not share a colleague's browser");
        let (s, _) = call(&server, "PATCH", &format!("/api/v1/extension/agents/{}", alice_agent.id), &env.bob, org, Some(json!({ "name": "x" }))).await;
        assert_eq!(s, 403);
        let (s, _) = call(&server, "DELETE", &format!("/api/v1/devices/{}", alice_agent.id), &env.bob, org, None).await;
        assert_eq!(s, 403, "a member does not disconnect a colleague");

        // A common browser takes the missions of whoever has no browser of
        // their own, but not those of a member who has one, even offline.
        let (s, _) = call(&server, "PATCH", &format!("/api/v1/extension/agents/{}", alice_agent.id), &env.alice, org, Some(share)).await;
        assert_eq!(s, 200);
        assert_eq!(held(alice_agent.id).await, vec![by_alice.clone()], "Bob has his browser");
        let (s, _) = call(&server, "DELETE", &format!("/api/v1/devices/{}", bob_agent.id), &env.bob, org, None).await;
        assert_eq!(s, 204);
        let mut both = vec![by_alice.clone(), by_bob.clone()];
        both.sort();
        assert_eq!(held(alice_agent.id).await, both, "Bob's mission falls back on the common browser");
        let (s, _) = call(&server, "PATCH", &format!("/api/v1/extension/agents/{}", alice_agent.id), &env.alice, org, Some(json!({ "common": false }))).await;
        assert_eq!(s, 200);
        assert_eq!(held(alice_agent.id).await, vec![by_alice.clone()], "taken back at once");

        // Changing what a colleague's mission does makes it the editor's;
        // pausing it does not.
        let bob_browser = pair(&server, &env.bob, org, &bob_install).await;
        let (_, m) = call(&server, "GET", &format!("/api/v1/extension/missions/{by_alice}"), &env.bob, org, None).await;
        let (s, m) = call(&server, "PATCH", &format!("/api/v1/extension/missions/{by_alice}"), &env.bob, org, Some(json!({ "expected_version": m["version"], "enabled": false }))).await;
        assert_eq!(s, 200, "{m}");
        let (s, m) = call(&server, "PATCH", &format!("/api/v1/extension/missions/{by_alice}"), &env.alice, org, Some(json!({ "expected_version": m["version"], "enabled": true }))).await;
        assert_eq!(s, 200, "{m}");
        assert_eq!(held(alice_agent.id).await, vec![by_alice.clone()]);
        let (s, m) = call(&server, "PATCH", &format!("/api/v1/extension/missions/{by_alice}"), &env.bob, org, Some(json!({ "expected_version": m["version"], "interval_secs": m["interval_secs"].as_i64().unwrap() + 60 }))).await;
        assert_eq!(s, 200, "{m}");
        assert!(held(alice_agent.id).await.is_empty(), "no longer Alice's");
        assert!(held(bob_browser.id).await.contains(&by_alice));

        // A member who leaves takes their browsers with them.
        let bob_id = {
            use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
            pnex_backend::models::_entities::users::Entity::find()
                .filter(pnex_backend::models::_entities::users::Column::Email.eq("bob@example.com"))
                .one(&env.db).await.unwrap().unwrap().id
        };
        let (s, _) = call(&server, "DELETE", &format!("/api/v1/orgs/{org}/members/{bob_id}"), &env.alice, org, None).await;
        assert_eq!(s, 204);
        assert!(pnex_backend::services::extension::agent_by_id(&env.db, bob_browser.id).await.unwrap().is_none());
    })
    .await;
}

/// D781–D783: a rule run started for a browser agent waits queued, is
/// leased by the starter's own browser with its sealed plan, and keeps what
/// the agent sends back only from the rule's site; a step off the site ends
/// it as failed.
#[tokio::test]
#[serial]
async fn a_rule_run_plays_on_the_starters_browser_and_stays_on_its_site() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let yaml = "name: search\nsite: example.org\nparams: [query]\nsteps:\n  - open: \"/\"\n  - type: {text: \"{{query}}\"}\n  - press: Enter\n  - extract: {all_text: true, as: page}\n";
        let start = |query: &str| json!({ "yaml": yaml, "params": { "query": query }, "engine": "extension" });
        let (s, run) = call(&server, "POST", "/api/v1/site-recipes/runs", &env.alice, org, Some(start("jane doe"))).await;
        assert_eq!(s, 202, "{run}");
        assert_eq!(run["engine"], "extension");
        let id = run["id"].as_str().unwrap().to_string();
        // No resume on a browser agent: it plays a rule from its start.
        let mut resumed = start("x");
        resumed["resume"] = json!({ "run_id": id, "step": 1 });
        let (s, _) = call(&server, "POST", "/api/v1/site-recipes/runs", &env.alice, org, Some(resumed)).await;
        assert_eq!(s, 400);

        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else {
            panic!("the run is leased with its plan");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, id);
        assert_eq!(runs[0].site, "example.org");
        assert_eq!(runs[0].params["query"], "jane doe");
        assert_eq!(runs[0].recipe.steps.len(), 4);

        let step = |index: u32, action: &str, url: &str| AgentMsg::RunStep {
            run_id: id.clone(),
            step: serde_json::from_value(json!({
                "index": index, "action": action, "label": action, "status": "ok", "url": url, "ms": 5,
                "screenshot": "step.jpg" })).unwrap(),
        };
        send(&mut ws, &step(0, "open", "https://example.org/")).await;
        let extract = |name: &str, from: &str| AgentMsg::RunExtract {
            run_id: id.clone(),
            extract: serde_json::from_value(json!({
                "name": name, "kind": "text", "text": "results for jane doe", "chars": 20,
                "sha256": "x", "from": from })).unwrap(),
        };
        send(&mut ws, &extract("page", "https://www.example.org/search?q=jane")).await;
        send(&mut ws, &extract("elsewhere", "https://evil.example/page")).await;
        send(&mut ws, &AgentMsg::RunDone { run_id: id.clone(), ok: true, failed_step: None, reason: None }).await;
        // Its end frees the agent, told at once.
        assert_eq!(recv(&mut ws).await, ServerMsg::Runs { runs: vec![] });
        sync(&mut ws).await;
        let (_, done) = call(&server, "GET", &format!("/api/v1/site-recipes/runs/{id}"), &env.alice, org, None).await;
        assert_eq!(done["status"], "ok", "{done}");
        assert_eq!(done["steps"].as_array().unwrap().len(), 1);
        assert!(done["steps"][0].get("screenshot").is_none(), "an agent stores no screenshot");
        let names: Vec<&str> = done["extracts"].as_array().unwrap().iter().filter_map(|e| e["name"].as_str()).collect();
        assert_eq!(names, ["page"], "an extract from off the site is dropped");

        // A step off the site ends the run as failed.
        let (_, second) = call(&server, "POST", "/api/v1/site-recipes/runs", &env.alice, org, Some(start("john"))).await;
        let id2 = second["id"].as_str().unwrap().to_string();
        let mut ws = { drop(ws); open(&server, &install, &a).await };
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else { panic!("second run leased") };
        assert_eq!(runs[0].run_id, id2);
        send(&mut ws, &AgentMsg::RunStep {
            run_id: id2.clone(),
            step: serde_json::from_value(json!({
                "index": 0, "action": "open", "label": "/", "status": "ok", "url": "https://evil.example/", "ms": 5 })).unwrap(),
        }).await;
        sync(&mut ws).await;
        let (_, failed) = call(&server, "GET", &format!("/api/v1/site-recipes/runs/{id2}"), &env.alice, org, None).await;
        assert_eq!((failed["status"].as_str(), failed["reason"].as_str()), (Some("failed"), Some("off-site")), "{failed}");
        assert_eq!(failed["steps"][0]["reason"], "off-site");
    })
    .await;
}

/// D782: a rule that needs a session goes to a browser agent by itself, is
/// refused when the org has none, and a run the site blocked on the server
/// browser goes on to the agent under the same id.
#[tokio::test]
#[serial]
async fn a_rule_needing_a_session_goes_to_an_agent_and_a_blocked_run_moves_over() {
    use pnex_backend::services::site_recipes::{self as recipes, NewRun};
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let yaml = "name: needs\nsite: example.org\nsession: required\nsteps:\n  - open: \"/\"\n";
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/site-recipes/runs",
            &env.alice,
            org,
            Some(json!({ "yaml": yaml })),
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (409, Some("site-recipe-no-agent")),
            "{e}"
        );

        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;
        let (s, run) = call(
            &server,
            "POST",
            "/api/v1/site-recipes/runs",
            &env.alice,
            org,
            Some(json!({ "yaml": yaml })),
        )
        .await;
        assert_eq!(s, 202, "{run}");
        assert_eq!(run["engine"], "extension", "routed by `session: required`");

        // A server run the site blocked: the same id, queued for the agent.
        let plain =
            recipes::parse("name: blocked\nsite: example.org\nsteps:\n  - open: \"/\"\n").unwrap();
        let params = Default::default();
        // Started by alice, like the run above: her own browser may take it (D550).
        use sea_orm::ConnectionTrait as _;
        let alice_id: i64 = env
            .db
            .query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT created_by FROM runner_jobs WHERE id = $1",
                vec![uuid::Uuid::parse_str(run["id"].as_str().unwrap())
                    .unwrap()
                    .into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "created_by")
            .unwrap();
        let new = NewRun {
            recipe_id: None,
            recipe_version: None,
            recipe: &plain,
            yaml: "name: blocked\nsite: example.org\nsteps:\n  - open: \"/\"\n",
            params: &params,
            trigger: "studio",
            user_id: Some(alice_id),
            engine: pnex_core::site_recipe::RunEngine::ServerBrowser,
            purpose: None,
        };
        let blocked = recipes::insert_run(&env.db, org, &new).await.unwrap();
        let id = uuid::Uuid::parse_str(&blocked.id).unwrap();
        assert!(recipes::hand_to_extension(&env.db, org, id).await);
        assert!(
            !recipes::hand_to_extension(&env.db, org, id).await,
            "moved once"
        );
        let (_, moved) = call(
            &server,
            "GET",
            &format!("/api/v1/site-recipes/runs/{}", blocked.id),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            (moved["engine"].as_str(), moved["status"].as_str()),
            (Some("extension"), Some("running")),
            "{moved}"
        );

        // The first queued run is leased first; the moved one waits its turn.
        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else {
            panic!("leased")
        };
        assert_eq!(runs[0].run_id, run["id"].as_str().unwrap());
        send(
            &mut ws,
            &AgentMsg::RunDone {
                run_id: runs[0].run_id.clone(),
                ok: true,
                failed_step: None,
                reason: None,
            },
        )
        .await;
        let ServerMsg::Runs { runs } = recv(&mut ws).await else {
            panic!("next leased")
        };
        assert_eq!(runs[0].run_id, blocked.id);
    })
    .await;
}

/// D788: a person entered with their company is searched by the default
/// rule in the author's browser agent; a profile at the company is attached,
/// a past position only proposed, and nobody who objected is searched.
#[tokio::test]
#[serial]
async fn a_person_is_matched_through_the_search_rule_and_a_job_change_is_proposed() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        for pack in ["investigation", "companies", "prospecting_b2b"] {
            let (s, e) = call(&server, "POST", &format!("/api/v1/ontology/packs/{pack}/install"), &env.alice, org, None).await;
            assert_eq!(s, 200, "{pack} {e}");
        }
        let new = |type_key: &str, title: &str| json!({ "type_key": type_key, "title": title, "properties": {} });
        let id_of = |v: &Value| v["id"].as_str().unwrap().to_string();
        let (_, acme) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(new("organization", "ACME INDUSTRIES SAS"))).await;
        let acme = id_of(&acme);
        let person = |name: &'static str| {
            let (server, alice, acme) = (&server, env.alice.clone(), acme.clone());
            async move {
                let (_, p) = call(server, "POST", "/api/v1/ontology/objects", &alice, org, Some(json!({ "type_key": "person", "title": name, "properties": {} }))).await;
                let p = p["id"].as_str().unwrap().to_string();
                let (s, l) = call(server, "POST", "/api/v1/ontology/links", &alice, org, Some(json!({"link_type": "employs", "source_id": acme, "target_id": p}))).await;
                assert_eq!(s, 201, "{l}");
                p
            }
        };
        let install = Install::new();
        let a = pair(&server, &env.alice, org, &install).await;

        // Nobody without a company, nobody who objected.
        let (_, lone) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(new("person", "Lone Wolf"))).await;
        let (s, e) = call(&server, "POST", &format!("/api/v1/extension/people/{}/matches", id_of(&lone)), &env.alice, org, None).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("ext-person-without-company")), "{e}");
        let objecting = person("Olga Object").await;
        call(&server, "POST", &format!("/api/v1/extension/people/{objecting}/opposition"), &env.alice, org, None).await;
        let (s, e) = call(&server, "POST", &format!("/api/v1/extension/people/{objecting}/matches"), &env.alice, org, None).await;
        assert_eq!((s, e["error"].as_str()), (409, Some("prospecting-opposed")), "{e}");

        let elodie = person("Élodie Martin").await;
        let (s, m) = call(&server, "POST", &format!("/api/v1/extension/people/{elodie}/matches"), &env.alice, org, None).await;
        assert_eq!(s, 201, "{m}");
        assert_eq!(m["state"], "searching");
        assert_eq!(m["queries"][0], "elodie martin acme industries");
        assert_eq!(m["queries"].as_array().unwrap().last().unwrap(), "elodie martin");

        // The agent plays the search: the cards, the first profile opened.
        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else { panic!("the search is a rule run") };
        assert_eq!(runs[0].params["query"], "elodie martin acme industries");
        assert!(runs[0].recipe.needs_session());
        assert!(runs[0].recipe.steps.iter().any(|s| s.action() == "type"), "typed in the search bar");
        let cards = json!([
            {"card": "Élodie Martin · Head of purchasing at ACME Industries", "link": "https://www.linkedin.com/in/elodie-martin-1?trk=x"},
            {"card": "Elodie Martinez · Nurse", "link": "https://www.linkedin.com/in/elodie-martinez"},
        ]);
        play_search(&mut ws, runs[0].run_id.clone(), cards, "Elodie Martin\nHead of purchasing at ACME Industries\nToulouse").await;
        let (_, listed) = call(&server, "GET", &format!("/api/v1/extension/people/{elodie}/matches"), &env.alice, org, None).await;
        let found = &listed["results"][0];
        assert_eq!(found["state"], "matched", "{found}");
        assert_eq!(found["candidates"][0]["verdict"], "write");
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{elodie}/links"), &env.alice, org, None).await;
        let links = links.to_string();
        assert!(links.contains("uses_identifier") && links.contains("published_by"), "{links}");
        let (_, p) = call(&server, "GET", &format!("/api/v1/ontology/objects/{elodie}"), &env.alice, org, None).await;
        assert_eq!(p["properties"]["profile_to_read"].as_str().map(str::len), Some(36), "{p}");

        // A job change: the company far down the history, proposed only.
        let john = person("John Roe").await;
        let (s, _) = call(&server, "POST", &format!("/api/v1/extension/people/{john}/matches"), &env.alice, org, None).await;
        assert_eq!(s, 201);
        drop(ws);
        let mut ws = open(&server, &install, &a).await;
        assert!(matches!(recv(&mut ws).await, ServerMsg::Missions { .. }));
        let ServerMsg::Runs { runs } = recv(&mut ws).await else { panic!("second search") };
        let filler = "lorem ipsum ".repeat(80);
        let profile = format!("John Roe\nEngineer at Initech\nParis\n{filler}\nExperience\nBuyer · ACME Industries · 2018 - 2022");
        let cards = json!([{"card": "John Roe · Engineer at Initech", "link": "https://www.linkedin.com/in/john-roe"}]);
        play_search(&mut ws, runs[0].run_id.clone(), cards, &profile).await;
        let (_, listed) = call(&server, "GET", &format!("/api/v1/extension/people/{john}/matches"), &env.alice, org, None).await;
        let proposed = &listed["results"][0];
        assert_eq!(proposed["state"], "proposed", "{proposed}");
        assert_eq!(proposed["candidates"][0]["left_company"], true);
        let (_, inbox) = call(&server, "GET", "/api/v1/proposals?limit=50", &env.alice, org, None).await;
        assert!(inbox.to_string().contains("person_match"), "{inbox}");
    })
    .await;
}

/// The agent side of a search run (D788): the result cards, the first
/// profile opened, the run over.
async fn play_search(ws: &mut common::DevWs, run: String, cards: Value, profile: &str) {
    send(
        ws,
        &AgentMsg::RunExtract {
            run_id: run.clone(),
            extract: serde_json::from_value(json!({
            "name": "results", "kind": "cards", "text": cards.to_string(), "chars": 10,
            "sha256": "x", "from": "https://www.linkedin.com/search/results/all/?keywords=x",
            "cards": cards }))
            .unwrap(),
        },
    )
    .await;
    send(
        ws,
        &AgentMsg::RunExtract {
            run_id: run.clone(),
            extract: serde_json::from_value(json!({
            "name": "profile.1", "kind": "text", "text": profile, "chars": 10,
            "sha256": "x", "from": "https://www.linkedin.com/in/someone" }))
            .unwrap(),
        },
    )
    .await;
    send(
        ws,
        &AgentMsg::RunDone {
            run_id: run,
            ok: true,
            failed_step: None,
            reason: None,
        },
    )
    .await;
    assert!(matches!(recv(ws).await, ServerMsg::Runs { .. }));
    sync(ws).await;
}
