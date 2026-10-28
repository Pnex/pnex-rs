//! Notification tests (D49–D54): channel/template CRUD, secret masking
//! (D54), uniqueness 409, org isolation, preview, webhook test-draft
//! against a local receiver, internal endpoint + WS bus + journal.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

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
    unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
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
            // The websocket test-draft loops back through the deliver
            // endpoint; its URL default resolves to the dev server on
            // :5150 when the config section is absent. Retarget it at this
            // test server so the loopback lands here, not on a live server.
            if let Some(addr) = server.server_address() {
                let base = addr.to_string().trim_end_matches('/').to_string();
                unsafe {
                    std::env::set_var(
                        "PNEX_NOTIFY_DELIVER_URL",
                        format!("{base}/internal/notify/deliver"),
                    );
                }
            }
            common::seed_catalogue(&ctx.db).await;
            f(server, env).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

const INTERNAL_TOKEN: &str = "test-internal-token";

/// Local webhook receiver (axum) — returns (url, seen) where seen holds
/// (headers, body) of the last request.
async fn spawn_webhook_receiver() -> (
    String,
    std::sync::Arc<tokio::sync::Mutex<Option<(axum::http::HeaderMap, serde_json::Value)>>>,
) {
    use axum::routing::post;
    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let seen_for_handler = seen.clone();
    let app = axum::Router::new().route(
        "/hook",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                let seen = seen_for_handler.clone();
                async move {
                    *seen.lock().await = Some((headers, body));
                    axum::http::StatusCode::OK
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/hook"), seen)
}

use axum::Json;

/// Creates a channel and returns (id, response payload).
async fn create_channel(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    body: serde_json::Value,
) -> (String, serde_json::Value) {
    let resp = server
        .post("/api/v1/notify/channels")
        .add_header("Content-Type", "application/json")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    let status = resp.status_code();
    let json = resp.json::<serde_json::Value>();
    assert_eq!(status, 201, "channel creation: {json}");
    (json["id"].as_str().unwrap().to_string(), json)
}

#[tokio::test]
#[serial]
async fn kinds_expose_the_shipped_channels() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let kinds = server
            .get("/api/v1/notify/kinds")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        let list = kinds["results"].as_array().expect("kinds page");
        let names: Vec<&str> = list.iter().map(|k| k["kind"].as_str().unwrap()).collect();
        assert!(names.contains(&"websocket"), "{names:?}");
        assert!(names.contains(&"webhook"), "{names:?}");
        assert!(names.contains(&"ntfy"), "{names:?}");
        // notify-kinds.v1 contract: websocket without fields, webhook with a
        // required url + write-only secret, ntfy with required server/topic +
        // optional secret token.
        let webhook = list.iter().find(|k| k["kind"] == "webhook").unwrap();
        let fields = webhook["field_spec"].as_array().unwrap();
        assert!(fields
            .iter()
            .any(|f| f["id"] == "url" && f["required"] == true));
        assert!(fields
            .iter()
            .any(|f| f["id"] == "secret_value" && f["type"] == "secret"));
        let ntfy = list.iter().find(|k| k["kind"] == "ntfy").unwrap();
        let fields = ntfy["field_spec"].as_array().unwrap();
        assert!(fields
            .iter()
            .any(|f| f["id"] == "server" && f["required"] == true));
        assert!(fields
            .iter()
            .any(|f| f["id"] == "topic" && f["required"] == true));
        assert!(fields
            .iter()
            .any(|f| f["id"] == "token" && f["type"] == "secret" && f["required"] == false));
        // Telegram: secret bot_token required + chat_id required.
        let telegram = list.iter().find(|k| k["kind"] == "telegram").unwrap();
        let fields = telegram["field_spec"].as_array().unwrap();
        assert!(fields
            .iter()
            .any(|f| f["id"] == "bot_token" && f["type"] == "secret" && f["required"] == true));
        assert!(fields
            .iter()
            .any(|f| f["id"] == "chat_id" && f["required"] == true));
        // Slack / Discord: webhook_url carries a secret (D65).
        for kind in ["slack", "discord"] {
            let k = list.iter().find(|x| x["kind"] == kind).unwrap();
            let fields = k["field_spec"].as_array().unwrap();
            assert!(
                fields.iter().any(|f| f["id"] == "webhook_url"
                    && f["type"] == "secret"
                    && f["required"] == true),
                "{kind}"
            );
        }
        // SMTP: secret password, tls as a select with its 3 options.
        let smtp = list.iter().find(|k| k["kind"] == "smtp").unwrap();
        let fields = smtp["field_spec"].as_array().unwrap();
        assert!(fields
            .iter()
            .any(|f| f["id"] == "password" && f["type"] == "secret" && f["required"] == false));
        assert!(fields.iter().any(|f| f["id"] == "tls"
            && f["type"] == "select"
            && f["options"] == serde_json::json!(["starttls", "tls", "none"])));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn crud_cycle_masking_and_409() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (id, created) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "kind": "webhook",
                "name": "domotique",
                "enabled": true,
                "config": {
                    "url": "https://home.example/hook",
                    "secret_header": "x-sig",
                    "secret_value": { "value": "s3cr3t" }
                }
            }),
        )
        .await;

        // ── D54: the secret is NEVER returned, only its presence is.
        assert_eq!(created["config"]["secret_value"], serde_json::Value::Null);
        assert_eq!(created["secrets_set"], serde_json::json!(["secret_value"]));
        assert_eq!(created["config"]["url"], "https://home.example/hook");

        // ── PUT without the secret: kept (registry merge).
        let updated = server
            .put(&format!("/api/v1/notify/channels/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "webhook",
                "name": "domotique",
                "enabled": false,
                "config": { "url": "https://home.example/hook2", "secret_header": "x-sig" }
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(updated["enabled"], false);
        assert_eq!(updated["config"]["url"], "https://home.example/hook2");
        assert_eq!(updated["config"]["secret_value"], serde_json::Value::Null);
        assert_eq!(updated["secrets_set"], serde_json::json!(["secret_value"]));

        // ── (org, name) uniqueness: 409 (differs from viz: natural key).
        let clash = server
            .post("/api/v1/notify/channels")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "webhook", "name": "domotique",
                "config": { "url": "https://x.example" }
            }))
            .await;
        assert_eq!(clash.status_code(), 409);

        // ── Validation: unknown kind and missing URL → field 400.
        let bad_kind = server
            .post("/api/v1/notify/channels")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "kind": "gotify", "name": "n", "config": {} }))
            .await;
        assert_eq!(bad_kind.status_code(), 400);
        assert_eq!(
            bad_kind.json::<serde_json::Value>()["errors"]["fields"]["kind"],
            "invalid"
        );
        let bad_url = server
            .post("/api/v1/notify/channels")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "kind": "webhook", "name": "n", "config": {} }))
            .await;
        assert_eq!(bad_url.status_code(), 400);
        let bad_url = bad_url.json::<serde_json::Value>();
        assert!(
            bad_url["error"] == "notify-channel-config-invalid"
                || bad_url["error"] == "notify-channel-field-required",
            "{bad_url}"
        );

        // ── Delete 204.
        let deleted = server
            .delete(&format!("/api/v1/notify/channels/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deleted.status_code(), 204);
    })
    .await;
}

