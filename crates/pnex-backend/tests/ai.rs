//! Integration tests of the AI assistant: kill-switch, LLM providers (CRUD,
//! key held by the vault, roles, the org default drives the assistant), full
//! turn with the tool loop (flow created as a draft, NEVER deployed),
//! forbidden tools refused, provider errors.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.
//! Le LLM est un **mock HTTP local** (`spawn_mock_llm`) : script de
//! réponses + enregistrement des requêtes (pattern `spawn_mock_o2`).

mod common;

use std::collections::VecDeque;
use std::sync::Mutex;

use axum::routing::post;
use axum::Router;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;
use tokio::net::TcpListener;

// ─────────────────────────── Mock LLM ───────────────────────────

#[derive(Debug, Clone)]
enum MockReply {
    Ok(serde_json::Value),
    Status(u16, serde_json::Value),
}

#[derive(Default)]
struct MockLlmState {
    requests: Mutex<Vec<serde_json::Value>>,
    script: Mutex<VecDeque<MockReply>>,
}

impl MockLlmState {
    fn record(&self, body: serde_json::Value) {
        self.requests.lock().expect("lock").push(body);
    }

    fn next_reply(&self) -> MockReply {
        self.script
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or(MockReply::Ok(serde_json::json!({
                "choices": [{
                    "message": {"content": "fin", "tool_calls": null},
                    "finish_reason": "stop"
                }]
            })))
    }
}

static MOCK: std::sync::OnceLock<MockLlmState> = std::sync::OnceLock::new();

fn mock() -> &'static MockLlmState {
    MOCK.get_or_init(MockLlmState::default)
}
use axum::response::IntoResponse;
use axum::Json;

async fn spawn_mock_llm() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock llm");
    let addr = listener.local_addr().expect("addr mock llm");
    async fn handler(
        axum::extract::State(state): axum::extract::State<&'static MockLlmState>,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        state.record(parsed);
        match state.next_reply() {
            MockReply::Ok(v) => Json(v).into_response(),
            MockReply::Status(code, v) => (
                axum::http::StatusCode::from_u16(code).expect("code"),
                Json(v),
            )
                .into_response(),
        }
    }
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .route("/v1/messages", post(handler))
        .with_state(mock());
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock llm");
    });
    let url = format!("http://{addr}/v1");
    *MOCK_URL.lock().expect("lock") = url.clone();
    url
}

static MOCK_URL: Mutex<String> = Mutex::new(String::new());

/// URL of the mock LLM of the running test.
fn mock_url() -> String {
    MOCK_URL.lock().expect("lock").clone()
}

fn reply_tool_call(id: &str, name: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "choices": [{
            "message": {
                "content": null,
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": args.to_string()}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

fn reply_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "choices": [{
            "message": {"content": text, "tool_calls": null},
            "finish_reason": "stop"
        }]
    })
}

struct Env {
    alice: String,
    bob: String,
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
        .expect("org personnelle")
}

async fn with_app_ai<F, Fut>(enabled: bool, with_provider: bool, f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_AI_ENABLED", if enabled { "true" } else { "false" }) };
    let mock_url = if with_provider {
        Some(spawn_mock_llm().await)
    } else {
        None
    };
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
            if let Some(url) = mock_url {
                let org = personal_org(&server, &env.alice).await;
                org_provider(&ctx, org, &url).await;
            }
            f(server, env, ctx).await;
        },
    )
    .await;
}

/// Default provider of alice's org pointing at the mock LLM.
async fn org_provider(ctx: &loco_rs::app::AppContext, org: i64, url: &str) {
    use pnex_backend::services::ai::providers::{self, Author};
    let ring = pnex_backend::services::secrets::Keyring::from_config(&ctx.config).expect("keyring");
    providers::create(
        &ctx.db,
        &ring,
        org,
        Author {
            user_id: None,
            can_write_secrets: true,
        },
        &pnex_core::LlmProviderInput {
            name: "mock".into(),
            kind: "openai_compat".into(),
            base_url: Some(url.to_string()),
            model: "test-model".into(),
            api_key: Some(pnex_core::SecretFieldInput::Value {
                value: "test-key".into(),
            }),
            is_default: true,
        },
    )
    .await
    .expect("org provider");
}

fn reset_mock() {
    mock().requests.lock().expect("lock").clear();
    mock().script.lock().expect("lock").clear();
}

fn mock_requests() -> Vec<serde_json::Value> {
    mock().requests.lock().expect("lock").clone()
}

fn push_reply(reply: MockReply) {
    mock().script.lock().expect("lock").push_back(reply);
}

async fn chat_send(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    user_text: &str,
) -> axum_test::TestResponse {
    let conv = new_conversation(server, token, org_id).await;
    send_in(server, token, org_id, &conv, user_text).await
}

/// Creates an empty conversation, returns its id.
async fn new_conversation(server: &axum_test::TestServer, token: &str, org_id: i64) -> String {
    let created = server
        .post("/api/v1/ai/conversations")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .await;
    assert_eq!(created.status_code(), 201, "{}", created.text());
    created.json::<serde_json::Value>()["id"]
        .as_str()
        .expect("conversation id")
        .to_string()
}

/// One turn in an existing conversation (only the new message is sent).
async fn send_in(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    conversation: &str,
    user_text: &str,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/ai/conversations/{conversation}/messages"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({ "content": user_text }))
        .await
}

// ─────────────────────────── Tests ───────────────────────────

