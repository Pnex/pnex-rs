//! Pages (pages.md #21): REST CRUD, org isolation (R1), viewer read-only on
//! the API and on the websocket (R2), named versions, and live sync over
//! `/ws/doc/{id}` (y-protocols, persisted before relay, schema allowlist).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and Valkey (websocket tickets).

mod common;

use std::time::Duration;

use axum_test::WsMessage;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;
use yrs::sync::{Message, SyncMessage};
use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, Transact, Update, XmlFragment};

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
    let config: RequestConfig = RequestConfigBuilder::new().build();
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

async fn add_viewer(server: &axum_test::TestServer, owner: &str, org: i64) {
    let (st, _) = call(
        server,
        "POST",
        &format!("/api/v1/orgs/{org}/members"),
        owner,
        org,
        Some(json!({ "email": "bob@example.com", "role": "viewer" })),
    )
    .await;
    assert!(st == 200 || st == 201, "{st}");
}

#[tokio::test]
#[serial]
async fn crud_versions_isolation_and_viewer() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;

        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org_a,
            Some(json!({ "title": "  " })),
        )
        .await;
        assert_eq!((st, e["title"].as_str()), (400, Some("required")));
        let long = "x".repeat(256);
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org_a,
            Some(json!({ "title": long })),
        )
        .await;
        assert_eq!((st, e["title"].as_str()), (400, Some("max_length:255")));

        let (st, d) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org_a,
            Some(json!({ "title": "Procédure P-12" })),
        )
        .await;
        assert_eq!(st, 201, "{d}");
        let path = format!("/api/v1/docs/{}", d["id"].as_str().unwrap());
        let (st, list) = call(&server, "GET", "/api/v1/docs", &env.alice, org_a, None).await;
        assert_eq!((st, list["count"].as_i64()), (200, Some(1)));
        let (st, r) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org_a,
            Some(json!({ "title": "Procédure P-12 v2" })),
        )
        .await;
        assert_eq!((st, r["title"].as_str()), (200, Some("Procédure P-12 v2")));

        let (st, v) = call(
            &server,
            "POST",
            &format!("{path}/versions"),
            &env.alice,
            org_a,
            Some(json!({ "label": "draft" })),
        )
        .await;
        assert_eq!((st, v["version_number"].as_i64()), (201, Some(1)), "{v}");
        let (st, v2) = call(
            &server,
            "POST",
            &format!("{path}/versions"),
            &env.alice,
            org_a,
            Some(json!({ "label": "final" })),
        )
        .await;
        assert_eq!((st, v2["version_number"].as_i64()), (201, Some(2)));
        let (_, vs) = call(
            &server,
            "GET",
            &format!("{path}/versions"),
            &env.alice,
            org_a,
            None,
        )
        .await;
        assert_eq!(vs["results"][0]["label"], "final");
        let (st, state) = call(
            &server,
            "GET",
            &format!("{path}/versions/1"),
            &env.alice,
            org_a,
            None,
        )
        .await;
        assert_eq!(st, 200);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(state["state_b64"].as_str().unwrap())
            .unwrap();
        assert!(Update::decode_v1(&bytes).is_ok());
        let (st, _) = call(
            &server,
            "GET",
            &format!("{path}/versions/9"),
            &env.alice,
            org_a,
            None,
        )
        .await;
        assert_eq!(st, 404);

        // Another org never sees the page (404, not 403).
        let (_, other) = call(&server, "GET", "/api/v1/docs", &env.bob, org_b, None).await;
        assert_eq!(other["count"], 0);
        for (m, body) in [
            ("GET", None),
            ("PATCH", Some(json!({ "title": "x" }))),
            ("DELETE", None),
        ] {
            let (st, _) = call(&server, m, &path, &env.bob, org_b, body).await;
            assert_eq!(st, 404, "{m}");
        }

        // Viewer: reads, never writes.
        add_viewer(&server, &env.alice, org_a).await;
        let (st, _) = call(&server, "GET", &path, &env.bob, org_a, None).await;
        assert_eq!(st, 200);
        for (m, p, body) in [
            (
                "POST",
                "/api/v1/docs".to_string(),
                Some(json!({ "title": "v" })),
            ),
            ("PATCH", path.clone(), Some(json!({ "title": "v" }))),
            ("DELETE", path.clone(), None),
            (
                "POST",
                format!("{path}/versions"),
                Some(json!({ "label": "v" })),
            ),
        ] {
            let (st, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(st, 403, "{m} {p}");
            assert_eq!(e["error"], "docs-write-forbidden");
        }

        let (st, _) = call(&server, "DELETE", &path, &env.alice, org_a, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "GET", &path, &env.alice, org_a, None).await;
        assert_eq!(st, 404);
    })
    .await;
}