/// D54 on a second kind: the bot_token is masked on GET, `secrets_set`
/// lists it, and a PUT without the secret keeps it (registry merge).
#[tokio::test]
#[serial]
async fn crud_cycle_masking_telegram() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (id, created) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "kind": "telegram",
                "name": "bot-alertes",
                "enabled": true,
                "config": {
                    "bot_token": { "value": "123:ABCDEF" },
                    "chat_id": "-1001234567890"
                }
            }),
        )
        .await;

        assert_eq!(created["config"]["bot_token"], serde_json::Value::Null);
        assert_eq!(created["secrets_set"], serde_json::json!(["bot_token"]));
        assert_eq!(created["config"]["chat_id"], "-1001234567890");

        // PUT without the secret: kept.
        let updated = server
            .put(&format!("/api/v1/notify/channels/{id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "telegram",
                "name": "bot-alertes",
                "enabled": true,
                "config": { "chat_id": "@moncanal" }
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(updated["config"]["chat_id"], "@moncanal");
        assert_eq!(updated["config"]["bot_token"], serde_json::Value::Null);
        assert_eq!(updated["secrets_set"], serde_json::json!(["bot_token"]));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_viewer() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (id, _) = create_channel(
            &server,
            &env.alice,
            org_a,
            serde_json::json!({
                "kind": "websocket", "name": "bus", "config": {}
            }),
        )
        .await;

        // Bob on his org: empty list, masked detail 404.
        let list = server
            .get("/api/v1/notify/channels")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(list["count"], 0);
        let cross = server
            .get(&format!("/api/v1/notify/channels/{id}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .await;
        assert_eq!(cross.status_code(), 404);

        // Viewer of org A: reads, doesn't write.
        server
            .post(&format!("/api/v1/orgs/{org_a}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let read = server
            .get("/api/v1/notify/channels")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(read.status_code(), 200);
        let denied = server
            .post("/api/v1/notify/channels")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org_a.to_string())
            .json(&serde_json::json!({ "kind": "websocket", "name": "v", "config": {} }))
            .await;
        assert_eq!(denied.status_code(), 403);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn templates_crud_and_preview() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = server
            .post("/api/v1/notify/templates")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "seuil",
                "subject": "[{{ device }}] alerte",
                "body": "⚠️ {{ device }} : {{ value }} (seuil {{ vars.seuil }}, {{ meta.ts }})",
                "vars": [ { "name": "seuil", "example": "80" } ]
            }))
            .await;
        assert_eq!(created.status_code(), 201);
        let tpl = created.json::<serde_json::Value>();
        let id = tpl["id"].as_str().unwrap().to_string();

        // Preview: missing vars → example; payload exposed at the root.
        let preview = server
            .post(&format!("/api/v1/notify/templates/{id}/preview"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "payload": { "device": "chaudiere", "value": 82.5 }
            }))
            .await
            .json::<serde_json::Value>();
        assert_eq!(preview["subject"], "[chaudiere] alerte");
        assert!(
            preview["body"].as_str().unwrap().contains("seuil 80"),
            "{preview}"
        );
        assert!(preview["body"].as_str().unwrap().contains("82.5"));

        // Preview with a var override.
        let preview2 = server
            .post(&format!("/api/v1/notify/templates/{id}/preview"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "vars": { "seuil": "90" },
                "payload": { "device": "d", "value": 1 }
            }))
            .await
            .json::<serde_json::Value>();
        assert!(preview2["body"].as_str().unwrap().contains("seuil 90"));

        // Syntactically broken template → 400 body field.
        let bad = server
            .post("/api/v1/notify/templates")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "name": "cassé", "body": "{% if %}" }))
            .await;
        assert_eq!(bad.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_draft_and_test_webhook_channel() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (url, seen) = spawn_webhook_receiver().await;

        // ── Test-draft before saving: the receiver gets the payload.
        let draft = server
            .post("/api/v1/notify/channels/test-draft")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "webhook",
                "name": "draft",
                "config": { "url": url, "secret_header": "x-sig", "secret_value": { "value": "tok" } }
            }))
            .await;
        assert_eq!(draft.status_code(), 204, "{}", draft.text());
        let (headers, body) = seen.lock().await.take().expect("webhook received");
        assert_eq!(
            headers.get("x-sig").and_then(|v| v.to_str().ok()),
            Some("tok")
        );
        assert!(body["subject"].as_str().unwrap().contains("draft"));
        assert_eq!(body["meta"]["test"], true);

        // ── Test of a persisted channel: journaled with source test.
        let (id, _) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "kind": "webhook", "name": "hook", "enabled": false,
                "config": { "url": url }
            }),
        )
        .await;
        // enabled:false is bypassed by the test (that's its purpose).
        let tested = server
            .post(&format!("/api/v1/notify/channels/{id}/test"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(tested.status_code(), 204, "{:?}", tested.text());

        let deliveries = server
            .get(&format!("/api/v1/notify/deliveries?channel_id={id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        // The journal lives in OpenObserve (D86), absent from the test
        // config: the read is a coded 503 (the O2 round trip is
        // `tests/notify_journal.rs`, `--ignored`).
        assert_eq!(deliveries.status_code(), 503, "{}", deliveries.text());
        assert_eq!(
            deliveries.json::<serde_json::Value>()["error"],
            "notify-journal-unavailable"
        );

        // ── No journal, no badge (never an error).
        let detail = server
            .get(&format!("/api/v1/notify/channels/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert!(detail["last_status"].is_null(), "{detail}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn test_channel_with_rendered_template() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (url, seen) = spawn_webhook_receiver().await;

        // Template with a declared var (example 80) + persisted webhook channel.
        let created = server
            .post("/api/v1/notify/templates")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "name": "alerte",
                "subject": "[{{ device }}] seuil dépassé",
                "body": "⚠️ {{ device }} : {{ value }} (seuil {{ vars.seuil }})",
                "vars": [ { "name": "seuil", "example": "80" } ]
            }))
            .await;
        assert_eq!(created.status_code(), 201, "{created:?}");
        let tpl_id = created.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (id, _) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({
                "kind": "webhook", "name": "hook", "enabled": false,
                "config": { "url": url }
            }),
        )
        .await;

        // ── Test with a template: the rendered message goes out on the channel
        //    (the var override wins over the declared example).
        let tested = server
            .post(&format!("/api/v1/notify/channels/{id}/test"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "template_id": tpl_id,
                "vars": { "seuil": "95", "device": "chaudiere", "value": "97.5" }
            }))
            .await;
        assert_eq!(tested.status_code(), 204, "{}", tested.text());
        let (headers, body) = seen.lock().await.take().expect("webhook received");
        let _ = headers;
        assert_eq!(body["subject"], "[chaudiere] seuil dépassé");
        assert_eq!(body["body"], "⚠️ chaudiere : 97.5 (seuil 95)");
        assert_eq!(body["meta"]["test"], true);

        // ── Unknown template → 404.
        let missing = server
            .post(&format!("/api/v1/notify/channels/{id}/test"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "template_id": uuid::Uuid::nil() }))
            .await;
        assert_eq!(missing.status_code(), 404);
    })
    .await;
}