/// (1) Kill-switch OFF → status enabled:false, chat 403, aucune requête au mock LLM.
#[tokio::test]
#[serial]
async fn kill_switch_coupe_tout() {
    with_app_ai(false, false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        let status = server
            .get("/api/v1/ai/status")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(status["enabled"], false, "{status}");
        assert_eq!(status["configured"], false);

        let resp = chat_send(&server, &env.alice, org, "salut").await;
        assert_eq!(resp.status_code(), 403, "{}", resp.text());
        assert!(
            mock_requests().is_empty(),
            "aucune requête ne doit atteindre le mock LLM"
        );
    })
    .await;
}

fn provider_body(
    name: &str,
    kind: &str,
    base_url: Option<&str>,
    key: Option<&str>,
) -> serde_json::Value {
    let mut body = serde_json::json!({ "name": name, "kind": kind, "model": "m1" });
    if let Some(url) = base_url {
        body["base_url"] = serde_json::json!(url);
    }
    if let Some(key) = key {
        body["api_key"] = serde_json::json!({ "value": key });
    }
    body
}

async fn send_json(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<serde_json::Value>,
) -> axum_test::TestResponse {
    let req = match method {
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "PATCH" => server.patch(path),
        "DELETE" => server.delete(path),
        _ => server.get(path),
    }
    .add_header("Authorization", bearer(token))
    .add_header("X-Org-Id", org.to_string());
    match body {
        Some(body) => {
            req.add_header("Content-Type", "application/json")
                .json(&body)
                .await
        }
        None => req.await,
    }
}

/// (2) Provider CRUD: the key goes to the vault (reference only, never
/// returned), the first provider becomes the default, a single default per
/// org, field validation, unique names, delete drops the dedicated key.
#[tokio::test]
#[serial]
async fn providers_crud_keep_the_key_in_the_vault() {
    with_app_ai(false, false, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        let created = send_json(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &env.alice,
            org,
            Some(provider_body("main", "anthropic", None, Some("sk-orj-123"))),
        )
        .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        assert!(!created.text().contains("sk-orj-123"), "key never returned");
        let main = created.json::<serde_json::Value>();
        assert_eq!(main["api_key"]["name"], "llm/main/api_key", "{main}");
        assert_eq!(main["is_default"], true, "first provider = default");
        let main_id = main["id"].as_str().expect("id").to_string();

        // The vault holds the value: ciphertext only.
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        let row = pnex_backend::models::_entities::org_secrets::Entity::find()
            .filter(pnex_backend::models::_entities::org_secrets::Column::Name.eq("llm/main/api_key"))
            .one(&ctx.db)
            .await
            .expect("query")
            .expect("dedicated secret");
        assert!(!String::from_utf8_lossy(&row.ciphertext).contains("sk-orj-123"));

        // Update without api_key keeps it.
        let kept = send_json(
            &server,
            "PUT",
            &format!("/api/v1/ai/providers/{main_id}"),
            &env.alice,
            org,
            Some(serde_json::json!({ "name": "main", "kind": "anthropic", "model": "m2", "is_default": true })),
        )
        .await;
        assert_eq!(kept.status_code(), 200, "{}", kept.text());
        let kept = kept.json::<serde_json::Value>();
        assert_eq!(kept["api_key"]["secret_id"], main["api_key"]["secret_id"]);
        assert_eq!(kept["model"], "m2");

        // A second default takes the flag over.
        let second = send_json(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &env.alice,
            org,
            Some({
                let mut b = provider_body("local", "openai_compat", Some("http://127.0.0.1:9/v1"), Some("k2"));
                b["is_default"] = serde_json::json!(true);
                b
            }),
        )
        .await;
        assert_eq!(second.status_code(), 201, "{}", second.text());
        let list = send_json(&server, "GET", "/api/v1/ai/providers", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        let defaults: Vec<&str> = list
            .as_array()
            .expect("array")
            .iter()
            .filter(|p| p["is_default"] == true)
            .map(|p| p["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(defaults, vec!["local"], "{list}");

        // Validation and name conflict.
        for bad in [
            provider_body("x", "mistral", None, Some("k")),
            provider_body("x", "openai_compat", None, Some("k")),
            provider_body("x", "openai_compat", Some("http://h/v1/"), Some("k")),
            provider_body("x", "anthropic", None, None),
            provider_body("", "anthropic", None, Some("k")),
        ] {
            let resp = send_json(&server, "POST", "/api/v1/ai/providers", &env.alice, org, Some(bad.clone())).await;
            assert_eq!(resp.status_code(), 400, "{bad} → {}", resp.text());
        }
        let dup = send_json(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &env.alice,
            org,
            Some(provider_body("main", "anthropic", None, Some("k"))),
        )
        .await;
        assert_eq!(dup.status_code(), 409, "{}", dup.text());
        assert!(dup.text().contains("llm-provider-name-taken"), "{}", dup.text());

        // Delete drops the dedicated key.
        let del = send_json(
            &server,
            "DELETE",
            &format!("/api/v1/ai/providers/{main_id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(del.status_code(), 204, "{}", del.text());
        let secrets = send_json(&server, "GET", "/api/v1/secrets", &env.alice, org, None).await.text();
        assert!(!secrets.contains("llm/main/api_key"), "{secrets}");
        assert!(secrets.contains("llm/local/api_key"), "{secrets}");
    })
    .await;
}

/// (3) Viewer: may list, may not write.
#[tokio::test]
#[serial]
async fn viewer_cannot_manage_providers() {
    with_app_ai(false, false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        // bob logs in once (JIT user provisioning) before being added.
        let _ = personal_org(&server, &env.bob).await;
        let add = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"email": "bob@example.com", "role": "viewer"}))
            .await;
        assert_eq!(add.status_code(), 201, "{}", add.text());

        let list = send_json(&server, "GET", "/api/v1/ai/providers", &env.bob, org, None).await;
        assert_eq!(list.status_code(), 200, "{}", list.text());
        let create = send_json(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &env.bob,
            org,
            Some(provider_body("main", "anthropic", None, Some("k"))),
        )
        .await;
        assert_eq!(create.status_code(), 403, "{}", create.text());
        assert!(
            create.text().contains("llm-provider-forbidden"),
            "{}",
            create.text()
        );
    })
    .await;
}