// ── Websocket helpers (a yrs client speaking y-protocols) ────────────────

async fn open(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    doc: &str,
) -> axum_test::TestWebSocket {
    server
        .get_websocket(&format!(
            "/ws/doc/{doc}?ticket={}",
            common::ws_ticket(server, token, org).await
        ))
        .await
        .into_websocket()
        .await
}

/// Next y-protocols message (binary frames only).
async fn next(ws: &mut axum_test::TestWebSocket) -> Message {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.receive_message())
            .await
            .expect("message in time");
        match msg {
            WsMessage::Binary(b) => return Message::decode_v1(&b).expect("y-protocols message"),
            WsMessage::Close(f) => panic!("closed: {f:?}"),
            _ => continue,
        }
    }
}

/// Next document update, skipping presence traffic.
async fn next_update(ws: &mut axum_test::TestWebSocket) -> Vec<u8> {
    loop {
        match next(ws).await {
            Message::Sync(SyncMessage::Update(u) | SyncMessage::SyncStep2(u)) => return u,
            _ => continue,
        }
    }
}

async fn send(ws: &mut axum_test::TestWebSocket, msg: Message) {
    ws.send_message(WsMessage::Binary(msg.encode_v1().into()))
        .await;
}

/// Client handshake: server step 1 answered, own step 1 sent, step 2 read.
async fn handshake(ws: &mut axum_test::TestWebSocket, local: &Doc) {
    match next(ws).await {
        Message::Sync(SyncMessage::SyncStep1(sv)) => {
            let diff = local.transact().encode_state_as_update_v1(&sv);
            send(ws, Message::Sync(SyncMessage::SyncStep2(diff))).await;
        }
        other => panic!("expected server step 1, got {other:?}"),
    }
    let sv = local.transact().state_vector();
    send(ws, Message::Sync(SyncMessage::SyncStep1(sv))).await;
    let u = next_update(ws).await;
    local
        .transact_mut()
        .apply_update(Update::decode_v1(&u).unwrap())
        .unwrap();
}

/// Appends a paragraph and returns the update it produced.
fn write_paragraph(doc: &Doc, text: &str) -> Vec<u8> {
    let root = doc.get_or_insert_xml_fragment("prosemirror");
    let before = doc.transact().state_vector();
    {
        let mut txn = doc.transact_mut();
        let len = root.len(&txn);
        let p = root.insert(&mut txn, len, XmlElementPrelim::empty("paragraph"));
        p.push_back(&mut txn, XmlTextPrelim::new(text));
    }
    doc.transact().encode_state_as_update_v1(&before)
}

fn text_of(doc: &Doc) -> String {
    let root = doc.get_or_insert_xml_fragment("prosemirror");
    root.get_string(&doc.transact())
}

