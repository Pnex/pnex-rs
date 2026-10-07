//! Org secrets vault (secrets.md D110–D117): CRUD without values, AEAD at
//! rest, org isolation, viewer restrictions, usages + 409 on delete,
//! secret-field resolution (pick / dedicated value).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{
    flow_versions, flows, notify_channels, org_secrets, organization_members,
    sea_orm_active_enums::OrgMemberRole, secret_usages,
};
use pnex_backend::services::secrets::store::{self, StoreError, Writer};
use pnex_backend::services::secrets::Keyring;
use pnex_core::{SecretConsumerKind, SecretFieldInput};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serde_json::{json, Value};
use serial_test::serial;
use uuid::Uuid;

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "sec-alice-000000000000000",
        "alice",
        "sec-alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "sec-bob-00000000000000000",
        "bob",
        "sec-bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            f(server, ctx, alice, bob).await;
        },
    )
    .await;
}

/// (user id, personal org id).
async fn provision(server: &axum_test::TestServer, token: &str) -> (i64, i64) {
    let body: Value = server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json();
    (
        body["id"].as_i64().unwrap(),
        body["orgs"][0]["id"].as_i64().unwrap(),
    )
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
        "PATCH" => server.patch(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", bearer(token))
    .add_header("X-Org-Id", org.to_string());
    let resp = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = resp.status_code().as_u16();
    let json = if resp.text().is_empty() {
        Value::Null
    } else {
        resp.json()
    };
    (status, json)
}

fn ring(ctx: &loco_rs::app::AppContext) -> Keyring {
    Keyring::from_config(&ctx.config).expect("test keyring")
}

#[tokio::test]
#[serial]
async fn crud_never_returns_values_and_encrypts_at_rest() {
    with_app(|server, ctx, alice, _bob| async move {
        let (_, org) = provision(&server, &alice).await;

        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": " telegram-bot ", "description": "On-call bot", "value": "tg-SECRET-123" })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        assert_eq!(created["name"], "telegram-bot");
        assert!(!created.to_string().contains("tg-SECRET-123"));
        let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();

        // At rest: ciphertext only, bound to the org.
        let row = org_secrets::Entity::find_by_id(id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.org_id, Some(org));
        assert!(!row
            .ciphertext
            .windows(b"tg-SECRET".len())
            .any(|w| w == b"tg-SECRET"));
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), id)
                .await
                .unwrap(),
            "tg-SECRET-123"
        );

        // List + search, no value anywhere.
        call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "smtp-password", "value": "pw" })),
        )
        .await;
        let (s, list) = call(&server, "GET", "/api/v1/secrets", &alice, org, None).await;
        assert_eq!(s, 200);
        assert_eq!(list["count"], 2);
        assert_eq!(list["results"][0]["name"], "smtp-password");
        assert_eq!(list["results"][1]["description"], "On-call bot");
        assert_eq!(list["results"][1]["updated_by"], "sec-alice@example.com");
        assert!(!list.to_string().contains("tg-SECRET"));
        let (_, found) = call(
            &server,
            "GET",
            "/api/v1/secrets?search=TELE",
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(found["count"], 1);

        // Rename keeps the value; a new value replaces it.
        let (s, _) = call(
            &server,
            "PUT",
            &format!("/api/v1/secrets/{id}"),
            &alice,
            org,
            Some(json!({ "name": "telegram-oncall" })),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), id)
                .await
                .unwrap(),
            "tg-SECRET-123"
        );
        let (s, _) = call(
            &server,
            "PUT",
            &format!("/api/v1/secrets/{id}"),
            &alice,
            org,
            Some(json!({ "name": "telegram-oncall", "value": "rotated" })),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), id)
                .await
                .unwrap(),
            "rotated"
        );

        // Validation + name uniqueness.
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "  ", "value": "x" })),
        )
        .await;
        assert_eq!((s, body["name"].as_str()), (400, Some("required")));
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "no-value" })),
        )
        .await;
        assert_eq!((s, body["value"].as_str()), (400, Some("required")));
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "smtp-password", "value": "x" })),
        )
        .await;
        assert_eq!((s, body["error"].as_str()), (409, Some("secret-name-taken")));

        // Delete.
        let path = format!("/api/v1/secrets/{id}");
        assert_eq!(call(&server, "DELETE", &path, &alice, org, None).await.0, 204);
        let (s, body) = call(&server, "DELETE", &path, &alice, org, None).await;
        assert_eq!((s, body["error"].as_str()), (404, Some("secret-not-found")));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn other_orgs_and_viewers_cannot_write_or_read_details() {
    with_app(|server, ctx, alice, bob| async move {
        let (_, alice_org) = provision(&server, &alice).await;
        let (bob_id, bob_org) = provision(&server, &bob).await;
        let (_, created) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            alice_org,
            Some(json!({ "name": "api-key", "description": "internal", "value": "v" })),
        )
        .await;
        let id = created["id"].as_str().unwrap().to_string();

        // Bob's own org does not see it, and cannot touch it by id.
        let (_, list) = call(&server, "GET", "/api/v1/secrets", &bob, bob_org, None).await;
        assert_eq!(list["count"], 0);
        let (s, _) = call(
            &server,
            "PUT",
            &format!("/api/v1/secrets/{id}"),
            &bob,
            bob_org,
            Some(json!({ "name": "stolen", "value": "x" })),
        )
        .await;
        assert_eq!(s, 404);
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(bob_org), id.parse().unwrap())
                .await
                .unwrap_err()
                .to_string(),
            StoreError::NotFound.to_string()
        );

        // Bob as viewer of Alice's org: names only, no writes.
        organization_members::ActiveModel {
            org_id: Set(alice_org),
            user_id: Set(bob_id),
            role: Set(OrgMemberRole::Viewer),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let (s, list) = call(&server, "GET", "/api/v1/secrets", &bob, alice_org, None).await;
        assert_eq!(s, 200);
        assert_eq!(list["results"][0]["name"], "api-key");
        assert!(list["results"][0]["description"].is_null());
        assert!(list["results"][0]["updated_by"].is_null());
        for (method, path, body) in [
            (
                "POST",
                "/api/v1/secrets".to_string(),
                Some(json!({ "name": "n", "value": "v" })),
            ),
            (
                "PUT",
                format!("/api/v1/secrets/{id}"),
                Some(json!({ "name": "n" })),
            ),
            ("DELETE", format!("/api/v1/secrets/{id}"), None),
        ] {
            let (s, body) = call(&server, method, &path, &bob, alice_org, body).await;
            assert_eq!(
                (s, body["error"].as_str()),
                (403, Some("secret-write-forbidden")),
                "{method} {path}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn usages_block_delete_and_dedicated_secrets_follow_their_consumer() {
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, org) = provision(&server, &alice).await;
        let (_, bob_org) = provision(&server, &bob).await;
        let kr = ring(&ctx);
        let writer = Writer {
            org_id: Some(org),
            user_id: Some(alice_id),
        };

        // Typed value → dedicated secret, re-typed value → same row replaced.
        let dedicated = "notify/oncall/token";
        let first = store::resolve_field(
            &ctx.db,
            &kr,
            writer,
            true,
            &SecretFieldInput::Value { value: "t1".into() },
            dedicated,
        )
        .await
        .unwrap();
        let second = store::resolve_field(
            &ctx.db,
            &kr,
            writer,
            true,
            &SecretFieldInput::Value { value: "t2".into() },
            dedicated,
        )
        .await
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(store::reveal(&ctx.db, &kr, Some(org), first).await.unwrap(), "t2");

        // A member without vault rights may only pick.
        assert!(matches!(
            store::resolve_field(
                &ctx.db,
                &kr,
                writer,
                false,
                &SecretFieldInput::Value { value: "x".into() },
                dedicated,
            )
            .await,
            Err(StoreError::WriteForbidden)
        ));
        let shared = store::create(&ctx.db, &kr, writer, "shared-token", None, "s")
            .await
            .unwrap()
            .id;
        assert_eq!(
            store::resolve_field(
                &ctx.db,
                &kr,
                writer,
                false,
                &SecretFieldInput::Pick { secret_id: shared },
                dedicated,
            )
            .await
            .unwrap(),
            shared
        );
        // Picking another org's secret fails as not found.
        let foreign = Writer {
            org_id: Some(bob_org),
            user_id: None,
        };
        assert!(matches!(
            store::resolve_field(
                &ctx.db,
                &kr,
                foreign,
                true,
                &SecretFieldInput::Pick { secret_id: shared },
                "x",
            )
            .await,
            Err(StoreError::NotFound)
        ));

        // Usages: listed, block delete.
        store::set_usages(
            &ctx.db,
            SecretConsumerKind::NotifyChannel,
            "17",
            &[("token".into(), first), ("fallback".into(), shared)],
        )
        .await
        .unwrap();
        let (_, list) = call(
            &server,
            "GET",
            "/api/v1/secrets?search=shared",
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            list["results"][0]["usages"],
            json!([{ "kind": "notify-channel", "consumer_id": "17", "field": "fallback", "label": null }])
        );
        let (s, body) = call(
            &server,
            "DELETE",
            &format!("/api/v1/secrets/{shared}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!((s, body["error"].as_str()), (409, Some("secret-in-use")));

        // Consumer deleted: its dedicated secret goes, the shared one stays.
        store::release_consumer(
            &ctx.db,
            Some(org),
            SecretConsumerKind::NotifyChannel,
            "17",
            "notify/oncall/",
        )
        .await
        .unwrap();
        assert!(org_secrets::Entity::find_by_id(first)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/secrets/{shared}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 204);
    })
    .await;
}

// ───────────────────── Lot S4: notification channels ─────────────────────

async fn stored_channel(ctx: &loco_rs::app::AppContext, id: &str) -> notify_channels::Model {
    notify_channels::Entity::find_by_id(id.parse::<Uuid>().unwrap())
        .one(&ctx.db)
        .await
        .unwrap()
        .expect("channel row")
}

async fn usages_of_consumer(ctx: &loco_rs::app::AppContext, consumer: &str) -> Vec<(String, Uuid)> {
    let mut out: Vec<(String, Uuid)> = secret_usages::Entity::find()
        .filter(secret_usages::Column::ConsumerId.eq(consumer))
        .all(&ctx.db)
        .await
        .unwrap()
        .into_iter()
        .map(|u| (u.field, u.secret_id))
        .collect();
    out.sort();
    out
}

#[tokio::test]
#[serial]
async fn notify_channel_secrets_live_in_the_vault() {
    with_app(|server, ctx, alice, _bob| async move {
        let (_, org) = provision(&server, &alice).await;

        // Typed as a bare string (legacy API shape): lands in the
        // dedicated secret, the row only keeps a reference.
        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "oncall", "enabled": true,
                "config": { "bot_token": "123:FIRST", "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        assert!(!created.to_string().contains("123:FIRST"));
        assert_eq!(
            created["secrets"]["bot_token"]["name"],
            "notify/oncall/bot_token"
        );
        assert_eq!(created["secrets_set"], json!(["bot_token"]));
        let channel_id = created["id"].as_str().unwrap().to_string();
        let dedicated: Uuid = created["secrets"]["bot_token"]["secret_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();

        let row = stored_channel(&ctx, &channel_id).await;
        assert!(
            !row.config.to_string().contains("123:FIRST"),
            "{}",
            row.config
        );
        assert_eq!(row.config["bot_token"]["secret_id"], dedicated.to_string());
        assert_eq!(row.config["chat_id"], "@oncall");
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), dedicated)
                .await
                .unwrap(),
            "123:FIRST"
        );
        assert_eq!(
            usages_of_consumer(&ctx, &channel_id).await,
            [("bot_token".to_string(), dedicated)]
        );

        // `null` keeps it; `{"value"}` replaces the dedicated secret in
        // place; a rename moves the dedicated secret along.
        let (s, updated) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "oncall", "enabled": true,
                "config": { "bot_token": null, "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!(s, 200, "{updated}");
        assert_eq!(
            updated["secrets"]["bot_token"]["secret_id"],
            dedicated.to_string()
        );
        let (s, updated) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "astreinte", "enabled": true,
                "config": { "bot_token": { "value": "456:SECOND" }, "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!(s, 200, "{updated}");
        assert_eq!(
            updated["secrets"]["bot_token"]["secret_id"],
            dedicated.to_string()
        );
        assert_eq!(
            updated["secrets"]["bot_token"]["name"],
            "notify/astreinte/bot_token"
        );
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), dedicated)
                .await
                .unwrap(),
            "456:SECOND"
        );

        // Validation runs on the resolved value (a bad typed token is a 400,
        // nothing is written).
        let (s, _) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "astreinte", "enabled": true,
                "config": { "bot_token": { "value": "has space" }, "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!(s, 400);

        // Picking a shared secret drops the now unused dedicated one.
        let shared = store::create(
            &ctx.db,
            &ring(&ctx),
            Writer {
                org_id: Some(org),
                user_id: None,
            },
            "shared-bot",
            None,
            "789:SHARED",
        )
        .await
        .unwrap()
        .id;
        let (s, picked) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "astreinte", "enabled": true,
                "config": { "bot_token": { "secret_id": shared }, "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!(s, 200, "{picked}");
        assert_eq!(picked["secrets"]["bot_token"]["name"], "shared-bot");
        assert!(org_secrets::Entity::find_by_id(dedicated)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());

        // A secret of another org cannot be picked.
        let (_, other_org) = provision(&server, &_bob).await;
        let foreign = store::create(
            &ctx.db,
            &ring(&ctx),
            Writer {
                org_id: Some(other_org),
                user_id: None,
            },
            "foreign",
            None,
            "000:FOREIGN",
        )
        .await
        .unwrap()
        .id;
        let (s, body) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "astreinte", "enabled": true,
                "config": { "bot_token": { "secret_id": foreign }, "chat_id": "@oncall" }
            })),
        )
        .await;
        assert_eq!((s, body["error"].as_str()), (404, Some("secret-not-found")));

        // Deleting the channel releases its usage, the shared secret stays.
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(usages_of_consumer(&ctx, &channel_id).await.is_empty());
        assert!(org_secrets::Entity::find_by_id(shared)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn runtime_reads_a_secret_only_through_a_deployed_flow() {
    const TOKEN: &str = "secrets-runtime-token";
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", TOKEN) };
    with_app(|server, ctx, alice, bob| async move {
        let (_, org) = provision(&server, &alice).await;
        let (_, other_org) = provision(&server, &bob).await;
        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &alice,
            org,
            Some(json!({
                "kind": "telegram", "name": "alerts", "enabled": true,
                "config": { "bot_token": "123:RUNTIME", "chat_id": "@alerts" }
            })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        let channel_id = created["id"].as_str().unwrap().to_string();
        let secret = created["secrets"]["bot_token"]["secret_id"]
            .as_str()
            .unwrap()
            .to_string();

        let fetch = |org_id: i64, token: &'static str| {
            let path = format!("/internal/flow/secret/{secret}?org_id={org_id}");
            let server = &server;
            async move {
                let resp = server
                    .get(&path)
                    .add_header("x-pnex-flow-token", token)
                    .await;
                (resp.status_code().as_u16(), resp.text())
            }
        };

        // No deployed flow sends on the channel yet.
        let (s, body) = fetch(org, TOKEN).await;
        assert_eq!(s, 404, "{body}");
        assert!(!body.contains("123:RUNTIME"));

        let flow = flows::ActiveModel {
            name: Set("alerting".into()),
            status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let version = flow_versions::ActiveModel {
            flow_id: Set(flow.id),
            version_number: Set(1),
            graph: Set(json!({ "nodes": [{
                "id": "n1", "kind": "pnex_notify",
                "config": { "channel_ids": [channel_id], "template_id": null }
            }] })),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let mut am: flows::ActiveModel = flow.into();
        am.deployed_version_id = Set(Some(version.id));
        am.update(&ctx.db).await.unwrap();

        let (s, body) = fetch(org, TOKEN).await;
        assert_eq!(s, 200, "{body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["value"],
            "123:RUNTIME"
        );

        // Wrong token, or another org's runtime: refused.
        assert_eq!(fetch(org, "wrong").await.0, 401);
        assert_eq!(fetch(other_org, TOKEN).await.0, 404);

        // A disabled channel no longer opens its secret.
        let mut ch: notify_channels::ActiveModel = stored_channel(&ctx, &channel_id).await.into();
        ch.enabled = Set(false);
        ch.update(&ctx.db).await.unwrap();
        assert_eq!(fetch(org, TOKEN).await.0, 404);
    })
    .await;
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
}

// ───────────────────── Lot S5: http-fetch node secrets ─────────────────────

fn http_fetch_graph(auth: Value) -> Value {
    json!({ "nodes": [{
        "id": "h1", "kind": "http_fetch",
        "config": { "url": "https://api.example.dev/x", "auth": auth }
    }] })
}

async fn latest_graph(ctx: &loco_rs::app::AppContext, flow_id: i64) -> Value {
    use sea_orm::QueryOrder;
    flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(&ctx.db)
        .await
        .unwrap()
        .expect("version")
        .graph
}

async fn mark_deployed(ctx: &loco_rs::app::AppContext, flow_id: i64) {
    use sea_orm::QueryOrder;
    let version = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let flow = flows::Entity::find_by_id(flow_id)
        .one(&ctx.db)
        .await
        .unwrap()
        .unwrap();
    let mut am: flows::ActiveModel = flow.into();
    am.deployed_version_id = Set(Some(version.id));
    am.status = Set(pnex_core::FLOW_STATUS_DEPLOYED.into());
    am.update(&ctx.db).await.unwrap();
}

#[tokio::test]
#[serial]
async fn http_fetch_secrets_never_stay_in_the_graph() {
    const TOKEN: &str = "secrets-s5-runtime-token";
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", TOKEN) };
    with_app(|server, ctx, alice, bob| async move {
        let (_, org) = provision(&server, &alice).await;
        let (_, other_org) = provision(&server, &bob).await;

        // Typed bearer token: the stored graph and the response only hold
        // a reference to the node field's dedicated secret.
        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/flows",
            &alice,
            org,
            Some(json!({
                "name": "collect",
                "graph": http_fetch_graph(json!({"mode": "bearer", "token": "tok-PLAIN-1"})),
            })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        assert!(!created.to_string().contains("tok-PLAIN-1"), "{created}");
        let flow_id = created["id"].as_i64().unwrap();
        let stored = latest_graph(&ctx, flow_id).await;
        assert!(!stored.to_string().contains("tok-PLAIN-1"), "{stored}");
        let secret: Uuid = stored["nodes"][0]["config"]["auth"]["token"]["secret_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let row = store::find(&ctx.db, Some(org), secret).await.unwrap();
        assert_eq!(row.name, format!("flow/{flow_id}/h1/auth.token"));
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), secret).await.unwrap(),
            "tok-PLAIN-1"
        );
        assert_eq!(
            usages_of_consumer(&ctx, &flow_id.to_string()).await,
            [("h1/auth.token".to_string(), secret)]
        );

        // A new typed value replaces the dedicated secret in place.
        let (s, updated) = call(
            &server,
            "PATCH",
            &format!("/api/v1/flows/{flow_id}"),
            &alice,
            org,
            Some(json!({
                "expected_version_number": 1,
                "graph": http_fetch_graph(json!({"mode": "bearer", "token": "tok-PLAIN-2"})),
            })),
        )
        .await;
        assert_eq!(s, 200, "{updated}");
        assert_eq!(
            updated["graph"]["nodes"][0]["config"]["auth"]["token"]["secret_id"],
            secret.to_string()
        );
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), secret).await.unwrap(),
            "tok-PLAIN-2"
        );

        // In use: the vault refuses to delete it.
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/secrets/{secret}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 409);

        // A secret of another org cannot be referenced.
        let foreign = store::create(
            &ctx.db,
            &ring(&ctx),
            Writer { org_id: Some(other_org), user_id: None },
            "foreign-token",
            None,
            "tok-FOREIGN",
        )
        .await
        .unwrap()
        .id;
        let (s, body) = call(
            &server,
            "PATCH",
            &format!("/api/v1/flows/{flow_id}"),
            &alice,
            org,
            Some(json!({
                "expected_version_number": 2,
                "graph": http_fetch_graph(json!({"mode": "bearer", "token": {"secret_id": foreign}})),
            })),
        )
        .await;
        assert_eq!((s, body["error"].as_str()), (404, Some("secret-not-found")));

        // Runtime: refused until deployed, then served.
        let fetch = |token: &'static str| {
            let path = format!("/internal/flow/secret/{secret}?org_id={org}");
            let server = &server;
            async move {
                let resp = server.get(&path).add_header("x-pnex-flow-token", token).await;
                (resp.status_code().as_u16(), resp.text())
            }
        };
        assert_eq!(fetch(TOKEN).await.0, 404);
        mark_deployed(&ctx, flow_id).await;
        let (s, body) = fetch(TOKEN).await;
        assert_eq!(s, 200, "{body}");
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["value"], "tok-PLAIN-2");

        // Deleting the flow drops its dedicated secret.
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/flows/{flow_id}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(org_secrets::Entity::find_by_id(secret)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());
    })
    .await;
    unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
}