/// (4) The org runs on its default provider (no platform fallback, D119);
/// switching the default switches the assistant; the test ping hits the
/// provider once.
#[tokio::test]
#[serial]
async fn the_org_default_provider_drives_the_assistant() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        let status = send_json(&server, "GET", "/api/v1/ai/status", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        assert_eq!(status["configured"], true, "{status}");
        assert_eq!(status["provider_name"], "mock");
        assert_eq!(status["model"], "test-model");

        let list = send_json(
            &server,
            "GET",
            "/api/v1/ai/providers",
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        let rows = list.as_array().expect("array");
        assert_eq!(rows.len(), 1, "only the org's own providers: {list}");
        assert_eq!(rows[0]["is_default"], true, "{list}");
        assert!(!rows[0]["api_key"].is_null(), "{list}");

        let mock_url = mock_url();
        let created = send_json(
            &server,
            "POST",
            "/api/v1/ai/providers",
            &env.alice,
            org,
            Some({
                let mut body =
                    provider_body("mine", "openai_compat", Some(&mock_url), Some("org-key"));
                body["is_default"] = serde_json::json!(true);
                body
            }),
        )
        .await;
        assert_eq!(created.status_code(), 201, "{}", created.text());
        let id = created.json::<serde_json::Value>()["id"]
            .as_str()
            .expect("id")
            .to_string();

        let status = send_json(&server, "GET", "/api/v1/ai/status", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        assert_eq!(status["provider_name"], "mine", "{status}");
        assert_eq!(status["model"], "m1");

        let before = mock_requests().len();
        let test = send_json(
            &server,
            "POST",
            &format!("/api/v1/ai/providers/{id}/test"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(test.status_code(), 200, "{}", test.text());
        let body = test.json::<serde_json::Value>();
        assert_eq!(body["ok"], true, "{body}");
        assert_eq!(mock_requests().len(), before + 1, "one ping");
    })
    .await;
}

/// Graphe canonique inject→device→calc→metric.
fn graph_adc_metric() -> serde_json::Value {
    serde_json::json!({
        "nodes": [
            {"id": "n1", "kind": "inject", "config": {"repeat_secs": 2},
             "outputs": [{"port": 0, "targets": ["n2"]}]},
            {"id": "n2", "kind": "device_read",
             "config": {"device_id": "soil_sensor", "pins": ["A0"]},
             "outputs": [{"port": 0, "targets": ["n3"]}, {"port": 1, "targets": []}]},
            {"id": "n3", "kind": "calc", "config": {"expression": "soil_sensor_A0 * 0.01"},
             "outputs": [{"port": 0, "targets": ["n4"]}]},
            {"id": "n4", "kind": "metric", "config": {"metric_name": "soil_volts"},
             "outputs": [{"port": 0, "targets": []}]}
        ]
    })
}

/// (5) Tour complet : tool_use create_flow → flow en DRAFT v1, AUCUN
/// deploy, le 2e appel LLM porte le tool_result.
#[tokio::test]
#[serial]
async fn tour_complet_cree_un_flow_sans_deployer() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        push_reply(MockReply::Ok(reply_tool_call(
            "t1",
            "create_flow",
            serde_json::json!({"name": "a0→volts", "graph": graph_adc_metric()}),
        )));
        push_reply(MockReply::Ok(reply_text("Flow créé en brouillon.")));

        let resp = chat_send(
            &server,
            &env.alice,
            org,
            "lecture A0 → volts, toutes les 2 s",
        )
        .await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["answer"], "Flow créé en brouillon.");
        assert_eq!(body["tool_trace"][0]["ok"], true, "{body}");
        assert_eq!(body["tool_trace"][0]["name"], "create_flow");
        assert!(
            body["tool_trace"][0]["flow_id"].as_i64().is_some(),
            "trace porte le flow_id (deep link)"
        );

        let flows_list = server
            .get("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(flows_list["count"], 1, "{flows_list}");
        assert_eq!(flows_list["results"][0]["status"], "draft");
        assert_eq!(flows_list["results"][0]["latest_version_number"], 1);

        let reqs = mock_requests();
        assert_eq!(reqs.len(), 2, "2 appels LLM attendus, reçu {}", reqs.len());
        let roles: Vec<&str> = reqs[1]["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter_map(|m| m["role"].as_str())
            .collect();
        assert!(
            roles.contains(&"tool"),
            "le 2e appel doit contenir le tool_result: {roles:?}"
        );
    })
    .await;
}

/// (6) L'agent demande un outil interdit → ok:false, flow jamais déployé.
#[tokio::test]
#[serial]
async fn l_agent_ne_peut_pas_deployer() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        // Un flow existe d'abord (créé via l'API humaine).
        let created = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({
                "name": "cible",
                "graph": graph_adc_metric()
            }))
            .await
            .json::<serde_json::Value>();
        let flow_id = created["id"].as_i64().expect("id");

        push_reply(MockReply::Ok(reply_tool_call(
            "t1",
            "deploy_flow",
            serde_json::json!({"flow_id": flow_id}),
        )));
        push_reply(MockReply::Ok(reply_text(
            "Déploiement impossible, action manuelle requise.",
        )));

        let resp = chat_send(&server, &env.alice, org, "déploie le flow").await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], false, "{body}");
        let summary = body["tool_trace"][0]["summary"]
            .as_str()
            .unwrap_or_default();
        assert!(summary.contains("outil inconnu"), "{summary}");

        // Le flow reste DRAFT (jamais déployé).
        let detail = server
            .get(&format!("/api/v1/flows/{flow_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json::<serde_json::Value>();
        assert_eq!(detail["status"], "draft", "{detail}");
        assert_eq!(detail["deployed_version_number"], serde_json::Value::Null);
    })
    .await;
}

