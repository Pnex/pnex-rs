//! Mailboxes (mail.md D453–D463): a CRUD of sources whose password stays in
//! the vault, who manages and who reads (a member's own mailbox, a shared
//! one read by its teams, never a member's mailbox by an admin), messages
//! stored once with their forward and authentication, the deploy gate of
//! `mail_received`, and the poller recording a server it cannot reach.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::mail;
use sea_orm::DatabaseConnection;
use serde_json::{json, Value};
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
    db: DatabaseConnection,
    config: loco_rs::config::Config,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(
                server,
                Env {
                    alice,
                    bob,
                    db: ctx.db.clone(),
                    config: ctx.config.clone(),
                },
            )
            .await;
        },
    )
    .await;
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
        "PUT" => server.put(path),
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
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn me(server: &axum_test::TestServer, token: &str) -> Value {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()
}

fn input(name: &str, address: &str, extra: Value) -> Value {
    let mut v = json!({
        "name": name, "address": address,
        "imap_host": "imap.example.org", "imap_user": address,
        "password": {"value": "s3cret-imap"}
    });
    for (k, x) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = x;
    }
    v
}

/// Alice's org, Bob a member, a team « Sales » holding Bob.
async fn setup(server: &axum_test::TestServer, env: &Env) -> (i64, i64, String) {
    let bob_id = me(server, &env.bob).await["id"].as_i64().unwrap();
    let org = me(server, &env.alice).await["orgs"][0]["id"]
        .as_i64()
        .unwrap();
    let (st, e) = call(
        server,
        "POST",
        &format!("/api/v1/orgs/{org}/members"),
        &env.alice,
        org,
        Some(json!({"email": "bob@example.com", "role": "member"})),
    )
    .await;
    assert!(st == 200 || st == 201, "{e}");
    let (st, team) = call(
        server,
        "POST",
        "/api/v1/iam/teams",
        &env.alice,
        org,
        Some(json!({"name": "Sales"})),
    )
    .await;
    assert_eq!(st, 201, "{team}");
    let team_id = team["id"].as_str().unwrap().to_string();
    let (st, t) = call(
        server,
        "POST",
        &format!("/api/v1/iam/teams/{team_id}/members"),
        &env.alice,
        org,
        Some(json!({"user_id": bob_id})),
    )
    .await;
    assert_eq!(st, 200, "{t}");
    (org, bob_id, team_id)
}

/// A parsed message with the verdict the poller's DKIM check would give.
fn parsed(from: &str, message_id: &str, verified: bool, body: &str) -> mail::parse::Parsed {
    let raw = format!(
        "From: {from}\r\nTo: sales@example.com\r\nSubject: Fwd: Devis\r\nMessage-ID: <{message_id}>\r\n\r\n{body}\r\n"
    );
    let mut p = mail::parse::parse(raw.as_bytes(), false, &[], "x").unwrap();
    p.auth = mail::verify::verdict("example.com", [("example.com", verified, !verified)]);
    p
}