#[tokio::test]
#[serial]
async fn live_sync_persists_and_enforces_schema_and_roles() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, d) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org,
            Some(json!({ "title": "Live" })),
        )
        .await;
        let id = d["id"].as_str().unwrap().to_string();

        // Two editors: what one writes reaches the other.
        let (a_doc, b_doc) = (Doc::new(), Doc::new());
        let mut a = open(&server, &env.alice, org, &id).await;
        handshake(&mut a, &a_doc).await;
        let mut b = open(&server, &env.alice, org, &id).await;
        handshake(&mut b, &b_doc).await;
        let u = write_paragraph(&a_doc, "torque 45 Nm");
        send(&mut a, Message::Sync(SyncMessage::Update(u))).await;
        let got = next_update(&mut b).await;
        b_doc
            .transact_mut()
            .apply_update(Update::decode_v1(&got).unwrap())
            .unwrap();
        assert!(
            text_of(&b_doc).contains("torque 45 Nm"),
            "{}",
            text_of(&b_doc)
        );

        // Content outside the schema closes the session, nothing is stored.
        let evil = Doc::new();
        let before = evil.transact().state_vector();
        {
            let root = evil.get_or_insert_xml_fragment("prosemirror");
            let mut txn = evil.transact_mut();
            root.push_back(&mut txn, XmlElementPrelim::empty("script"));
        }
        let bad = evil.transact().encode_state_as_update_v1(&before);
        send(&mut b, Message::Sync(SyncMessage::Update(bad))).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), b.receive_message())
                .await
                .expect("close")
            {
                WsMessage::Close(Some(f)) => {
                    assert_eq!(u16::from(f.code), 4422);
                    break;
                }
                WsMessage::Close(None) => panic!("close without code"),
                _ => continue,
            }
        }

        // A viewer reads the page live but its updates are dropped.
        personal_org(&server, &env.bob).await; // provisions bob
        add_viewer(&server, &env.alice, org).await;
        let v_doc = Doc::new();
        let mut v = open(&server, &env.bob, org, &id).await;
        handshake(&mut v, &v_doc).await;
        assert!(text_of(&v_doc).contains("torque 45 Nm"));
        let sneaky = write_paragraph(&v_doc, "viewer edit");
        send(&mut v, Message::Sync(SyncMessage::Update(sneaky))).await;

        // Everyone leaves: the room closes and compacts; a fresh client
        // rebuilds the page from the database alone.
        drop(a);
        drop(v);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let fresh = Doc::new();
        let mut c = open(&server, &env.alice, org, &id).await;
        handshake(&mut c, &fresh).await;
        let text = text_of(&fresh);
        assert!(text.contains("torque 45 Nm"), "{text}");
        assert!(!text.contains("viewer edit"), "{text}");
        assert!(!text.contains("script"), "{text}");

        // A named version freezes that content.
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/docs/{id}/versions"),
            &env.alice,
            org,
            Some(json!({ "label": "v1" })),
        )
        .await;
        assert_eq!(st, 201);
        let (_, state) = call(
            &server,
            "GET",
            &format!("/api/v1/docs/{id}/versions/1"),
            &env.alice,
            org,
            None,
        )
        .await;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(state["state_b64"].as_str().unwrap())
            .unwrap();
        let frozen = Doc::new();
        frozen
            .transact_mut()
            .apply_update(Update::decode_v1(&bytes).unwrap())
            .unwrap();
        assert!(text_of(&frozen).contains("torque 45 Nm"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ws_refuses_bad_ticket_and_foreign_page() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, d) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org_a,
            Some(json!({ "title": "Private" })),
        )
        .await;
        let id = d["id"].as_str().unwrap();
        for (path, code) in [
            (format!("/ws/doc/{id}"), 4002),
            (format!("/ws/doc/{id}?ticket=nope"), 4001),
            (
                format!(
                    "/ws/doc/{id}?ticket={}",
                    common::ws_ticket(&server, &env.bob, org_b).await
                ),
                4004,
            ),
        ] {
            let mut ws = server.get_websocket(&path).await.into_websocket().await;
            match tokio::time::timeout(Duration::from_secs(5), ws.receive_message())
                .await
                .expect("close")
            {
                WsMessage::Close(Some(f)) => assert_eq!(u16::from(f.code), code, "{path}"),
                other => panic!("{path}: expected close, got {other:?}"),
            }
        }
    })
    .await;
}