/// Sets a flow status directly (deploying needs the runtime; only the
/// stored status matters to the D143 guard).
async fn set_flow_status(ctx: &loco_rs::app::AppContext, flow_id: i64, status: &str) {
    use pnex_backend::models::_entities::flows;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let row = flows::Entity::find_by_id(flow_id)
        .one(&ctx.db)
        .await
        .expect("read flow")
        .expect("flow exists");
    let mut active: flows::ActiveModel = row.into();
    active.status = Set(status.to_string());
    active.update(&ctx.db).await.expect("update status");
}

/// D143: update_flow refuses a deployed flow with the coded refusal
/// `ai-flow-running` (no new version), accepts it once stopped, and
/// refuses a stale `expected_version` instead of overwriting a human save.
#[tokio::test]
#[serial]
async fn update_flow_only_on_a_stopped_flow() {
    with_app_ai(true, true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created = server
            .post("/api/v1/flows")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"name": "running", "graph": graph_adc_metric()}))
            .await
            .json::<serde_json::Value>();
        let flow_id = created["id"].as_i64().expect("id");
        let update = |version: i64| {
            reply_tool_call(
                "t1",
                "update_flow",
                serde_json::json!({"flow_id": flow_id, "expected_version": version, "graph": graph_adc_metric()}),
            )
        };
        let latest_version = |server: &axum_test::TestServer| {
            let req = server
                .get(&format!("/api/v1/flows/{flow_id}"))
                .add_header("Authorization", bearer(&env.alice))
                .add_header("X-Org-Id", org.to_string());
            async move { req.await.json::<serde_json::Value>()["latest_version_number"].clone() }
        };

        // Deployed → refused, coded, no new version.
        set_flow_status(&ctx, flow_id, "deployed").await;
        reset_mock();
        push_reply(MockReply::Ok(update(1)));
        push_reply(MockReply::Ok(reply_text("Stop it first.")));
        let body = chat_send(&server, &env.alice, org, "edit it").await.json::<serde_json::Value>();
        let trace = &body["tool_trace"][0];
        assert_eq!(trace["ok"], false, "{body}");
        assert_eq!(trace["code"], "ai-flow-running", "{body}");
        assert_eq!(trace["args"]["flow"], "running", "{body}");
        assert_eq!(latest_version(&server).await, 1);
        let tool_result = mock_requests()[1].to_string();
        assert!(tool_result.contains("ai-flow-running"), "the model reads the code");

        // Stopped by the user → accepted as version 2.
        set_flow_status(&ctx, flow_id, "stopped").await;
        reset_mock();
        push_reply(MockReply::Ok(update(1)));
        push_reply(MockReply::Ok(reply_text("Done.")));
        let body = chat_send(&server, &env.alice, org, "edit it").await.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], true, "{body}");
        assert_eq!(latest_version(&server).await, 2);

        // Stale expected_version (the model read v1, v2 exists) → conflict.
        reset_mock();
        push_reply(MockReply::Ok(update(1)));
        push_reply(MockReply::Ok(reply_text("Reloading.")));
        let body = chat_send(&server, &env.alice, org, "edit it").await.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], false, "{body}");
        assert_eq!(latest_version(&server).await, 2);
    })
    .await;
}

/// (7) Fournisseur 401 → 502 avec message actionnable.
#[tokio::test]
#[serial]
async fn echec_auth_repond_actionnable() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        push_reply(MockReply::Status(
            401,
            serde_json::json!({"error": {"message": "invalid x-api-key"}}),
        ));

        let resp = chat_send(&server, &env.alice, org, "salut").await;
        assert_eq!(resp.status_code(), 502, "{}", resp.text());
        let text = resp.text();
        assert!(
            text.contains("ai_auth") || text.contains("Clé API refusée"),
            "message actionnable attendu: {text}"
        );
    })
    .await;
}

/// (8) No provider at all: status not configured, chat refused without
/// any LLM call.
#[tokio::test]
#[serial]
async fn without_provider_the_assistant_is_not_configured() {
    with_app_ai(true, false, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        let status = send_json(&server, "GET", "/api/v1/ai/status", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        assert_eq!(status["enabled"], true, "{status}");
        assert_eq!(status["configured"], false, "{status}");
        let resp = chat_send(&server, &env.alice, org, "salut").await;
        assert_eq!(resp.status_code(), 400, "{}", resp.text());
        assert!(mock_requests().is_empty(), "no LLM call without a provider");
    })
    .await;
}

/// (9) Borne d'itérations : 7 tool_use consécutifs → la boucle force une
/// réponse textuelle finale (outils vides), pas de boucle infinie.
#[tokio::test]
#[serial]
async fn borne_iterations_force_la_reponse_finale() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        reset_mock();

        for i in 0..7 {
            push_reply(MockReply::Ok(reply_tool_call(
                &format!("t{i}"),
                "validate_calc_expression",
                serde_json::json!({"expression": format!("x * {i}")}),
            )));
        }

        let resp = chat_send(&server, &env.alice, org, "va-y").await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        let trace = body["tool_trace"].as_array().expect("trace");
        assert_eq!(trace.len(), 7, "7 outils exécutés puis conclusion forcée");
        assert!(
            !body["answer"].as_str().unwrap_or_default().is_empty(),
            "une réponse textuelle finale est forcée: {body}"
        );
    })
    .await;
}