#[tokio::test]
#[serial]
async fn mailboxes_are_managed_and_read_by_their_owners_and_teams() {
    with_app(|server, env| async move {
        let (org, _bob_id, team) = setup(&server, &env).await;

        // A member connects their own mailbox, typing its password.
        let (st, own) = call(&server, "POST", "/api/v1/mailboxes", &env.bob, org,
            Some(input("Bob", "bob@example.com", json!({})))).await;
        assert_eq!(st, 201, "{own}");
        assert_eq!(own["has_password"], true);
        assert_eq!(own["imap_port"], 993);
        assert_eq!(own["folders"], json!(["INBOX"]));
        assert!(own.to_string().find("s3cret").is_none(), "never the password");
        let bob_box = own["id"].as_str().unwrap().to_string();

        // But never picks a vault secret, nor creates a shared mailbox.
        let pick = json!({"password": {"secret_id": "00000000-0000-0000-0000-000000000001"}});
        let (st, e) = call(&server, "POST", "/api/v1/mailboxes", &env.bob, org, Some(input("Bob 2", "b2@example.com", pick))).await;
        assert_eq!((st, e["errors"]["fields"]["password"].as_str()), (400, Some("invalid")), "{e}");
        let (st, _) = call(&server, "POST", "/api/v1/mailboxes", &env.bob, org,
            Some(input("Sales", "sales@example.com", json!({"shared": true, "teams": [team]})))).await;
        assert_eq!(st, 400);

        // A malformed server name is refused (an internal one too, by the
        // egress policy of the deployment, R8: the tests allow loopback).
        let (st, e) = call(&server, "POST", "/api/v1/mailboxes", &env.bob, org,
            Some(input("Bad", "l@example.com", json!({"imap_host": "imap host/x"})))).await;
        assert_eq!((st, e["errors"]["fields"]["imap_host"].as_str()), (400, Some("invalid")), "{e}");

        // The admin shares one with the team.
        let (st, shared) = call(&server, "POST", "/api/v1/mailboxes", &env.alice, org,
            Some(input("Sales", "sales@example.com", json!({"shared": true, "teams": [team]})))).await;
        assert_eq!(st, 201, "{shared}");
        assert!(shared.get("owner").is_none());
        let sales = shared["id"].as_str().unwrap().to_string();
        let (st, _) = call(&server, "POST", "/api/v1/mailboxes", &env.alice, org,
            Some(input("Sales", "other@example.com", json!({})))).await;
        assert_eq!(st, 409, "name taken");

        // Who reads what.
        let (_, l) = call(&server, "GET", "/api/v1/mailboxes", &env.bob, org, None).await;
        let readable = |l: &Value, id: &str| l["results"].as_array().unwrap().iter().find(|b| b["id"] == id).unwrap()["readable"].clone();
        assert_eq!((readable(&l, &bob_box), readable(&l, &sales)), (json!(true), json!(true)));
        let (_, l) = call(&server, "GET", "/api/v1/mailboxes", &env.alice, org, None).await;
        assert_eq!(readable(&l, &bob_box), json!(false), "an admin never reads a member's mailbox");
        let (st, _) = call(&server, "GET", &format!("/api/v1/mailboxes/{bob_box}/messages"), &env.alice, org, None).await;
        assert_eq!(st, 404);
        // ... nor changes it, but may pause it.
        let (st, _) = call(&server, "PUT", &format!("/api/v1/mailboxes/{bob_box}"), &env.alice, org,
            Some(input("Bob", "bob@example.com", json!({})))).await;
        assert_eq!(st, 404);
        let (st, p) = call(&server, "POST", &format!("/api/v1/mailboxes/{bob_box}/state"), &env.alice, org, Some(json!({"paused": true}))).await;
        assert_eq!((st, p["state"].clone()), (200, json!("paused")), "{p}");

        // Moving to another server needs the password again (R9).
        let mut moved = input("Bob", "bob@example.com", json!({"imap_host": "mail.other.org"}));
        moved.as_object_mut().unwrap().remove("password");
        let (st, e) = call(&server, "PUT", &format!("/api/v1/mailboxes/{bob_box}"), &env.bob, org, Some(moved.clone())).await;
        assert_eq!((st, e["errors"]["fields"]["password"].as_str()), (400, Some("required")), "{e}");
        moved["password"] = json!({"value": "new-pass"});
        let (st, e) = call(&server, "PUT", &format!("/api/v1/mailboxes/{bob_box}"), &env.bob, org, Some(moved)).await;
        assert_eq!(st, 200, "{e}");

        // Messages stored once, the forward of a member trusted.
        let box_row = mail::find(&env.db, org, uuid::Uuid::parse_str(&sales).unwrap()).await.unwrap();
        let members = vec!["bob@example.com".to_string()];
        let fwd = "---------- Forwarded message ---------\nFrom: Élodie Martin <elodie@acme.fr>\nSubject: Devis ligne 3\n\nBonjour";
        let p = parsed("Bob <bob@example.com>", "f1@x", true, fwd);
        let m = mail::poller::store(&env.db, &box_row, "INBOX", 1, p.clone(), &members).await.unwrap().expect("new");
        assert!(m.forwarded.as_ref().unwrap().trusted);
        assert_eq!(m.correspondent().address, "elodie@acme.fr");
        assert!(mail::poller::store(&env.db, &box_row, "INBOX", 1, p, &members).await.unwrap().is_none(), "once per Message-ID");
        // Forwarded on by a colleague first: the client is still found.
        let nested = format!("---------- Forwarded message ---------\nFrom: Bob <bob@example.com>\nSubject: Fwd: Devis\n\n{fwd}");
        let m = mail::poller::store(&env.db, &box_row, "INBOX", 3, parsed("Bob <bob@example.com>", "f3@x", true, &nested), &members)
            .await.unwrap().unwrap();
        assert_eq!(m.correspondent().address, "elodie@acme.fr");
        assert!(m.forwarded.unwrap().trusted);
        let spoof = parsed("Bob <bob@example.com>", "f2@x", false, fwd);
        let m = mail::poller::store(&env.db, &box_row, "INBOX", 2, spoof, &members).await.unwrap().unwrap();
        assert!(!m.forwarded.unwrap().trusted, "a failed authentication is never believed");

        let (st, msgs) = call(&server, "GET", &format!("/api/v1/mailboxes/{sales}/messages"), &env.bob, org, None).await;
        assert_eq!((st, msgs["count"].clone()), (200, json!(3)), "{msgs}");
        assert_eq!(msgs["results"][0]["from"]["address"], "bob@example.com");

        // Deleting drops its messages and its password.
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/mailboxes/{sales}"), &env.bob, org, None).await;
        assert_eq!(st, 404, "a member does not delete a shared mailbox");
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/mailboxes/{sales}"), &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "GET", &format!("/api/v1/mailboxes/{sales}/messages"), &env.bob, org, None).await;
        assert_eq!(st, 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_flow_listens_only_to_mailboxes_its_deployer_reads() {
    with_app(|server, env| async move {
        let (org, _, _) = setup(&server, &env).await;
        let (_, own) = call(
            &server,
            "POST",
            "/api/v1/mailboxes",
            &env.alice,
            org,
            Some(input("Alice", "alice@example.com", json!({}))),
        )
        .await;
        let alice_box = own["id"].as_str().unwrap().to_string();
        let graph = json!({"nodes": [
            {"id": "mr", "kind": "mail_received", "config": {"mailboxes": [alice_box]},
             "outputs": [{"port": 0, "targets": ["dbg"]}]},
            {"id": "dbg", "kind": "debug"}
        ]});
        let (st, f) = call(
            &server,
            "POST",
            "/api/v1/flows",
            &env.bob,
            org,
            Some(json!({"name": "Spy", "graph": graph})),
        )
        .await;
        assert_eq!(st, 201, "{f}");
        let flow_id = f["id"].as_i64().unwrap();
        let (st, d) = call(
            &server,
            "POST",
            &format!("/api/v1/flows/{flow_id}/deploy"),
            &env.bob,
            org,
            Some(json!({})),
        )
        .await;
        // Bob deploys flows (member), but not on Alice's mailbox.
        assert_eq!(
            (st, d["errors"]["violations"][0]["code"].clone()),
            (400, json!("mail-mailbox-forbidden")),
            "{d}"
        );
        let empty = json!({"nodes": [{"id": "mr", "kind": "mail_received", "config": {}}]});
        let (st, f) = call(
            &server,
            "POST",
            "/api/v1/flows",
            &env.alice,
            org,
            Some(json!({"name": "Draft", "graph": empty})),
        )
        .await;
        assert_eq!(st, 201, "an empty trigger saves as a draft: {f}");
        let (st, d) = call(
            &server,
            "POST",
            &format!("/api/v1/flows/{}/deploy", f["id"]),
            &env.alice,
            org,
            Some(json!({})),
        )
        .await;
        assert_eq!(
            (st, d["errors"]["violations"][0]["code"].clone()),
            (400, json!("mail-no-mailbox")),
            "{d}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_restarted_node_replays_the_mails_it_missed() {
    with_app(|server, env| async move {
        let (org, _, _) = setup(&server, &env).await;
        let (_, own) = call(&server, "POST", "/api/v1/mailboxes", &env.alice, org,
            Some(input("Alice", "alice@example.com", json!({})))).await;
        let alice_box = own["id"].as_str().unwrap().to_string();
        let graph = json!({"nodes": [
            {"id": "mr", "kind": "mail_received",
             "config": {"mailboxes": [alice_box], "from_contains": "acme"},
             "outputs": [{"port": 0, "targets": ["dbg"]}]},
            {"id": "dbg", "kind": "debug"}
        ]});
        let (_, f) = call(&server, "POST", "/api/v1/flows", &env.alice, org,
            Some(json!({"name": "Inbox", "graph": graph}))).await;
        let flow_id = f["id"].as_i64().unwrap();
        // Marked deployed as the runtime would ack it (none in these tests).
        use sea_orm::ConnectionTrait;
        env.db.execute_unprepared(&format!(
            "UPDATE flows SET status = '{}', deployed_version_id = \
               (SELECT id FROM flow_versions WHERE flow_id = {flow_id} ORDER BY version_number DESC LIMIT 1) \
             WHERE id = {flow_id}",
            pnex_core::FLOW_STATUS_DEPLOYED
        )).await.unwrap();
        let node = |id: &str| pnex_core::ontology::flow::FlowNodeRef {
            org_id: org,
            flow_id,
            version: 1,
            node_id: id.into(),
        };
        let rq = || pnex_core::mail::MailReplayRequest { node: node("mr") };

        // First start: the cursor begins, no history.
        assert!(mail::replay::replay(&env.db, &rq()).await.unwrap().is_empty());

        // Stopped meanwhile: two mails stored, one passes the node's filter.
        let row = mail::find(&env.db, org, uuid::Uuid::parse_str(&alice_box).unwrap()).await.unwrap();
        for (i, from) in ["Élodie <elodie@acme.fr>", "x@other.org"].iter().enumerate() {
            mail::poller::store(&env.db, &row, "INBOX", i as u32 + 1, parsed(from, &format!("r{i}@x"), true, "Bonjour"), &[])
                .await.unwrap().unwrap();
        }
        let missed = mail::replay::replay(&env.db, &rq()).await.unwrap();
        assert_eq!(missed.iter().map(|m| m.from.address.as_str()).collect::<Vec<_>>(), ["elodie@acme.fr"]);

        // Once emitted and acknowledged, never again; a new one comes.
        let ak = |id: String| pnex_core::mail::MailAckRequest { node: node("mr"), id };
        mail::replay::ack(&env.db, &ak(missed[0].id.clone())).await.unwrap();
        assert!(mail::replay::replay(&env.db, &rq()).await.unwrap().is_empty());
        mail::poller::store(&env.db, &row, "INBOX", 3, parsed("bob@acme.fr", "r3@x", true, "Suite"), &[])
            .await.unwrap().unwrap();
        let next = mail::replay::replay(&env.db, &rq()).await.unwrap();
        assert_eq!(next.len(), 1);
        // The cursor never moves back.
        mail::replay::ack(&env.db, &ak(next[0].id.clone())).await.unwrap();
        mail::replay::ack(&env.db, &ak(missed[0].id.clone())).await.unwrap();
        assert!(mail::replay::replay(&env.db, &rq()).await.unwrap().is_empty());

        // Another node, or a forged one, gets nothing.
        let forged = mail::replay::replay(&env.db, &pnex_core::mail::MailReplayRequest { node: node("dbg") }).await;
        assert!(forged.is_err());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_poller_records_a_server_it_cannot_reach() {
    with_app(|server, env| async move {
        let (org, _, _) = setup(&server, &env).await;
        let (st, b) = call(
            &server,
            "POST",
            "/api/v1/mailboxes",
            &env.alice,
            org,
            Some(input(
                "Nowhere",
                "n@example.com",
                json!({"imap_host": "imap.nowhere.invalid"}),
            )),
        )
        .await;
        assert_eq!(st, 201, "{b}");
        let id = uuid::Uuid::parse_str(b["id"].as_str().unwrap()).unwrap();
        let row = mail::find(&env.db, org, id).await.unwrap();
        let ring = pnex_backend::services::secrets::Keyring::from_config(&env.config).unwrap();
        assert_eq!(mail::poller::poll(&env.db, &ring, None, &row).await, 0);
        let (_, l) = call(&server, "GET", "/api/v1/mailboxes", &env.alice, org, None).await;
        let b = &l["results"][0];
        assert_eq!(b["state"], "active", "a network error is tried again");
        assert_eq!(b["last_error"]["code"], "mail-unreachable", "{b}");
        let (st, t) = call(
            &server,
            "POST",
            &format!("/api/v1/mailboxes/{id}/test"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            (st, t["ok"].clone(), t["error"]["code"].clone()),
            (200, json!(false), json!("mail-unreachable")),
            "{t}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_attachment_is_saved_as_a_media_only_by_a_reader() {
    with_app(|server, env| async move {
        let (org, _, _) = setup(&server, &env).await;
        let (_, b) = call(
            &server,
            "POST",
            "/api/v1/mailboxes",
            &env.alice,
            org,
            Some(input(
                "Alice",
                "alice@example.com",
                json!({"imap_host": "imap.nowhere.invalid"}),
            )),
        )
        .await;
        let id = uuid::Uuid::parse_str(b["id"].as_str().unwrap()).unwrap();
        let row = mail::find(&env.db, org, id).await.unwrap();
        let raw = "From: elodie@acme.fr\r\nTo: alice@example.com\r\nSubject: Facture\r\n\
Message-ID: <inv1@acme.fr>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"m\"\r\n\r\n\
--m\r\nContent-Type: text/plain\r\n\r\nCi-joint.\r\n\
--m\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"facture.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--m--\r\n";
        let p = mail::parse::parse(raw.as_bytes(), false, &[], "x").unwrap();
        let m = mail::poller::store(&env.db, &row, "INBOX", 1, p, &[])
            .await
            .unwrap()
            .unwrap();
        let path = |i: usize| format!("/api/v1/mailboxes/{id}/messages/{}/attachments/{i}/media", m.id);

        // Bob does not read Alice's own mailbox: it does not exist for him.
        let (st, e) = call(&server, "POST", &path(0), &env.bob, org, None).await;
        assert_eq!((st, e["error"].clone()), (404, json!("mail-mailbox-not-found")), "{e}");
        let (st, e) = call(&server, "POST", &path(1), &env.alice, org, None).await;
        assert_eq!((st, e["error"].clone()), (404, json!("mail-attachment-not-found")), "{e}");
        // PneX keeps no attachment bytes: the server is asked again.
        let (st, e) = call(&server, "POST", &path(0), &env.alice, org, None).await;
        assert_eq!((st, e["error"].clone()), (502, json!("mail-unreachable")), "{e}");

        // Once saved, the same media is answered without the server.
        use sea_orm::ConnectionTrait;
        let media = uuid::Uuid::new_v4();
        env.db
            .execute_unprepared(&format!(
                "INSERT INTO media_assets (id, org_id, kind, name) VALUES ('{media}', {org}, 'document', 'facture.pdf'); \
                 UPDATE mail_messages SET attachments = jsonb_set(attachments, '{{0,media_id}}', '\"{media}\"') \
                 WHERE id = '{}'",
                m.id
            ))
            .await
            .unwrap();
        let (st, a) = call(&server, "POST", &path(0), &env.alice, org, None).await;
        assert_eq!((st, a["media_id"].clone()), (200, json!(media.to_string())), "{a}");
        let (_, l) = call(&server, "GET", &format!("/api/v1/mailboxes/{id}/messages"), &env.alice, org, None).await;
        assert_eq!(l["results"][0]["attachments"][0]["media_id"], json!(media.to_string()));
    })
    .await;
}