// Fail-closed internal endpoint: without `PNEX_NOTIFY_INTERNAL_TOKEN`, every
// delivery is 401 (and nothing is journaled).

/// Test-draft of an ntfy channel against a local receiver — full path
/// validate → send (JSON publication `{topic, message, title}` to
/// `{server}/`).
#[tokio::test]
#[serial]
async fn test_draft_ntfy_against_local_receiver() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        // ntfy receiver: the channel publishes to `{server}/` as JSON.
        use axum::routing::post;
        let seen = std::sync::Arc::new(tokio::sync::Mutex::new(None));
        let seen_for_handler = seen.clone();
        let app = axum::Router::new().route(
            "/",
            post(
                move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
                    let seen = seen_for_handler.clone();
                    async move {
                        *seen.lock().await = Some((headers, body));
                        axum::http::StatusCode::OK
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let draft = server
            .post("/api/v1/notify/channels/test-draft")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "ntfy",
                "name": "push",
                "config": {
                    "server": format!("http://{addr}"),
                    "topic": "alertes",
                    "token": { "value": "tk_test" },
                    "priority": "high",
                    "tags": "warning"
                }
            }))
            .await;
        assert_eq!(draft.status_code(), 204, "{}", draft.text());
        let (headers, body) = seen.lock().await.take().expect("ntfy publication received");
        assert_eq!(
            headers.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer tk_test")
        );
        assert_eq!(body["topic"], "alertes");
        assert!(body["title"].as_str().unwrap().contains("push"));
        assert_eq!(body["priority"], 4, "JSON API: numeric priority");
        assert_eq!(body["tags"], serde_json::json!(["warning"]));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn internal_without_token_refused() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (id, _) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({ "kind": "websocket", "name": "bus", "config": {} }),
        )
        .await;
        let resp = server
            .post("/internal/notify/deliver")
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "org_id": org, "channel_id": id,
                "subject": "s", "body": "b"
            }))
            .await;
        assert_eq!(resp.status_code(), 401);
        // Nothing journaled: without OpenObserve the org has no journal at
        // all (coded 503).
        let deliveries = server
            .get(&format!("/api/v1/notify/deliveries?channel_id={id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(deliveries.status_code(), 503, "{}", deliveries.text());
    })
    .await;
}