// ─────────────────────── Conversations (D145) ───────────────────────

/// Adds bob to `org` with `role` (bob logs in once first: JIT user).
async fn add_bob(server: &axum_test::TestServer, env: &Env, org: i64, role: &str) {
    let _ = personal_org(server, &env.bob).await;
    let add = server
        .post(&format!("/api/v1/orgs/{org}/members"))
        .add_header("Authorization", bearer(&env.alice))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({"email": "bob@example.com", "role": role}))
        .await;
    assert_eq!(add.status_code(), 201, "{}", add.text());
}

/// A conversation is private to its author within its org: another
/// member of the same org, and the author from another org, get 404 on
/// read, rename, delete and send; lists carry no tool trace.
#[tokio::test]
#[serial]
async fn conversations_are_private_to_user_and_org() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        // bob's personal org, read before he joins alice's (orgs[0] after).
        let bob_org = personal_org(&server, &env.bob).await;
        assert_ne!(bob_org, org);
        add_bob(&server, &env, org, "admin").await;
        // alice is also a member of bob's org: same user, other org.
        let add = server
            .post(&format!("/api/v1/orgs/{bob_org}/members"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"email": "alice@example.com", "role": "admin"}))
            .await;
        assert_eq!(add.status_code(), 201, "{}", add.text());
        reset_mock();
        push_reply(MockReply::Ok(reply_tool_call(
            "t1",
            "describe_node_types",
            serde_json::json!({"kinds": ["calc"]}),
        )));
        push_reply(MockReply::Ok(reply_text("hello")));
        let conv = new_conversation(&server, &env.alice, org).await;
        let turn = send_in(&server, &env.alice, org, &conv, "my secret plan").await;
        assert_eq!(turn.status_code(), 200, "{}", turn.text());
        let path = format!("/api/v1/ai/conversations/{conv}");

        // bob (admin of the same org) and alice from bob's org: 404.
        for (token, as_org) in [(&env.bob, org), (&env.alice, bob_org)] {
            let get = send_json(&server, "GET", &path, token, as_org, None).await;
            assert_eq!(get.status_code(), 404, "{}", get.text());
            let rename = send_json(
                &server,
                "PATCH",
                &path,
                token,
                as_org,
                Some(serde_json::json!({"title": "x"})),
            )
            .await;
            assert_eq!(rename.status_code(), 404, "{}", rename.text());
            let del = send_json(&server, "DELETE", &path, token, as_org, None).await;
            assert_eq!(del.status_code(), 404, "{}", del.text());
            let send = send_in(&server, token, as_org, &conv, "hi").await;
            assert_eq!(send.status_code(), 404, "{}", send.text());
            let list = send_json(
                &server,
                "GET",
                "/api/v1/ai/conversations",
                token,
                as_org,
                None,
            )
            .await;
            assert_eq!(list.json::<serde_json::Value>()["count"], 0);
        }

        // The author sees it, titled by the first message, without trace in
        // the list; the detail holds both messages and the trace.
        let list = send_json(
            &server,
            "GET",
            "/api/v1/ai/conversations",
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(list["count"], 1, "{list}");
        assert_eq!(list["results"][0]["title"], "my secret plan");
        assert!(!list.to_string().contains("tool_trace"), "{list}");
        let detail = send_json(&server, "GET", &path, &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        let msgs = detail["messages"].as_array().expect("messages");
        assert_eq!(msgs.len(), 2, "{detail}");
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["tool_trace"][0]["name"], "describe_node_types");
    })
    .await;
}

/// The history is rebuilt by the server: the second turn replays the
/// first one (user + assistant) to the model, the client sends only the
/// new message.
#[tokio::test]
#[serial]
async fn history_is_rebuilt_server_side() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let conv = new_conversation(&server, &env.alice, org).await;
        reset_mock();
        push_reply(MockReply::Ok(reply_text("first answer")));
        push_reply(MockReply::Ok(reply_text("second answer")));
        assert_eq!(
            send_in(&server, &env.alice, org, &conv, "first question")
                .await
                .status_code(),
            200
        );
        let second = send_in(&server, &env.alice, org, &conv, "second question").await;
        assert_eq!(second.status_code(), 200, "{}", second.text());
        let sent = mock_requests()[1]["messages"].to_string();
        for part in ["first question", "first answer", "second question"] {
            assert!(sent.contains(part), "{part} missing from {sent}");
        }
    })
    .await;
}

/// A viewer may converse, but write tools are refused inside the tool.
#[tokio::test]
#[serial]
async fn viewer_converses_but_cannot_write() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        add_bob(&server, &env, org, "viewer").await;
        reset_mock();
        push_reply(MockReply::Ok(reply_tool_call(
            "t1",
            "create_flow",
            serde_json::json!({"name": "x", "graph": graph_adc_metric()}),
        )));
        push_reply(MockReply::Ok(reply_text("not allowed")));
        let resp = chat_send(&server, &env.bob, org, "make a flow").await;
        assert_eq!(resp.status_code(), 200, "{}", resp.text());
        let body = resp.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], false, "{body}");
        let flows = send_json(&server, "GET", "/api/v1/flows", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        assert_eq!(flows["count"], 0, "{flows}");
    })
    .await;
}