// ───────────────────── R9 / SEC-W2: secret bound to its destination ─────────────────────

fn fetch_graph_to(url: &str, secret: &str) -> Value {
    json!({ "nodes": [{
        "id": "h1", "kind": "http_fetch",
        "config": { "url": url, "auth": {"mode": "bearer", "token": {"secret_id": secret}} }
    }] })
}

#[tokio::test]
#[serial]
async fn members_keep_secrets_but_never_rewire_them() {
    with_app(|server, _ctx, alice, bob| async move {
        let (_, org) = provision(&server, &alice).await;
        provision(&server, &bob).await;
        let (s, body) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &alice,
            org,
            Some(json!({ "email": "sec-bob@example.com", "role": "member" })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s} {body}");
        let (_, created) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "api", "value": "tok-R9" })),
        )
        .await;
        let secret = created["id"].as_str().unwrap().to_string();
        let locked = |s: u16, body: &Value| {
            assert_eq!(
                (s, body["error"].as_str()),
                (403, Some("secret-destination-locked")),
                "{body}"
            )
        };

        // The owner wires the secret toward api.example.dev.
        let (s, flow) = call(
            &server,
            "POST",
            "/api/v1/flows",
            &alice,
            org,
            Some(json!({ "name": "r9", "graph": fetch_graph_to("https://api.example.dev/a", &secret) })),
        )
        .await;
        assert_eq!(s, 201, "{flow}");
        let flow_id = flow["id"].as_i64().unwrap();
        let patch = |token: &str, version: i64, url: &str| {
            let body = json!({
                "expected_version_number": version,
                "graph": fetch_graph_to(url, &secret),
            });
            let path = format!("/api/v1/flows/{flow_id}");
            let token = token.to_string();
            let server = &server;
            async move { call(server, "PATCH", &path, &token, org, Some(body)).await }
        };

        // (1) Member keeps it toward the same origin (path change only).
        let (s, body) = patch(&bob, 1, "https://api.example.dev/b?x=1").await;
        assert_eq!(s, 200, "{body}");
        // (2) Member points it to another host, port or scheme: refused.
        for url in [
            "https://collector.attacker.example/a",
            "https://api.example.dev:8443/a",
            "http://api.example.dev/a",
        ] {
            let (s, body) = patch(&bob, 2, url).await;
            locked(s, &body);
        }
        // (3) Member reuses it toward the same destination in ANOTHER flow:
        // refused too, a member never wires a secret.
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/flows",
            &bob,
            org,
            Some(json!({ "name": "copy", "graph": fetch_graph_to("https://api.example.dev/a", &secret) })),
        )
        .await;
        locked(s, &body);
        // (4) Owner moves it: allowed, and the new binding holds for the member.
        let (s, body) = patch(&alice, 2, "https://api2.example.dev/a").await;
        assert_eq!(s, 200, "{body}");
        let (s, body) = patch(&bob, 3, "https://api2.example.dev/c").await;
        assert_eq!(s, 200, "{body}");

        // (5) Webhook channel wired by the owner; member renames it, but
        // cannot change its URL.
        let (s, channel) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &alice,
            org,
            Some(json!({
                "kind": "webhook", "name": "hook", "enabled": true,
                "config": {
                    "url": "https://hooks.example.dev/in",
                    "secret_header": "X-Token",
                    "secret_value": { "secret_id": secret }
                }
            })),
        )
        .await;
        assert_eq!(s, 201, "{channel}");
        let channel_id = channel["id"].as_str().unwrap().to_string();
        let put = |name: &str, url: &str| {
            json!({
                "kind": "webhook", "name": name, "enabled": true,
                "config": { "url": url, "secret_header": "X-Token", "secret_value": null }
            })
        };
        let (s, body) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &bob,
            org,
            Some(put("hook-renamed", "https://hooks.example.dev/in")),
        )
        .await;
        assert_eq!(s, 200, "{body}");
        let (s, body) = call(
            &server,
            "PUT",
            &format!("/api/v1/notify/channels/{channel_id}"),
            &bob,
            org,
            Some(put("hook-renamed", "https://collector.attacker.example/in")),
        )
        .await;
        locked(s, &body);

        // (6) Test drafts: the stored channel pointed elsewhere, or a
        // picked secret on a new draft, are refused before any send.
        let mut draft = put("hook-renamed", "https://collector.attacker.example/in");
        draft["channel_id"] = json!(channel_id);
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/notify/channels/test-draft",
            &bob,
            org,
            Some(draft),
        )
        .await;
        locked(s, &body);
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/notify/channels/test-draft",
            &bob,
            org,
            Some(json!({
                "kind": "webhook", "name": "new",
                "config": {
                    "url": "https://collector.attacker.example/in",
                    "secret_header": "X-Token",
                    "secret_value": { "secret_id": secret }
                }
            })),
        )
        .await;
        locked(s, &body);

        // (7) WiFi: the member keeps the password, never swaps it nor
        // renames the SSID it is sent to.
        let (s, wifi) = call(
            &server,
            "POST",
            "/api/v1/edge/wifi-credentials",
            &alice,
            org,
            Some(json!({ "ssid": "Home", "password": { "value": "wifi-R9" } })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s} {wifi}");
        let wifi_path = format!("/api/v1/edge/wifi-credentials/{}", wifi["id"]);
        for body in [
            json!({ "ssid": "Home", "password": { "secret_id": secret } }),
            json!({ "ssid": "Attacker-AP" }),
        ] {
            let (s, out) = call(&server, "PUT", &wifi_path, &bob, org, Some(body)).await;
            locked(s, &out);
        }
        let (s, out) = call(
            &server,
            "PUT",
            &wifi_path,
            &bob,
            org,
            Some(json!({ "ssid": "Home" })),
        )
        .await;
        assert_eq!(s, 200, "{out}");
    })
    .await;
}