/// Full chain: subscribed WS + internal endpoint (valid token) → frame
/// received + journal (source flow). Wrong token → 401.
#[tokio::test]
#[serial]
async fn internal_delivers_ws_frame_and_logs() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (id, _) = create_channel(
            &server,
            &env.alice,
            org,
            serde_json::json!({ "kind": "websocket", "name": "bus", "config": {} }),
        )
        .await;

        // Bad token → 401.
        let bad = server
            .post("/internal/notify/deliver")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-internal-token", "wrong")
            .json(&serde_json::json!({
                "org_id": org, "channel_id": id, "body": "b"
            }))
            .await;
        assert_eq!(bad.status_code(), 401);

        // WS subscriber (valid JWT + org).
        unsafe { std::env::set_var("PNEX_NOTIFY_INTERNAL_TOKEN", INTERNAL_TOKEN) };
        let mut ws = server
            .get_websocket(&format!(
                "/ws/notify?ticket={}",
                common::ws_ticket(&server, &env.alice, org).await
            ))
            .await
            .into_websocket()
            .await;

        // Internal delivery → 200 {delivered: 1}.
        let resp = server
            .post("/internal/notify/deliver")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-internal-token", INTERNAL_TOKEN)
            .json(&serde_json::json!({
                "org_id": org, "channel_id": id,
                "subject": "Alerte seuil",
                "body": "chaudiere : 82.5 °C",
                "meta": { "flow": 12 }
            }))
            .await;
        assert_eq!(resp.status_code(), 200, "{:?}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["delivered"], 1);

        // The JSON frame is received by the subscriber (filtering out the
        // heartbeat Pings, whose first tick is immediate).
        let frame = loop {
            let m = ws.receive_message().await;
            if let axum_test::WsMessage::Text(t) = m {
                break t;
            }
        };
        let item: serde_json::Value = serde_json::from_str(&frame).expect("frame JSON");
        assert_eq!(item["subject"], "Alerte seuil");
        assert_eq!(item["body"], "chaudiere : 82.5 °C");
        assert_eq!(item["meta"]["flow"], 12);
        drop(ws);

        // The journal entry goes to O2 (absent here): the delivery itself
        // never depends on it.
        let journal = server
            .post("/internal/notify/journal")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-internal-token", INTERNAL_TOKEN)
            .json(&serde_json::json!({
                "org_id": org, "channel_id": id, "source": "flow", "status": "failed",
                "http_status": 502, "error": "HTTP 502"
            }))
            .await;
        assert_eq!(journal.status_code(), 202, "{:?}", journal.text());
        // A channel of another org (or unknown) is refused.
        let foreign = server
            .post("/internal/notify/journal")
            .add_header("Content-Type", "application/json")
            .add_header("x-pnex-internal-token", INTERNAL_TOKEN)
            .json(&serde_json::json!({
                "org_id": org, "channel_id": uuid::Uuid::nil(), "source": "flow", "status": "sent"
            }))
            .await;
        assert_eq!(foreign.status_code(), 404);
        let filtered = server
            .get("/api/v1/notify/deliveries?status=loud")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(filtered.status_code(), 400);
        assert_eq!(
            filtered.json::<serde_json::Value>()["errors"]["fields"]["status"],
            "invalid"
        );
        unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
    })
    .await;
}