/// One turn at a time: a held lease → 409 ai-conversation-busy, no LLM call.
#[tokio::test]
#[serial]
async fn one_turn_at_a_time() {
    with_app_ai(true, true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let conv = new_conversation(&server, &env.alice, org).await;
        let id = uuid::Uuid::parse_str(&conv).expect("uuid");
        assert!(
            pnex_backend::services::ai::conversations::acquire(&ctx.db, id)
                .await
                .expect("acquire")
        );
        reset_mock();
        let busy = send_in(&server, &env.alice, org, &conv, "hi").await;
        assert_eq!(busy.status_code(), 409, "{}", busy.text());
        assert!(
            busy.text().contains("ai-conversation-busy"),
            "{}",
            busy.text()
        );
        assert!(mock_requests().is_empty());
        pnex_backend::services::ai::conversations::release(&ctx.db, id).await;
        push_reply(MockReply::Ok(reply_text("ok")));
        assert_eq!(
            send_in(&server, &env.alice, org, &conv, "hi")
                .await
                .status_code(),
            200
        );
    })
    .await;
}

/// Rename, export, delete one, delete all; leaving the org erases the
/// member's conversations there; the purge erases inactive ones.
#[tokio::test]
#[serial]
async fn conversations_crud_export_and_erasure() {
    with_app_ai(true, true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let a = new_conversation(&server, &env.alice, org).await;
        let _b = new_conversation(&server, &env.alice, org).await;
        let path = format!("/api/v1/ai/conversations/{a}");

        let renamed = send_json(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(serde_json::json!({"title": "Pump"})),
        )
        .await;
        assert_eq!(renamed.json::<serde_json::Value>()["title"], "Pump");
        let empty = send_json(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(serde_json::json!({"title": " "})),
        )
        .await;
        assert_eq!(empty.status_code(), 400);

        let export = send_json(
            &server,
            "GET",
            "/api/v1/ai/conversations/export",
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(
            export["conversations"].as_array().expect("list").len(),
            2,
            "{export}"
        );

        assert_eq!(
            send_json(&server, "DELETE", &path, &env.alice, org, None)
                .await
                .status_code(),
            204
        );
        assert_eq!(
            send_json(&server, "GET", &path, &env.alice, org, None)
                .await
                .status_code(),
            404
        );
        let all = send_json(
            &server,
            "DELETE",
            "/api/v1/ai/conversations",
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(all["deleted"], 1, "{all}");

        // bob leaves alice's org → his conversations there are erased.
        add_bob(&server, &env, org, "member").await;
        let _ = new_conversation(&server, &env.bob, org).await;
        let bob_id = send_json(
            &server,
            "GET",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        let bob_id = bob_id
            .as_array()
            .or_else(|| bob_id["results"].as_array())
            .expect("members")
            .iter()
            .find(|m| m["email"] == "bob@example.com")
            .and_then(|m| m["user_id"].as_i64().or_else(|| m["id"].as_i64()))
            .expect("bob id");
        let left = send_json(
            &server,
            "DELETE",
            &format!("/api/v1/orgs/{org}/members/{bob_id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(left.status_code(), 204, "{}", left.text());
        use sea_orm::{EntityTrait, PaginatorTrait};
        let remaining = pnex_backend::models::_entities::ai_conversations::Entity::find()
            .count(&ctx.db)
            .await
            .expect("count");
        assert_eq!(remaining, 0);

        // Purge: a conversation inactive for longer than the retention goes.
        let _ = new_conversation(&server, &env.alice, org).await;
        assert_eq!(
            pnex_backend::services::ai::conversations::purge_inactive(&ctx.db, 30)
                .await
                .expect("purge"),
            0
        );
        use sea_orm::{ActiveModelTrait, Set};
        let row = pnex_backend::models::_entities::ai_conversations::Entity::find()
            .one(&ctx.db)
            .await
            .expect("read")
            .expect("row");
        let mut old: pnex_backend::models::_entities::ai_conversations::ActiveModel = row.into();
        old.last_message_at = Set((chrono::Utc::now() - chrono::Duration::days(31)).fixed_offset());
        old.update(&ctx.db).await.expect("age");
        assert_eq!(
            pnex_backend::services::ai::conversations::purge_inactive(&ctx.db, 30)
                .await
                .expect("purge"),
            1
        );
    })
    .await;
}

// ─────────────────────── Dashboards (D144) ───────────────────────

/// A dashboard widget whose control feeds a DEPLOYED flow can only be
/// moved by the assistant: renaming it is refused with ai-flow-running
/// naming the flow; once the flow is stopped the rename goes through.
#[tokio::test]
#[serial]
async fn coupled_dashboard_widget_is_frozen_while_its_flow_runs() {
    with_app_ai(true, true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let layout = serde_json::json!({
            "canvas": {"width": 1600, "height": 900},
            "widgets": [{"id": "w1", "type": "switch", "title": "Pump",
                         "x": 40, "y": 40, "w": 240, "h": 120, "source": [], "options": {}}],
            "wires": [],
        });
        let created = send_json(
            &server,
            "POST",
            "/api/v1/dashboards",
            &env.alice,
            org,
            Some(serde_json::json!({"name": "Garden", "layout": layout})),
        )
        .await
        .json::<serde_json::Value>();
        let dashboard_id = created["id"].as_str().expect("id").to_string();
        let control = created["layout"]["widgets"][0]["options"]["control"]["control_id"]
            .as_str()
            .expect("bound control")
            .to_string();

        // A flow listens to that control, deployed (status + version set
        // directly: deploying needs the runtime).
        let flow = send_json(
            &server,
            "POST",
            "/api/v1/flows",
            &env.alice,
            org,
            Some(serde_json::json!({"name": "pump loop", "graph": {"nodes": [
                {"id": "c", "kind": "control_source", "config": {"controls": [control]},
                 "outputs": [{"port": 0, "targets": ["d"]}]},
                {"id": "d", "kind": "debug", "config": {}}
            ]}})),
        )
        .await
        .json::<serde_json::Value>();
        let flow_id = flow["id"].as_i64().expect("flow id");
        {
            use pnex_backend::models::_entities::{flow_versions, flows};
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            let version = flow_versions::Entity::find()
                .filter(flow_versions::Column::FlowId.eq(flow_id))
                .one(&ctx.db)
                .await
                .expect("read version")
                .expect("version");
            let row = flows::Entity::find_by_id(flow_id)
                .one(&ctx.db)
                .await
                .expect("read flow")
                .expect("flow");
            let mut active: flows::ActiveModel = row.into();
            active.status = Set("deployed".into());
            active.deployed_version_id = Set(Some(version.id));
            active.update(&ctx.db).await.expect("deploy");
        }

        let current = send_json(&server, "GET", &format!("/api/v1/dashboards/{dashboard_id}"), &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        let mut renamed = current["layout"].clone();
        renamed["widgets"][0]["title"] = serde_json::json!("Valve");
        let update = |layout: serde_json::Value, version: i64| {
            reply_tool_call(
                "t1",
                "update_dashboard",
                serde_json::json!({"dashboard_id": dashboard_id, "expected_version": version, "layout": layout}),
            )
        };

        reset_mock();
        push_reply(MockReply::Ok(update(renamed.clone(), 1)));
        push_reply(MockReply::Ok(reply_text("stop it first")));
        let body = chat_send(&server, &env.alice, org, "rename the pump").await.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], false, "{body}");
        assert_eq!(body["tool_trace"][0]["code"], "ai-flow-running", "{body}");
        assert_eq!(body["tool_trace"][0]["args"]["flow"], "pump loop", "{body}");

        // Moving it is allowed while the flow runs.
        let mut moved = current["layout"].clone();
        moved["widgets"][0]["x"] = serde_json::json!(400);
        reset_mock();
        push_reply(MockReply::Ok(update(moved, 1)));
        push_reply(MockReply::Ok(reply_text("moved")));
        let body = chat_send(&server, &env.alice, org, "move it").await.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], true, "{body}");

        // The flow stopped by the user → the rename goes through (v3).
        set_flow_status(&ctx, flow_id, "stopped").await;
        reset_mock();
        push_reply(MockReply::Ok(update(renamed, 2)));
        push_reply(MockReply::Ok(reply_text("renamed")));
        let body = chat_send(&server, &env.alice, org, "rename now").await.json::<serde_json::Value>();
        assert_eq!(body["tool_trace"][0]["ok"], true, "{body}");
        let after = send_json(&server, "GET", &format!("/api/v1/dashboards/{dashboard_id}"), &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        assert_eq!(after["current_version_number"], 3, "{after}");
        assert_eq!(after["layout"]["widgets"][0]["title"], "Valve");
    })
    .await;
}

/// Conversation retention: an owner/admin may only shorten the platform
/// value, a viewer may not change it, and the purge applies each org's
/// effective value.
#[tokio::test]
#[serial]
async fn org_retention_only_shortens_and_drives_the_purge() {
    with_app_ai(true, true, |server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let get = send_json(
            &server,
            "GET",
            "/api/v1/ai/retention",
            &env.alice,
            org,
            None,
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(get["days"], 180, "{get}");
        assert_eq!(get["editable"], true);

        let above = send_json(
            &server,
            "PUT",
            "/api/v1/ai/retention",
            &env.alice,
            org,
            Some(serde_json::json!({"days": 400})),
        )
        .await;
        assert_eq!(above.status_code(), 422, "{}", above.text());
        assert!(above.text().contains("ai-retention-above-platform"));
        let set = send_json(
            &server,
            "PUT",
            "/api/v1/ai/retention",
            &env.alice,
            org,
            Some(serde_json::json!({"days": 30})),
        )
        .await
        .json::<serde_json::Value>();
        assert_eq!(set["days"], 30, "{set}");
        assert_eq!(set["org_days"], 30);

        // Only a platform admin sets the platform value.
        let platform = send_json(
            &server,
            "PUT",
            "/api/v1/ai/retention/default",
            &env.alice,
            org,
            Some(serde_json::json!({"days": 90})),
        )
        .await;
        assert_eq!(platform.status_code(), 403, "{}", platform.text());

        // A viewer of alice's org cannot change it.
        let add = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"email": "bob@example.com", "role": "viewer"}))
            .await;
        assert_eq!(add.status_code(), 201, "{}", add.text());
        let viewer = send_json(
            &server,
            "PUT",
            "/api/v1/ai/retention",
            &env.bob,
            org,
            Some(serde_json::json!({"days": 10})),
        )
        .await;
        assert_eq!(viewer.status_code(), 403, "{}", viewer.text());

        // 31-day-old conversations: erased in alice's org (30 d), kept in
        // bob's personal org (platform 180 d).
        let a = new_conversation(&server, &env.alice, org).await;
        let b = new_conversation(&server, &env.bob, bob_org).await;
        {
            use pnex_backend::models::_entities::ai_conversations;
            use sea_orm::{ActiveModelTrait, EntityTrait, Set};
            for id in [&a, &b] {
                let row =
                    ai_conversations::Entity::find_by_id(uuid::Uuid::parse_str(id).expect("uuid"))
                        .one(&ctx.db)
                        .await
                        .expect("read")
                        .expect("row");
                let mut old: ai_conversations::ActiveModel = row.into();
                old.last_message_at =
                    Set((chrono::Utc::now() - chrono::Duration::days(31)).fixed_offset());
                old.update(&ctx.db).await.expect("age");
            }
        }
        let erased = pnex_backend::services::ai::conversations::purge_pass(&ctx.db)
            .await
            .expect("purge");
        assert_eq!(erased, 1);
        let kept = send_json(
            &server,
            "GET",
            &format!("/api/v1/ai/conversations/{b}"),
            &env.bob,
            bob_org,
            None,
        )
        .await;
        assert_eq!(kept.status_code(), 200);
    })
    .await;
}