// ───────────────────── Lot S6: WiFi credentials ─────────────────────

#[tokio::test]
#[serial]
async fn wifi_password_lives_in_the_vault() {
    with_app(|server, ctx, alice, _bob| async move {
        use pnex_backend::models::_entities::wifi_credentials;
        let (_, org) = provision(&server, &alice).await;

        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/edge/wifi-credentials",
            &alice,
            org,
            Some(json!({ "ssid": "Maison", "password": { "value": "wifi-PLAIN-1" } })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        assert!(!created.to_string().contains("wifi-PLAIN-1"));
        let id = created["id"].as_i64().unwrap();
        let secret: Uuid = created["password"]["secret_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(created["password"]["name"], "wifi/Maison/password");
        let row = wifi_credentials::Entity::find_by_id(id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.secret_id, Some(secret));
        assert_eq!(
            usages_of_consumer(&ctx, &id.to_string()).await,
            [("password".to_string(), secret)]
        );

        // Rename without a password: kept, the dedicated secret follows.
        let (s, renamed) = call(
            &server,
            "PUT",
            &format!("/api/v1/edge/wifi-credentials/{id}"),
            &alice,
            org,
            Some(json!({ "ssid": "Atelier" })),
        )
        .await;
        assert_eq!(s, 200, "{renamed}");
        assert_eq!(renamed["password"]["secret_id"], secret.to_string());
        assert_eq!(renamed["password"]["name"], "wifi/Atelier/password");
        assert_eq!(
            store::reveal(&ctx.db, &ring(&ctx), Some(org), secret)
                .await
                .unwrap(),
            "wifi-PLAIN-1"
        );

        // Picking a shared secret drops the dedicated one.
        let shared = store::create(
            &ctx.db,
            &ring(&ctx),
            Writer {
                org_id: Some(org),
                user_id: None,
            },
            "site-wifi",
            None,
            "wifi-SHARED",
        )
        .await
        .unwrap()
        .id;
        let (s, picked) = call(
            &server,
            "PUT",
            &format!("/api/v1/edge/wifi-credentials/{id}"),
            &alice,
            org,
            Some(json!({ "ssid": "Atelier", "password": { "secret_id": shared } })),
        )
        .await;
        assert_eq!(s, 200, "{picked}");
        assert_eq!(picked["password"]["name"], "site-wifi");
        assert!(org_secrets::Entity::find_by_id(secret)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none());

        // Delete: usage released, the shared secret stays.
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/edge/wifi-credentials/{id}"),
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(usages_of_consumer(&ctx, &id.to_string()).await.is_empty());
        assert!(org_secrets::Entity::find_by_id(shared)
            .one(&ctx.db)
            .await
            .unwrap()
            .is_some());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn members_never_wire_secrets_nor_administer() {
    with_app(|server, _ctx, alice, bob| async move {
        let (_, org) = provision(&server, &alice).await;
        let (bob_id, _) = provision(&server, &bob).await;

        // The owner grants the `member` role through the API.
        let (s, body) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &alice,
            org,
            Some(json!({ "email": "sec-bob@example.com", "role": "member" })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s} {body}");
        let (_, detail) = call(
            &server,
            "GET",
            &format!("/api/v1/orgs/{org}"),
            &alice,
            org,
            None,
        )
        .await;
        let bob_row = detail["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["user_id"] == bob_id)
            .cloned()
            .unwrap();
        assert_eq!(bob_row["role"], "member");

        let (s, created) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &alice,
            org,
            Some(json!({ "name": "tg", "value": "123:SHARED" })),
        )
        .await;
        assert_eq!(s, 201, "{created}");
        let secret_id = created["id"].as_str().unwrap().to_string();

        // Vault writes stay owner/admin.
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/secrets",
            &bob,
            org,
            Some(json!({ "name": "mine", "value": "v" })),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("secret-write-forbidden"))
        );

        // A member edits channels but cannot type a value...
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &bob,
            org,
            Some(json!({
                "kind": "telegram", "name": "typed", "enabled": true,
                "config": { "bot_token": { "value": "999:TYPED" }, "chat_id": "@x" }
            })),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("secret-write-forbidden")),
            "{body}"
        );

        // ...nor pick one (R9, SEC-W2: only owner/admin wire secrets)...
        let picked = json!({
            "kind": "telegram", "name": "picked", "enabled": true,
            "config": { "bot_token": { "secret_id": secret_id }, "chat_id": "@x" }
        });
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &bob,
            org,
            Some(picked.clone()),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("secret-destination-locked")),
            "{body}"
        );

        // ...but edits a channel the owner wired, whose value never shows.
        let (s, channel) = call(
            &server,
            "POST",
            "/api/v1/notify/channels",
            &alice,
            org,
            Some(picked),
        )
        .await;
        assert_eq!(s, 201, "{channel}");
        let (s, channel) = call(
            &server,
            "PUT",
            &format!(
                "/api/v1/notify/channels/{}",
                channel["id"].as_str().unwrap()
            ),
            &bob,
            org,
            Some(json!({
                "kind": "telegram", "name": "picked", "enabled": true,
                "config": { "bot_token": null, "chat_id": "@y" }
            })),
        )
        .await;
        assert_eq!(s, 200, "{channel}");
        assert_eq!(channel["secrets"]["bot_token"]["name"], "tg");
        assert!(!channel.to_string().contains("123:SHARED"));

        // Org governance stays owner/admin.
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &bob,
            org,
            Some(json!({ "name": "p", "kind": "anthropic", "model": "m1" })),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("llm-provider-forbidden"))
        );
        let (s, body) = call(
            &server,
            "PATCH",
            &format!("/api/v1/orgs/{org}"),
            &bob,
            org,
            Some(json!({ "name": "renamed" })),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("org-rename-forbidden"))
        );
        let (s, body) = call(
            &server,
            "PATCH",
            &format!("/api/v1/orgs/{org}/members/{bob_id}"),
            &bob,
            org,
            Some(json!({ "role": "admin" })),
        )
        .await;
        assert_eq!(
            (s, body["error"].as_str()),
            (403, Some("org-member-role-forbidden"))
        );
    })
    .await;
}