async fn add_member(server: &axum_test::TestServer, owner: &str, org: i64, role: &str) {
    let (st, _) = call(
        server,
        "POST",
        &format!("/api/v1/orgs/{org}/members"),
        owner,
        org,
        Some(json!({ "email": "bob@example.com", "role": role })),
    )
    .await;
    assert!(st == 200 || st == 201, "{st}");
}

#[tokio::test]
#[serial]
async fn comments_threads_roles_and_live_notice() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, d) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org_a,
            Some(json!({ "title": "Review" })),
        )
        .await;
        let id = d["id"].as_str().unwrap().to_string();
        let base = format!("/api/v1/docs/{id}/comments");

        // An open editor hears about comment changes.
        let mut ws = open(&server, &env.alice, org_a, &id).await;
        handshake(&mut ws, &Doc::new()).await;

        let (st, e) = call(
            &server,
            "POST",
            &base,
            &env.alice,
            org_a,
            Some(json!({ "body": " " })),
        )
        .await;
        assert_eq!((st, e["body"].as_str()), (400, Some("required")));
        let long_quote = "q".repeat(400);
        let (st, root) = call(
            &server,
            "POST",
            &base,
            &env.alice,
            org_a,
            Some(json!({ "body": "Check this value", "anchor": "AAE.AAI", "quote": long_quote })),
        )
        .await;
        assert_eq!(st, 201, "{root}");
        let root_id = root["id"].as_i64().unwrap();
        loop {
            if let Message::Custom(100, _) = next(&mut ws).await {
                break;
            }
        }
        let (st, reply) = call(
            &server,
            "POST",
            &base,
            &env.alice,
            org_a,
            Some(json!({ "body": "Done", "parent_id": root_id })),
        )
        .await;
        assert_eq!(st, 201);
        let (st, e) = call(
            &server,
            "POST",
            &base,
            &env.alice,
            org_a,
            Some(json!({ "body": "nested", "parent_id": reply["id"] })),
        )
        .await;
        assert_eq!((st, e["parent_id"].as_str()), (400, Some("invalid")));

        let (_, list) = call(&server, "GET", &base, &env.alice, org_a, None).await;
        assert_eq!(list["count"], 2);
        assert_eq!(
            list["results"][0]["quote"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            300
        );
        assert_eq!(list["results"][0]["mine"], true);
        assert!(list["results"][1]["anchor"].is_null());

        let root_path = format!("{base}/{root_id}");
        let (st, _) = call(
            &server,
            "PATCH",
            &root_path,
            &env.alice,
            org_a,
            Some(json!({ "resolved": true })),
        )
        .await;
        assert_eq!(st, 204);
        let (_, list) = call(&server, "GET", &base, &env.alice, org_a, None).await;
        assert_eq!(list["results"][0]["resolved"], true);

        // Another org: 404 everywhere.
        for (m, p, body) in [
            ("GET", base.clone(), None),
            ("POST", base.clone(), Some(json!({ "body": "x" }))),
            ("DELETE", root_path.clone(), None),
        ] {
            let (st, _) = call(&server, m, &p, &env.bob, org_b, body).await;
            assert_eq!(st, 404, "{m} {p}");
        }

        // Viewer: reads, never writes.
        add_member(&server, &env.alice, org_a, "viewer").await;
        let (st, _) = call(&server, "GET", &base, &env.bob, org_a, None).await;
        assert_eq!(st, 200);
        for (m, p, body) in [
            ("POST", base.clone(), Some(json!({ "body": "v" }))),
            (
                "PATCH",
                root_path.clone(),
                Some(json!({ "resolved": false })),
            ),
            ("DELETE", root_path.clone(), None),
        ] {
            let (st, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(
                (st, e["error"].as_str()),
                (403, Some("docs-write-forbidden")),
                "{m} {p}"
            );
        }

        // Member: comments, but cannot edit or delete alice's comment.
        let (st, _) = call(
            &server,
            "PATCH",
            &format!(
                "/api/v1/orgs/{org_a}/members/{}",
                member_id(&server, &env.alice, org_a, "bob@example.com").await
            ),
            &env.alice,
            org_a,
            Some(json!({ "role": "member" })),
        )
        .await;
        assert!(st == 200 || st == 204, "{st}");
        let (st, _) = call(
            &server,
            "POST",
            &base,
            &env.bob,
            org_a,
            Some(json!({ "body": "me too", "parent_id": root_id })),
        )
        .await;
        assert_eq!(st, 201);
        let (st, e) = call(
            &server,
            "PATCH",
            &root_path,
            &env.bob,
            org_a,
            Some(json!({ "body": "hijack" })),
        )
        .await;
        assert_eq!(
            (st, e["error"].as_str()),
            (403, Some("doc-comment-not-author"))
        );
        let (st, e) = call(&server, "DELETE", &root_path, &env.bob, org_a, None).await;
        assert_eq!(
            (st, e["error"].as_str()),
            (403, Some("doc-comment-not-author"))
        );

        // The owner deletes the thread, replies included.
        let (st, _) = call(&server, "DELETE", &root_path, &env.alice, org_a, None).await;
        assert_eq!(st, 204);
        let (_, list) = call(&server, "GET", &base, &env.alice, org_a, None).await;
        assert_eq!(list["count"], 0);
    })
    .await;
}