// ─────────────── Templates, functions, read-only surfaces ───────────────

/// Runs one assistant turn made of the given tool calls (one per model
/// step), then a final text; returns the trace.
async fn run_tools(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    calls: Vec<(&str, serde_json::Value)>,
) -> Vec<serde_json::Value> {
    reset_mock();
    for (i, (name, args)) in calls.into_iter().enumerate() {
        push_reply(MockReply::Ok(reply_tool_call(&format!("t{i}"), name, args)));
    }
    push_reply(MockReply::Ok(reply_text("done")));
    let body = chat_send(server, token, org, "go")
        .await
        .json::<serde_json::Value>();
    body["tool_trace"].as_array().cloned().unwrap_or_default()
}

/// Templates: created, previewed without any delivery, rewritten only on
/// top of the state read (stale updated_at refused); a viewer cannot write.
#[tokio::test]
#[serial]
async fn assistant_writes_templates_on_top_of_what_it_read() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let trace = run_tools(&server, &env.alice, org, vec![(
            "create_notification_template",
            serde_json::json!({"name": "High temp", "subject": "Alert", "body": "Temperature {{ temp }} °C"}),
        )])
        .await;
        assert_eq!(trace[0]["ok"], true, "{trace:?}");
        let list = send_json(&server, "GET", "/api/v1/notify/templates", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        let tpl = &list["results"][0];
        let id = tpl["id"].as_str().expect("id").to_string();
        assert_eq!(tpl["vars"][0]["name"], "temp", "variables detected: {tpl}");

        let trace = run_tools(&server, &env.alice, org, vec![
            ("preview_notification_template", serde_json::json!({"template_id": id, "vars": {"temp": "42"}})),
            ("update_notification_template", serde_json::json!({
                "template_id": id, "expected_updated_at": "2000-01-01T00:00:00+00:00",
                "name": "High temp", "body": "changed"})),
            ("update_notification_template", serde_json::json!({
                "template_id": id, "expected_updated_at": tpl["updated_at"],
                "name": "High temp", "body": "Hot: {{ temp }}"})),
        ])
        .await;
        assert_eq!(trace[0]["ok"], true, "{trace:?}");
        assert_eq!(trace[1]["ok"], false, "stale state refused: {trace:?}");
        assert_eq!(trace[2]["ok"], true, "{trace:?}");
        let rendered = mock_requests()[1].to_string();
        assert!(rendered.contains("Temperature 42"), "preview rendered: {rendered}");
        let deliveries = send_json(&server, "GET", "/api/v1/notify/deliveries", &env.alice, org, None).await;
        assert!(!deliveries.text().contains("\"status\""), "nothing sent: {}", deliveries.text());

        // A viewer cannot write a template.
        let _ = personal_org(&server, &env.bob).await;
        let add = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({"email": "bob@example.com", "role": "viewer"}))
            .await;
        assert_eq!(add.status_code(), 201);
        let trace = run_tools(&server, &env.bob, org, vec![(
            "create_notification_template",
            serde_json::json!({"name": "x", "body": "y"}),
        )])
        .await;
        assert_eq!(trace[0]["ok"], false, "{trace:?}");
    })
    .await;
}