fn test_key(byte: u8) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode([byte; 32])
}

#[tokio::test]
#[serial]
async fn rekey_moves_every_readable_row_to_the_write_key() {
    use pnex_backend::services::secrets::rekey;
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, org) = provision(&server, &alice).await;
        provision(&server, &bob).await;
        let old = Keyring::parse(&format!("old:{}", test_key(1))).unwrap();
        let lost = Keyring::parse(&format!("lost:{}", test_key(2))).unwrap();
        let rotated = Keyring::parse(&format!("new:{},old:{}", test_key(3), test_key(1))).unwrap();
        let only_new = Keyring::parse(&format!("new:{}", test_key(3))).unwrap();
        let base = rekey::stats(&ctx.db, &rotated).await.unwrap();

        let org_row = store::create(
            &ctx.db,
            &old,
            Writer {
                org_id: Some(org),
                user_id: Some(alice_id),
            },
            "org-key",
            None,
            "org-value",
        )
        .await
        .unwrap();
        let platform_row = store::create(
            &ctx.db,
            &old,
            Writer {
                org_id: None,
                user_id: None,
            },
            "platform-key",
            None,
            "platform-value",
        )
        .await
        .unwrap();
        let lost_row = store::create(
            &ctx.db,
            &lost,
            Writer {
                org_id: Some(org),
                user_id: None,
            },
            "lost-key",
            None,
            "gone",
        )
        .await
        .unwrap();

        let before = rekey::stats(&ctx.db, &rotated).await.unwrap();
        assert_eq!(before.total, base.total + 3);
        assert_eq!(before.stale, base.stale + 3);
        assert_eq!(before.unknown_key, base.unknown_key + 1);

        let report = rekey::rekey(&ctx.db, &rotated).await.unwrap();
        assert_eq!(report.rewritten, base.stale - base.unknown_key + 2);
        assert_eq!(report.unreadable, base.unknown_key + 1);

        // Rewritten rows read with the new key alone, value and audit
        // fields unchanged; the unreadable row is untouched.
        for (row, value) in [(&org_row, "org-value"), (&platform_row, "platform-value")] {
            assert_eq!(
                store::reveal(&ctx.db, &only_new, row.org_id, row.id)
                    .await
                    .unwrap(),
                value
            );
            let stored = org_secrets::Entity::find_by_id(row.id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.key_id, "new");
            assert_eq!(stored.updated_at, row.updated_at);
        }
        let stored = org_secrets::Entity::find_by_id(lost_row.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (stored.key_id.as_str(), &stored.nonce),
            ("lost", &lost_row.nonce)
        );
        assert_eq!(
            store::reveal(&ctx.db, &lost, Some(org), lost_row.id)
                .await
                .unwrap(),
            "gone"
        );

        let after = rekey::stats(&ctx.db, &rotated).await.unwrap();
        assert_eq!(after.stale, after.unknown_key);
        // Idempotent.
        assert_eq!(rekey::rekey(&ctx.db, &rotated).await.unwrap().rewritten, 0);

        // HTTP: platform admin only; reports what the server keyring
        // cannot read.
        let (s, _) = call(
            &server,
            "POST",
            "/api/v1/system/secrets/rekey",
            &bob,
            org,
            None,
        )
        .await;
        assert_eq!(s, 403);
        let mut admin: pnex_backend::models::_entities::users::ActiveModel =
            pnex_backend::models::_entities::users::Entity::find_by_id(alice_id)
                .one(&ctx.db)
                .await
                .unwrap()
                .unwrap()
                .into();
        admin.platform_admin = Set(true);
        admin.update(&ctx.db).await.unwrap();
        let server_ring = ring(&ctx);
        let expected = rekey::stats(&ctx.db, &server_ring).await.unwrap();
        let (s, body) = call(
            &server,
            "POST",
            "/api/v1/system/secrets/rekey",
            &alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200, "{body}");
        assert_eq!(body["remaining"], expected.unknown_key);
        assert_eq!(body["unreadable"], expected.unknown_key);
    })
    .await;
}
