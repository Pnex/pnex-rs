//! Runs of hosted agents and their LLM gateway (agent-environments.md D731, D732), against
//! a fake provider on a local port.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL).

mod common;

use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use serial_test::serial;

const PROVIDER_KEY: &str = "sk-provider-secret-value";
const DIGEST: &str = "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

type Seen = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

/// Answers like the Messages API: JSON, or a stream when asked.
async fn fake_provider() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let kept = seen.clone();
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |headers: HeaderMap, body: axum::Json<Value>| {
            let kept = kept.clone();
            async move {
                let stream = body["stream"].as_bool().unwrap_or(false);
                kept.lock().unwrap().push((headers, body.0));
                if stream {
                    let events = "event: message_start\n\
                        data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n\
                        event: message_delta\n\
                        data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\n";
                    ([("content-type", "text/event-stream")], events).into_response_parts()
                } else {
                    (
                        [("content-type", "application/json")],
                        json!({ "content": [{ "type": "text", "text": "hi" }],
                                "usage": { "input_tokens": 5, "output_tokens": 3 } })
                        .to_string(),
                    )
                        .into_response_parts()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

trait IntoParts {
    fn into_response_parts(self) -> axum::response::Response;
}

impl<T: axum::response::IntoResponse> IntoParts for T {
    fn into_response_parts(self) -> axum::response::Response {
        self.into_response()
    }
}

struct Env {
    alice: String,
    bob: String,
}

async fn with_db<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, sea_orm::DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe { std::env::set_var("PNEX_RATE_LIMIT", "off") };
    unsafe { std::env::set_var("PNEX_AI_ENABLED", "true") };
    // The fake provider listens on a loopback port.
    unsafe { std::env::set_var("PNEX_EGRESS", "open") };
    unsafe { std::env::set_var("PNEX_AGENT_GATEWAY_URL", "http://127.0.0.1:5150") };
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
            f(server, env, ctx.db.clone()).await;
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
        .unwrap()
}

async fn call(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
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
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// An environment of the org whose active build has an image. Returns its id.
async fn runnable(
    server: &axum_test::TestServer,
    db: &sea_orm::DatabaseConnection,
    token: &str,
    org: i64,
) -> String {
    let (s, r) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/image-registries",
        Some(json!({ "name": "r", "host": "registry.example.org", "repository": "team/app" })),
    )
    .await;
    assert_eq!(s, 201, "{r}");
    let (_, e) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/environments",
        Some(json!({ "name": "coder", "registry_id": r["id"] })),
    )
    .await;
    let id = e["id"].as_str().unwrap().to_string();
    let (s, v) = call(
        server,
        token,
        org,
        "POST",
        &format!("/api/v1/environments/{id}/versions"),
        Some(json!({ "dockerfile": "FROM x" })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    let sql = |q: String| sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, q);
    let job = db
        .query_one_raw(sql(format!(
            "INSERT INTO runner_jobs (org_id, kind, spec, required_capabilities, state, queue_expires_at) \
             VALUES ({org}, 'environment_build', '{{\"image\":\"x\",\"command\":[],\"timeout_secs\":60}}', '[]', 'succeeded', now()) \
             RETURNING id::text AS id"
        )))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "id")
        .unwrap();
    db.execute_unprepared(&format!(
        "INSERT INTO environment_builds (id, org_id, environment_id, number, job_id, repository, digest) \
         VALUES (gen_random_uuid(), {org}, '{id}', 1, '{job}', 'registry.example.org/team/app', '{DIGEST}'); \
         UPDATE environments SET active_build_id = (SELECT id FROM environment_builds WHERE job_id = '{job}') \
         WHERE id = '{id}'"
    ))
    .await
    .unwrap();
    id
}

async fn enrol(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let (s, t) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/runners/join-tokens",
        Some(json!({ "name": "r" })),
    )
    .await;
    assert_eq!(s, 200, "{t}");
    let res = server
        .post("/api/v1/runner/register")
        .json(&json!({ "join_token": t["token"], "capabilities": ["sandbox:pod"] }))
        .await;
    assert!(res.status_code().is_success(), "{}", res.text());
    res.json::<Value>()["token"].as_str().unwrap().to_string()
}

async fn gateway(
    server: &axum_test::TestServer,
    path: &str,
    token: &str,
    body: Value,
) -> (u16, String) {
    let res = server
        .post(&format!("/api/v1/agent-gateway/v1/{path}"))
        .add_header("x-api-key", token.to_string())
        .json(&body)
        .await;
    (res.status_code().as_u16(), res.text())
}