/// Websocket channel test-draft (before creation): the draft has no DB row
/// — deliver must publish the frame on the org bus without lookup or
/// journal (2026-09-23 regression: deliver 404).
#[tokio::test]
#[serial]
async fn test_draft_websocket_draft_reaches_the_bus() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        unsafe { std::env::set_var("PNEX_NOTIFY_INTERNAL_TOKEN", INTERNAL_TOKEN) };

        // WS subscriber (valid JWT + org) before the draft is sent.
        let mut ws = server
            .get_websocket(&format!(
                "/ws/notify?ticket={}",
                common::ws_ticket(&server, &env.alice, org).await
            ))
            .await
            .into_websocket()
            .await;

        // Websocket draft: nil channel_id on the deliver side, no row created.
        let draft = server
            .post("/api/v1/notify/channels/test-draft")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({
                "kind": "websocket",
                "name": "brouillon-ws",
                "config": {}
            }))
            .await;
        assert_eq!(draft.status_code(), 204, "{:?}", draft.text());

        // The draft's frame is received by the subscriber (heartbeat Pings
        // filtered — first tick immediate).
        let frame = loop {
            let m = ws.receive_message().await;
            if let axum_test::WsMessage::Text(t) = m {
                break t;
            }
        };
        let item: serde_json::Value = serde_json::from_str(&frame).expect("frame JSON");
        assert_eq!(item["subject"], "PNeX test — brouillon-ws");
        assert_eq!(item["meta"]["test"], true);
        assert_eq!(item["id"], 0);

        // No channel created by the test-draft.
        let channels = server
            .get("/api/v1/notify/channels")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(channels["count"], 0, "{channels}");
        unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
    })
    .await;
}

/// WS without a ticket → 4002; no ticket for a foreign org; replayed ticket → 4001.
#[tokio::test]
#[serial]
async fn ws_notify_auth_rejections() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        // Without a token: the connection is accepted then closed (4002).
        let mut ws = server
            .get_websocket("/ws/notify")
            .await
            .into_websocket()
            .await;
        let msg = ws.receive_message().await;
        assert!(matches!(msg, axum_test::WsMessage::Close(Some(f)) if u16::from(f.code) == 4002));

        // No ticket for an org one is not a member of.
        let refused = server
            .post("/api/v1/ws-ticket")
            .add_header("Authorization", format!("Bearer {}", env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert!(refused.status_code().is_client_error());

        // A ticket opens one socket only: replayed → 4001.
        let ticket = common::ws_ticket(&server, &env.alice, org).await;
        let _first = server
            .get_websocket(&format!("/ws/notify?ticket={ticket}"))
            .await
            .into_websocket()
            .await;
        let mut replay = server
            .get_websocket(&format!("/ws/notify?ticket={ticket}"))
            .await
            .into_websocket()
            .await;
        let msg = replay.receive_message().await;
        assert!(matches!(msg, axum_test::WsMessage::Close(Some(f)) if u16::from(f.code) == 4001));
    })
    .await;
}