/// Functions: created, read back with its code, a new version only on top
/// of the version read — the conflict check now lives in the shared
/// service, so the UI route gets it too.
#[tokio::test]
#[serial]
async fn assistant_versions_functions_on_top_of_what_it_read() {
    with_app_ai(true, true, |server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let code = "# @input x number\n# @output y number\ndef main(x):\n    return {\"y\": x * 2}\n";
        let trace = run_tools(&server, &env.alice, org, vec![(
            "create_function",
            serde_json::json!({"name": "double", "language": "starlark", "code": code}),
        )])
        .await;
        assert_eq!(trace[0]["ok"], true, "{trace:?}");
        let list = send_json(&server, "GET", "/api/v1/functions", &env.alice, org, None)
            .await
            .json::<serde_json::Value>();
        let id = list["results"][0]["id"].as_i64().expect("function id");

        let trace = run_tools(&server, &env.alice, org, vec![
            ("get_function", serde_json::json!({"function_id": id})),
            ("update_function", serde_json::json!({"function_id": id, "expected_version": 1, "code": code.replace('2', "3")})),
            ("update_function", serde_json::json!({"function_id": id, "expected_version": 1, "code": code})),
        ])
        .await;
        assert_eq!(trace[0]["ok"], true, "{trace:?}");
        assert!(mock_requests()[1].to_string().contains("x * 2"), "code read back");
        assert_eq!(trace[1]["ok"], true, "{trace:?}");
        assert_eq!(trace[2]["ok"], false, "stale version refused: {trace:?}");
        let versions = send_json(&server, "GET", &format!("/api/v1/functions/{id}/versions"), &env.alice, org, None).await;
        assert!(versions.text().contains("\"version_number\":2"), "{}", versions.text());
        assert!(!versions.text().contains("\"version_number\":3"), "{}", versions.text());

        // Read-only surfaces answer (empty org).
        let trace = run_tools(&server, &env.alice, org, vec![
            ("list_pois", serde_json::json!({})),
            ("list_tours", serde_json::json!({})),
            ("list_annotation_sets", serde_json::json!({})),
            ("list_controls", serde_json::json!({})),
            ("read_memory", serde_json::json!({})),
        ])
        .await;
        assert!(trace.iter().all(|t| t["ok"] == true), "{trace:?}");
    })
    .await;
}