async fn run_of(server: &axum_test::TestServer, token: &str, org: i64, id: &str) -> Value {
    call(
        server,
        token,
        org,
        "GET",
        &format!("/api/v1/agent-runs/{id}"),
        None,
    )
    .await
    .1
}

#[tokio::test]
#[serial]
async fn a_run_reaches_the_model_through_the_gateway_only_while_it_runs() {
    with_db(|server, env, db| async move {
        let (provider, seen) = fake_provider().await;
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        let (s, p) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/ai/providers",
            Some(json!({
                "name": "agents", "kind": "anthropic", "base_url": provider, "model": "org-model",
                "api_key": { "value": PROVIDER_KEY }, "usages": ["agent"]
            })),
        )
        .await;
        assert_eq!(s, 201, "{p}");

        let (s, run) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-runs",
            Some(json!({ "environment_id": environment, "timeout_secs": 60,
                         "env": { "MODE": "fix" }, "llm_calls_max": 2 })),
        )
        .await;
        assert_eq!(s, 201, "{run}");
        assert_eq!(run["state"], "queued");
        assert_eq!(
            run["image"],
            format!("registry.example.org/team/app@{DIGEST}")
        );
        let id = run["id"].as_str().unwrap().to_string();

        // The runner gets the run token, the gateway address and the rule to reach it,
        // never the provider key.
        let runner = enrol(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        assert!(!res.text().contains(PROVIDER_KEY));
        let job = res.json::<Value>();
        assert_eq!(job["kind"], "sandbox");
        let sandbox = &job["sandbox"];
        let token = sandbox["secrets"]["PNEX_RUN_TOKEN"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(sandbox["secrets"]["ANTHROPIC_API_KEY"], token);
        assert_eq!(sandbox["env"]["MODE"], "fix");
        assert_eq!(
            sandbox["env"]["ANTHROPIC_BASE_URL"],
            "http://127.0.0.1:5150/api/v1/agent-gateway"
        );
        assert_eq!(
            sandbox["egress"],
            json!([{ "cidr": "127.0.0.1/32", "port": 5150 }])
        );

        // Wrong door: the provider speaks Anthropic.
        let (s, b) = gateway(
            &server,
            "chat/completions",
            &token,
            json!({ "messages": [] }),
        )
        .await;
        assert_eq!(s, 409, "{b}");
        assert!(b.contains("agent-gateway-protocol-mismatch"));

        let (s, b) = gateway(
            &server,
            "messages",
            &token,
            json!({ "model": "engine-model", "max_tokens": 10, "messages": [] }),
        )
        .await;
        assert_eq!(s, 200, "{b}");
        assert!(b.contains("\"hi\""));
        let (s, b) = gateway(
            &server,
            "messages",
            &token,
            json!({ "model": "engine-model", "stream": true, "messages": [] }),
        )
        .await;
        assert_eq!(s, 200, "{b}");
        assert!(b.contains("message_delta"), "{b}");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            for (headers, body) in seen.iter() {
                assert_eq!(body["model"], "org-model");
                assert_eq!(headers["x-api-key"], PROVIDER_KEY);
                assert_eq!(headers["anthropic-version"], "2023-06-01");
            }
        }
        let mut counted = Value::Null;
        for _ in 0..40 {
            counted = run_of(&server, &env.alice, org, &id).await;
            if counted["output_tokens"] == 10 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(counted["llm_calls"], 2, "{counted}");
        assert_eq!(counted["input_tokens"], 16, "{counted}");
        assert_eq!(counted["output_tokens"], 10, "{counted}");
        assert_eq!(counted["state"], "running");

        // The run spent its calls.
        let (s, b) = gateway(&server, "messages", &token, json!({ "messages": [] })).await;
        assert_eq!(s, 429, "{b}");
        assert!(b.contains("agent-run-llm-budget"));
        let (s, _) = gateway(&server, "messages", "pnagr_unknown", json!({})).await;
        assert_eq!(s, 401);

        // The runner sends the journal before finishing (D733), within bounds. No O2 runs
        // in these tests: reading says so, with a code.
        let logs = format!("/api/v1/runner/jobs/{}/logs", job["id"].as_str().unwrap());
        let one_container = json!({ "containers": [{ "container": "job", "text": "done\n" }] });
        let res = server
            .post(&logs)
            .add_header("Authorization", format!("Bearer {runner}"))
            .json(&one_container)
            .await;
        assert_eq!(res.status_code().as_u16(), 204, "{}", res.text());
        let too_many = json!({ "containers": vec![one_container["containers"][0].clone(); 33] });
        let res = server
            .post(&logs)
            .add_header("Authorization", format!("Bearer {runner}"))
            .json(&too_many)
            .await;
        assert_eq!(res.status_code().as_u16(), 400, "{}", res.text());
        let journal = format!("/api/v1/agent-runs/{id}/logs");
        let (s, b) = call(&server, &env.alice, org, "GET", &journal, None).await;
        assert_eq!(s, 503, "{b}");
        assert!(b.to_string().contains("agent-run-logs-unavailable"), "{b}");

        // A stop closes the token at once and the runner loses the job.
        let stop = format!("/api/v1/agent-runs/{id}/stop");
        let (s, r) = call(&server, &env.alice, org, "POST", &stop, None).await;
        assert_eq!(s, 200, "{r}");
        assert_eq!(r["outcome"], "stopped");
        let (s, b) = gateway(&server, "messages", &token, json!({ "messages": [] })).await;
        assert_eq!(s, 401, "{b}");
        let res = server
            .post(&format!(
                "/api/v1/runner/jobs/{}/heartbeat",
                job["id"].as_str().unwrap()
            ))
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        assert_eq!(res.status_code().as_u16(), 409);
        let res = server
            .post(&logs)
            .add_header("Authorization", format!("Bearer {runner}"))
            .json(&one_container)
            .await;
        assert_eq!(
            res.status_code().as_u16(),
            409,
            "a stopped job takes no more logs"
        );
        let (s, b) = call(&server, &env.alice, org, "POST", &stop, None).await;
        assert_eq!(s, 409, "{b}");
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "nothing reached the provider after"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn runs_keep_to_their_org_and_rights() {
    with_db(|server, env, db| async move {
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        let environment = runnable(&server, &db, &env.alice, a).await;
        let body = json!({ "environment_id": environment, "timeout_secs": 60 });
        let (s, run) = call(
            &server,
            &env.alice,
            a,
            "POST",
            "/api/v1/agent-runs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(s, 201, "{run}");
        let one = format!("/api/v1/agent-runs/{}", run["id"].as_str().unwrap());

        // Bob in his own org sees neither the run nor the environment.
        assert_eq!(call(&server, &env.bob, b, "GET", &one, None).await.0, 404);
        let journal = format!("{one}/logs");
        assert_eq!(
            call(&server, &env.bob, b, "GET", &journal, None).await.0,
            404
        );
        let (s, d) = call(
            &server,
            &env.bob,
            b,
            "POST",
            "/api/v1/agent-runs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(s, 404, "{d}");
        let (_, list) = call(&server, &env.bob, b, "GET", "/api/v1/agent-runs", None).await;
        assert_eq!(list["count"], 0);

        // A viewer of Alice's org reads, and neither starts nor stops.
        let res = server
            .post(&format!("/api/v1/orgs/{a}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", a.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        assert_eq!(call(&server, &env.bob, a, "GET", &one, None).await.0, 200);
        let (s, _) = call(
            &server,
            &env.bob,
            a,
            "POST",
            "/api/v1/agent-runs",
            Some(body.clone()),
        )
        .await;
        assert_eq!(s, 403);
        let (s, _) = call(&server, &env.bob, a, "POST", &format!("{one}/stop"), None).await;
        assert_eq!(s, 403);

        // Variables the gateway owns, or that hold a credential, are refused.
        for env_vars in [
            json!({ "ANTHROPIC_API_KEY": "mine" }),
            json!({ "TOKEN": "-----BEGIN RSA PRIVATE KEY-----" }),
        ] {
            let mut bad = body.clone();
            bad["env"] = env_vars;
            let (s, d) = call(
                &server,
                &env.alice,
                a,
                "POST",
                "/api/v1/agent-runs",
                Some(bad),
            )
            .await;
            assert_eq!(s, 400, "{d}");
        }
        // A queued run's token does not exist yet: nothing opens the gateway.
        let (s, _) = gateway(&server, "messages", "pnagr_", json!({})).await;
        assert_eq!(s, 401);
    })
    .await;
}