/// User id of a member, from the org's member list.
async fn member_id(server: &axum_test::TestServer, token: &str, org: i64, email: &str) -> i64 {
    let (_, members) = call(
        server,
        "GET",
        &format!("/api/v1/orgs/{org}/members"),
        token,
        org,
        None,
    )
    .await;
    let rows = members
        .as_array()
        .cloned()
        .or_else(|| members["results"].as_array().cloned())
        .unwrap_or_default();
    rows.iter()
        .find(|m| m["email"] == email)
        .and_then(|m| m["user_id"].as_i64().or_else(|| m["id"].as_i64()))
        .unwrap_or_else(|| panic!("member {email} in {members}"))
}

fn presence(client: u64, clock: u32, json: &str) -> Message {
    use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
    let entry = AwarenessUpdateEntry {
        clock,
        json: json.into(),
    };
    Message::Awareness(AwarenessUpdate {
        clients: std::collections::HashMap::from([(yrs::block::ClientID::new(client), entry)]),
    })
}

/// Next presence frame, skipping document traffic.
async fn next_presence(ws: &mut axum_test::TestWebSocket) -> Vec<(u64, String)> {
    loop {
        if let Message::Awareness(u) = next(ws).await {
            return u
                .clients
                .into_iter()
                .map(|(id, e)| (id.get(), e.json.to_string()))
                .collect();
        }
    }
}

#[tokio::test]
#[serial]
async fn ws_refuses_dangling_updates_and_owns_presence() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, d) = call(
            &server,
            "POST",
            "/api/v1/docs",
            &env.alice,
            org,
            Some(json!({ "title": "Edge" })),
        )
        .await;
        let id = d["id"].as_str().unwrap().to_string();

        let mut a = open(&server, &env.alice, org, &id).await;
        handshake(&mut a, &Doc::new()).await;
        let mut b = open(&server, &env.alice, org, &id).await;
        handshake(&mut b, &Doc::new()).await;

        // a announces client 42; b sees it; b cannot rewrite it.
        send(&mut a, presence(42, 1, r#"{"user":{"name":"Alice"}}"#)).await;
        assert_eq!(
            next_presence(&mut b).await,
            vec![(42, r#"{"user":{"name":"Alice"}}"#.into())]
        );
        send(&mut b, presence(42, 2, "null")).await;
        send(&mut b, presence(7, 1, r#"{"user":{"name":"Bob"}}"#)).await;
        assert_eq!(
            next_presence(&mut a).await,
            vec![(7, r#"{"user":{"name":"Bob"}}"#.into())]
        );

        // a leaves: its presence is cleared for b.
        drop(a);
        assert_eq!(next_presence(&mut b).await, vec![(42, "null".into())]);

        // An update that depends on content the page does not have is refused.
        let local = Doc::new();
        let _first = write_paragraph(&local, "first");
        let second = write_paragraph(&local, "second");
        send(&mut b, Message::Sync(SyncMessage::Update(second))).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(5), b.receive_message())
                .await
                .expect("close")
            {
                WsMessage::Close(Some(f)) => {
                    assert_eq!(u16::from(f.code), 4422);
                    break;
                }
                WsMessage::Close(None) => panic!("close without code"),
                _ => continue,
            }
        }
    })
    .await;
}

/// Pages written by flows (D209/D210): a new page per message, then an
/// append to it, each freezing a named version stamped with the flow and
/// its AI provenance; approval by an editor (idempotent), refused to a
/// viewer; another org's page is not found; the internal route needs the
/// service token.
#[tokio::test]
#[serial]
async fn flow_writes_pages_and_versions_get_approved() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = &env.alice;
        let body = json!({
            "org_id": org, "flow_id": 7, "mode": "new_page", "title": "Weekly {date}",
            "markdown": "# Weekly\n\nA **calm** week.", "version_label": "week {week}",
            "ai_generated": true
        });
        let denied = server.post("/internal/flow/page-write").json(&body).await;
        assert_eq!(denied.status_code(), 401);
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-pages") };
        let post = |b: serde_json::Value| {
            let server = &server;
            async move {
                let res = server
                    .post("/internal/flow/page-write")
                    .add_header("x-pnex-flow-token", "tok-pages")
                    .json(&b)
                    .await;
                (res.status_code().as_u16(), res.json::<serde_json::Value>())
            }
        };
        let (st, created) = post(body.clone()).await;
        assert_eq!(st, 201, "{created}");
        assert!(created["title"].as_str().unwrap().starts_with("Weekly 20"));
        assert_eq!(created["version"], 1);
        let page = created["page_id"].as_str().unwrap().to_string();
        let mut append = body.clone();
        append["mode"] = json!("append");
        append["page_id"] = json!(page);
        append["markdown"] = json!("## Monday\n\n- one alert");
        append["ai_generated"] = json!(false);
        let (st, appended) = post(append.clone()).await;
        assert_eq!(st, 201, "{appended}");
        assert_eq!(appended["version"], 2);
        let mut foreign = append.clone();
        foreign["org_id"] = json!(org + 100_000);
        let (st, _) = post(foreign).await;
        assert_eq!(st, 404);

        let (_, versions) = call(
            &server,
            "GET",
            &format!("/api/v1/docs/{page}/versions"),
            a,
            org,
            None,
        )
        .await;
        let v1 = versions["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["version_number"] == 1)
            .unwrap()
            .clone();
        assert_eq!(v1["ai_generated"], true);
        assert_eq!(v1["flow_id"], 7);
        assert!(v1["label"].as_str().unwrap().starts_with("week 20"));
        assert!(v1.get("approved_at").is_none());

        let bob_org = personal_org(&server, &env.bob).await;
        add_viewer(&server, a, org).await;
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/docs/{page}/versions/1/approve"),
            &env.bob,
            org,
            None,
        )
        .await;
        assert_eq!(st, 403);
        let (st, approved) = call(
            &server,
            "POST",
            &format!("/api/v1/docs/{page}/versions/1/approve"),
            a,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{approved}");
        assert!(approved["approved_at"].is_string());
        let first = approved["approved_at"].clone();
        let (_, again) = call(
            &server,
            "POST",
            &format!("/api/v1/docs/{page}/versions/1/approve"),
            a,
            org,
            None,
        )
        .await;
        assert_eq!(again["approved_at"], first, "the first approval is kept");
        // PDF of a version (D209): A4, attachment, other orgs' pages not found.
        let res = server
            .get(&format!("/api/v1/docs/{page}/versions/1/pdf?lang=fr"))
            .add_header("Authorization", format!("Bearer {a}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.header("content-type"), "application/pdf");
        assert!(res.as_bytes().starts_with(b"%PDF-"));
        let res = server
            .get(&format!("/api/v1/docs/{page}/versions/1/pdf"))
            .add_header("Authorization", format!("Bearer {}", env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
