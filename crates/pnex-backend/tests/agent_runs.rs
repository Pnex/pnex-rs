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
            204,
            "a job stopped a moment ago still takes its logs (D759)"
        );
        // Past the grace of ten minutes, nothing more.
        db.execute_unprepared(&format!(
            "UPDATE runner_jobs SET finished_at = now() - interval '11 minutes' WHERE id = '{}'",
            job["id"].as_str().unwrap()
        ))
        .await
        .unwrap();
        let res = server
            .post(&logs)
            .add_header("Authorization", format!("Bearer {runner}"))
            .json(&one_container)
            .await;
        assert_eq!(res.status_code().as_u16(), 409, "too late for logs");
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

/// Runs a request with a raw body, as the transfer containers and an upload send it.
async fn raw(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    auth: &str,
    org: Option<i64>,
    headers: &[(&str, &str)],
    body: &[u8],
) -> axum_test::TestResponse {
    let mut req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {auth}"));
    if let Some(org) = org {
        req = req.add_header("X-Org-Id", org.to_string());
    }
    for (k, v) in headers {
        req = req.add_header(k.to_string(), v.to_string());
    }
    if method == "GET" {
        req.await
    } else {
        req.bytes(body.to_vec().into()).await
    }
}

/// Needs the test S3 too (`docker compose up -d rustfs rustfs-init`).
#[tokio::test]
#[serial]
async fn artifacts_go_in_checked_and_out_once_through_pnex_only() {
    with_db(|server, env, db| async move {
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;

        // A person gives a file to later runs.
        let notes = b"the flaky test is in parser.rs".to_vec();
        let res = raw(
            &server,
            "POST",
            "/api/v1/agent-artifacts?name=notes",
            &env.alice,
            Some(org),
            &[],
            &notes,
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
        let input = res.json::<Value>();
        assert_eq!(input["state"], "ready");
        assert_eq!(input["size"], notes.len());
        assert!(input["sha256"].as_str().unwrap().starts_with("sha256:"));
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-artifacts?name=Bad_Name",
            None,
        )
        .await;
        assert_eq!(s, 400);

        let (s, run) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-runs",
            Some(json!({
                "environment_id": environment, "timeout_secs": 60,
                "inputs": [{ "name": "notes", "artifact_id": input["id"] }],
                "outputs": [{ "name": "report", "required": true }, { "name": "extra" }]
            })),
        )
        .await;
        assert_eq!(s, 201, "{run}");
        let id = run["id"].as_str().unwrap().to_string();
        let outputs = format!("/api/v1/agent-runs/{id}/artifacts");
        let (_, list) = call(&server, &env.alice, org, "GET", &outputs, None).await;
        assert_eq!(list["count"], 2, "{list}");
        assert!(list["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["state"] == "pending"));

        // The claim gives the transfer containers their own token, apart from the agent's.
        let runner = enrol(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let job = res.json::<Value>();
        let job_id = job["id"].as_str().unwrap().to_string();
        let artifacts = &job["sandbox"]["artifacts"];
        let transfer = artifacts["token"].as_str().unwrap().to_string();
        assert!(transfer.starts_with("pnart_"));
        assert_eq!(
            artifacts["url"],
            "http://127.0.0.1:5150/api/v1/agent-transfer"
        );
        assert_eq!(artifacts["inputs"][0]["sha256"], input["sha256"]);
        assert_eq!(artifacts["outputs"][1]["name"], "extra");
        let run_token = job["sandbox"]["secrets"]["PNEX_RUN_TOKEN"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!job["sandbox"]["secrets"].to_string().contains(&transfer));

        // Inputs: declared ones only, with the artifact token only.
        let res = raw(
            &server,
            "GET",
            "/api/v1/agent-transfer/inputs/notes",
            &transfer,
            None,
            &[],
            b"",
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        assert_eq!(res.as_bytes().to_vec(), notes);
        for (token, name, status) in [
            (run_token.as_str(), "notes", 401),
            (transfer.as_str(), "other", 404),
        ] {
            let res = raw(
                &server,
                "GET",
                &format!("/api/v1/agent-transfer/inputs/{name}"),
                token,
                None,
                &[],
                b"",
            )
            .await;
            assert_eq!(res.status_code().as_u16(), status, "{name}");
        }

        // Outputs: once each, declared names only.
        let send = |name: &'static str, body: &'static [u8]| {
            let server = &server;
            let transfer = transfer.clone();
            async move {
                raw(
                    server,
                    "PUT",
                    &format!("/api/v1/agent-transfer/outputs/{name}"),
                    &transfer,
                    None,
                    &[("x-pnex-artifact-form", "file")],
                    body,
                )
                .await
            }
        };
        let res = send("report", b"all green").await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let report = res.json::<Value>();
        assert_eq!(report["state"], "ready");
        assert_eq!(send("report", b"again").await.status_code().as_u16(), 409);
        assert_eq!(send("undeclared", b"x").await.status_code().as_u16(), 404);
        // Over the limit of an artifact (64 KiB in the test config): refused whole, and
        // the run says so.
        static BIG: [u8; 70_000] = [7; 70_000];
        let res = send("extra", &BIG).await;
        assert_eq!(res.status_code().as_u16(), 413, "{}", res.text());
        let (_, list) = call(&server, &env.alice, org, "GET", &outputs, None).await;
        let extra = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == "extra")
            .unwrap()
            .clone();
        assert_eq!(extra["state"], "refused", "{extra}");
        assert_eq!(extra["error"], "agent-artifact-too-large");
        assert_eq!(send("extra", b"small").await.status_code().as_u16(), 409);
        let res = raw(
            &server,
            "POST",
            "/api/v1/agent-artifacts?name=big",
            &env.alice,
            Some(org),
            &[],
            &BIG,
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 413, "{}", res.text());

        // Downloaded as an attachment, never shown.
        let res = raw(
            &server,
            "GET",
            &format!("/api/v1/agent-artifacts/{}/content", report["id"].as_str().unwrap()),
            &env.alice,
            Some(org),
            &[],
            b"",
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        assert_eq!(res.as_bytes().to_vec(), b"all green".to_vec());
        assert_eq!(
            res.header("content-disposition"),
            "attachment; filename=\"report\""
        );
        assert_eq!(res.header("x-content-type-options"), "nosniff");

        // The end of the job: what was not sent is missing, the token is dead.
        let finish = |job: String| {
            let server = &server;
            let runner = runner.clone();
            async move {
                server
                    .post(&format!("/api/v1/runner/jobs/{job}/finish"))
                    .add_header("Authorization", format!("Bearer {runner}"))
                    .json(&json!({ "outcome": "succeeded", "exit_code": 0 }))
                    .await
            }
        };
        assert_eq!(finish(job_id.clone()).await.status_code().as_u16(), 204);
        let (_, list) = call(&server, &env.alice, org, "GET", &outputs, None).await;
        let state = |name: &str| {
            list["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["name"] == name)
                .unwrap()["state"]
                .clone()
        };
        assert_eq!(state("report"), "ready");
        assert_eq!(state("extra"), "refused", "a refused output stays refused");
        assert_eq!(run_of(&server, &env.alice, org, &id).await["outcome"], "succeeded");
        assert_eq!(send("extra", b"late").await.status_code().as_u16(), 401);

        // The ended run is a fact of the ontology: the file it left points back to it, and
        // it points to the file it read (D708, D761).
        let count = |sql: String| {
            let db = &db;
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    sql,
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "n")
                .unwrap()
            }
        };
        let produced = format!(
            "SELECT count(*) AS n FROM resource_edges e JOIN objects f ON f.id = e.source_object_id \
             JOIN objects r ON r.id = e.target_object_id \
             WHERE e.org_id = {org} AND e.relation = 'file_produced_by' AND e.valid_to IS NULL \
               AND r.properties->>'run_id' = '{id}' AND f.properties->>'file_name' = 'report'"
        );
        let read = format!(
            "SELECT count(*) AS n FROM resource_edges e JOIN objects r ON r.id = e.source_object_id \
             JOIN objects f ON f.id = e.target_object_id \
             WHERE e.org_id = {org} AND e.relation = 'run_read_file' AND e.valid_to IS NULL \
               AND r.properties->>'run_id' = '{id}' AND f.properties->>'artifact_id' = '{}'",
            input["id"].as_str().unwrap()
        );
        let mut seen = (0, 0);
        for _ in 0..50 {
            seen = (count(produced.clone()).await, count(read.clone()).await);
            if seen == (1, 1) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(seen, (1, 1), "produced and read links of the run");

        // A required output missing fails a run the agent ended well.
        let (s, run2) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-runs",
            Some(json!({ "environment_id": environment, "timeout_secs": 60,
                         "outputs": [{ "name": "patch", "required": true }] })),
        )
        .await;
        assert_eq!(s, 201, "{run2}");
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        let job2 = res.json::<Value>()["id"].as_str().unwrap().to_string();
        assert_eq!(finish(job2).await.status_code().as_u16(), 204);
        let after = run_of(&server, &env.alice, org, run2["id"].as_str().unwrap()).await;
        assert_eq!(after["outcome"], "failed", "{after}");
        assert_eq!(after["error"], "agent-run-output-missing");

        // Another org neither reads nor uses the artifacts of this one.
        let other = personal_org(&server, &env.bob).await;
        let one = format!("/api/v1/agent-artifacts/{}", input["id"].as_str().unwrap());
        assert_eq!(call(&server, &env.bob, other, "GET", &one, None).await.0, 404);
        let theirs = runnable(&server, &db, &env.bob, other).await;
        let (s, b) = call(
            &server,
            &env.bob,
            other,
            "POST",
            "/api/v1/agent-runs",
            Some(json!({ "environment_id": theirs, "timeout_secs": 60,
                         "inputs": [{ "name": "notes", "artifact_id": input["id"] }] })),
        )
        .await;
        assert_eq!(s, 404, "{b}");
        assert!(b.to_string().contains("agent-artifact-not-found"));

        // A viewer of the org sees the outputs, not their content.
        let res = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        assert_eq!(call(&server, &env.bob, org, "GET", &outputs, None).await.0, 200);
        let content = format!("{one}/content");
        assert_eq!(call(&server, &env.bob, org, "GET", &content, None).await.0, 403);

        // Past its expiry an artifact goes, its object first.
        // The store of config/test.yaml.
        let store = pnex_backend::services::artifact_store::S3Config {
            endpoint: "http://localhost:9000".into(),
            bucket: "pnex-test".into(),
            access_key: "rustfsadmin".into(),
            secret_key: "rustfsadmin".into(),
            path_style: true,
            ..Default::default()
        }
        .store()
        .unwrap();
        let input_id = input["id"].as_str().unwrap();
        let key = format!("org_{org}/agent-artifacts/uploads/{input_id}");
        assert!(store.size(&key).await.unwrap().is_some());
        db.execute_unprepared(&format!(
            "UPDATE agent_artifacts SET expires_at = now() - interval '1 hour' WHERE id = '{input_id}'"
        ))
        .await
        .unwrap();
        let gone = pnex_backend::services::agent_artifacts::sweep(&db, &store)
            .await
            .unwrap();
        assert!(gone >= 1);
        assert!(store.size(&key).await.unwrap().is_none());
        assert_eq!(call(&server, &env.alice, org, "GET", &one, None).await.0, 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn agents_are_service_accounts_whose_runs_only_tighten_them() {
    with_db(|server, env, db| async move {
        let (provider, seen) = fake_provider().await;
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        // The org's agent usage, and a provider of the agent's own (D710).
        for (name, model, usages) in [
            ("org", "org-model", json!(["agent"])),
            ("own", "agent-own-model", json!([])),
        ] {
            let (s, p) = call(
                &server,
                &env.alice,
                org,
                "POST",
                "/api/v1/ai/providers",
                Some(json!({ "name": name, "kind": "anthropic", "base_url": provider,
                             "model": model, "api_key": { "value": PROVIDER_KEY }, "usages": usages })),
            )
            .await;
            assert_eq!(s, 201, "{p}");
        }
        let (_, providers) = call(&server, &env.alice, org, "GET", "/api/v1/ai/providers", None).await;
        let own = providers["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "own")
            .unwrap()["id"]
            .clone();

        let spec = json!({
            "environment_id": environment, "timeout_secs": 600, "llm_calls_max": 20,
            "inputs": ["ticket"], "outputs": [{ "name": "patch", "required": true }],
            "provider_id": own
        });
        let (s, agent) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agents",
            Some(json!({ "name": "triage", "spec": spec })),
        )
        .await;
        assert_eq!(s, 201, "{agent}");
        assert_eq!(agent["role"], "viewer", "born viewer (D761)");
        assert_eq!(agent["version"], 1);
        let id = agent["id"].as_str().unwrap().to_string();
        // Its identity is a service account of the org.
        let (_, accounts) =
            call(&server, &env.alice, org, "GET", "/api/v1/iam/service-accounts", None).await;
        assert!(accounts.to_string().contains(agent["service_account_id"].as_str().unwrap()));
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agents",
            Some(json!({ "name": "Triage", "spec": spec })),
        )
        .await;
        assert_eq!(s, 409, "names are unique, whatever the case");

        // A new version, on the one that was read.
        let mut tighter = spec.clone();
        tighter["timeout_secs"] = json!(300);
        let one = format!("/api/v1/agents/{id}");
        let (s, v2) = call(
            &server,
            &env.alice,
            org,
            "PUT",
            &one,
            Some(json!({ "expected_version": 1, "spec": tighter })),
        )
        .await;
        assert_eq!(s, 200, "{v2}");
        assert_eq!(v2["version"], 2);
        let (s, b) = call(
            &server,
            &env.alice,
            org,
            "PUT",
            &one,
            Some(json!({ "expected_version": 1, "spec": tighter })),
        )
        .await;
        assert_eq!(s, 409, "{b}");
        assert!(b.to_string().contains("agent-version-conflict"));
        let (_, versions) = call(&server, &env.alice, org, "GET", &format!("{one}/versions"), None).await;
        assert_eq!(versions["count"], 2);

        // A run starts from the record and may only tighten it.
        let res = raw(
            &server,
            "POST",
            "/api/v1/agent-artifacts?name=ticket",
            &env.alice,
            Some(org),
            &[],
            b"the parser drops a field",
        )
        .await;
        let ticket = res.json::<Value>()["id"].clone();
        let runs = format!("{one}/runs");
        for (body, token) in [
            (json!({ "inputs": [{ "name": "ticket", "artifact_id": ticket }], "timeout_secs": 301 }), "above_agent"),
            (json!({ "inputs": [{ "name": "ticket", "artifact_id": ticket }], "llm_calls_max": 21 }), "above_agent"),
            (json!({ "inputs": [] }), "missing"),
            (json!({ "inputs": [{ "name": "ticket", "artifact_id": ticket }, { "name": "x", "artifact_id": ticket }] }), "undeclared"),
        ] {
            let (s, b) = call(&server, &env.alice, org, "POST", &runs, Some(body)).await;
            assert_eq!(s, 400, "{b}");
            assert!(b.to_string().contains(token), "{b}");
        }
        let (s, run) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &runs,
            Some(json!({ "inputs": [{ "name": "ticket", "artifact_id": ticket }], "llm_calls_max": 3 })),
        )
        .await;
        assert_eq!(s, 201, "{run}");
        assert_eq!(run["agent_id"], id);
        assert_eq!(run["agent_version"], 2);
        assert_eq!(run["llm_calls_max"], 3);

        // Its calls go to the agent's own provider, not the org's usage.
        let runner = enrol(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        let job = res.json::<Value>();
        assert_eq!(job["spec"]["timeout_secs"], 300, "the record's version 2");
        let token = job["sandbox"]["secrets"]["PNEX_RUN_TOKEN"]
            .as_str()
            .unwrap()
            .to_string();
        let (s, b) = gateway(&server, "messages", &token, json!({ "model": "x", "messages": [] })).await;
        assert_eq!(s, 200, "{b}");
        assert_eq!(seen.lock().unwrap().last().unwrap().1["model"], "agent-own-model");

        // A viewer reads agents, and neither creates nor deletes one.
        let other = personal_org(&server, &env.bob).await;
        let res = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        assert_eq!(call(&server, &env.bob, org, "GET", &one, None).await.0, 200);
        let (s, _) = call(
            &server,
            &env.bob,
            org,
            "POST",
            "/api/v1/agents",
            Some(json!({ "name": "mine", "spec": spec })),
        )
        .await;
        assert_eq!(s, 403);
        assert_eq!(call(&server, &env.bob, org, "DELETE", &one, None).await.0, 403);
        // Another org does not see it.
        assert_eq!(call(&server, &env.bob, other, "GET", &one, None).await.0, 404);

        // The tier caps agents apart from the members.
        db.execute_unprepared(&format!(
            "INSERT INTO subscription_tiers (name, max_sensor_devices, max_actuator_devices, \
               max_mixed_devices, min_build_interval_secs, created_at, updated_at, max_agents) \
             VALUES ('agents-cap-test', 10, 10, 10, 0, now(), now(), 1); \
             UPDATE organizations SET subscription_tier_id = \
               (SELECT id FROM subscription_tiers WHERE name = 'agents-cap-test') WHERE id = {org}"
        ))
        .await
        .unwrap();
        let (s, b) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agents",
            Some(json!({ "name": "second", "spec": spec })),
        )
        .await;
        assert_eq!(s, 409, "{b}");
        assert!(b.to_string().contains("agent-quota"));

        // Deleting it removes its identity; its run keeps its trace.
        assert_eq!(call(&server, &env.alice, org, "DELETE", &one, None).await.0, 204);
        assert_eq!(call(&server, &env.alice, org, "GET", &one, None).await.0, 404);
        let (_, accounts) =
            call(&server, &env.alice, org, "GET", "/api/v1/iam/service-accounts", None).await;
        assert!(!accounts.to_string().contains(agent["service_account_id"].as_str().unwrap()));
        let after = run_of(&server, &env.alice, org, run["id"].as_str().unwrap()).await;
        assert_eq!(after["id"], run["id"]);
        assert!(after["agent_id"].is_null());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn repos_and_services_reach_only_their_own_containers() {
    with_db(|server, env, db| async move {
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        let (s, secret) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/secrets",
            Some(json!({ "name": "forge_read", "value": "ghp_read-token-of-the-forge-value" })),
        )
        .await;
        assert_eq!(s, 201, "{secret}");
        let digest = "0".repeat(64);
        let body = json!({
            "environment_id": environment, "timeout_secs": 60,
            "repos": [{ "url": "https://localhost/acme/app.git", "git_ref": "main", "path": "app",
                        "secret_id": secret["id"] }],
            "services": [{ "name": "pg", "image": format!("cgr.dev/chainguard/postgres@sha256:{digest}"),
                           "env": { "POSTGRES_PASSWORD": "@generated", "POSTGRES_DB": "app" },
                           "ready_command": ["pg_isready"] }]
        });
        let (s, run) = call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(body)).await;
        assert_eq!(s, 201, "{run}");
        // The run keeps the name of the secret, never its value.
        let row = db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT repos::text AS r FROM agent_runs WHERE id = '{}'", run["id"].as_str().unwrap()),
            ))
            .await
            .unwrap()
            .unwrap();
        let stored: String = row.try_get("", "r").unwrap();
        assert!(!stored.contains("read-token-of-the-forge"));

        let runner = enrol(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let sandbox = res.json::<Value>()["sandbox"].clone();
        assert_eq!(sandbox["repos"][0]["token"], "ghp_read-token-of-the-forge-value");
        assert_eq!(sandbox["repos"][0]["path"], "app");
        let generated = sandbox["services"][0]["env"]["POSTGRES_PASSWORD"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(generated.len(), 48, "a fresh password, not the placeholder");
        assert_eq!(sandbox["services"][0]["env"]["POSTGRES_DB"], "app");
        assert_eq!(sandbox["secrets"]["PG_POSTGRES_PASSWORD"], generated, "the agent connects with it");
        assert!(!sandbox["secrets"].to_string().contains("read-token-of-the-forge"), "never to the agent");
        assert!(
            sandbox["egress"].as_array().unwrap().iter().any(|e| e["port"] == 443),
            "the forge is reachable: {}",
            sandbox["egress"]
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_scan_over_the_threshold_signals_by_default_and_refuses_on_demand() {
    with_db(|server, env, db| async move {
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        let (_, policy) =
            call(&server, &env.alice, org, "GET", "/api/v1/environments/scan-policy", None).await;
        assert_eq!(policy, json!({ "threshold": "critical", "mode": "signal" }), "D748 default");
        // A scan of the active image with one fixable critical flaw.
        let report = json!({ "components": [], "findings": [
            { "id": "CVE-2026-1", "package": "openssl", "installed": "3.0", "fixed": "3.1", "severity": "critical" }
        ] });
        db.execute_unprepared(&format!(
            "WITH j AS (INSERT INTO runner_jobs (org_id, kind, spec, required_capabilities, state, queue_expires_at) \
               VALUES ({org}, 'environment_scan', '{{\"image\":\"x\",\"command\":[],\"timeout_secs\":60}}', '[]', 'succeeded', now()) \
               RETURNING id) \
             INSERT INTO environment_scans (id, org_id, build_id, job_id, report, breaches) \
             SELECT gen_random_uuid(), {org}, e.active_build_id, j.id, '{report}', 1 \
             FROM environments e, j WHERE e.id = '{environment}'"
        ))
        .await
        .unwrap();
        let body = json!({ "environment_id": environment, "timeout_secs": 60 });
        let (s, b) = call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(body.clone())).await;
        assert_eq!(s, 201, "signal only: the run starts {b}");

        let (s, p) = call(
            &server,
            &env.alice,
            org,
            "PUT",
            "/api/v1/environments/scan-policy",
            Some(json!({ "threshold": "critical", "mode": "refuse" })),
        )
        .await;
        assert_eq!(s, 200, "{p}");
        let (s, b) = call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(body.clone())).await;
        assert_eq!(s, 409, "{b}");
        assert!(b.to_string().contains("environment-scan-refused"), "{b}");
        let (_, e) = call(&server, &env.alice, org, "GET", &format!("/api/v1/environments/{environment}"), None).await;
        let (s, b) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &format!("/api/v1/environments/{environment}/builds/{}/activate", e["active_build_id"].as_str().unwrap()),
            Some(json!({ "expected_version": e["version"] })),
        )
        .await;
        assert_eq!(s, 409, "{b}");
        assert!(b.to_string().contains("environment-scan-refused"), "{b}");

        // A policy change applies at once.
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "PUT",
            "/api/v1/environments/scan-policy",
            Some(json!({ "threshold": "none", "mode": "refuse" })),
        )
        .await;
        assert_eq!(s, 200);
        let (s, b) = call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(body)).await;
        assert_eq!(s, 201, "{b}");

        // Only an admin sets it.
        let res = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "member" }))
            .await;
        let _ = personal_org(&server, &env.bob).await;
        let res = if res.status_code().is_success() {
            res
        } else {
            server
                .post(&format!("/api/v1/orgs/{org}/members"))
                .add_header("Authorization", format!("Bearer {}", env.alice))
                .add_header("X-Org-Id", org.to_string())
                .json(&json!({ "email": "bob@example.com", "role": "member" }))
                .await
        };
        assert!(res.status_code().is_success(), "{}", res.text());
        let (s, _) = call(
            &server,
            &env.bob,
            org,
            "PUT",
            "/api/v1/environments/scan-policy",
            Some(json!({ "threshold": "critical", "mode": "signal" })),
        )
        .await;
        assert_eq!(s, 403);
    })
    .await;
}

/// Needs the test S3 too (`docker compose up -d rustfs rustfs-init`).
#[tokio::test]
#[serial]
async fn a_dependency_cache_goes_by_key_and_falls_back_to_the_latest() {
    with_db(|server, env, db| async move {
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        let body = json!({
            "environment_id": environment, "timeout_secs": 60,
            "caches": [{ "name": "cargo", "path": "/tmp/.cargo", "key_files": ["app/Cargo.lock"] }]
        });
        let (s, run) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-runs",
            Some(body),
        )
        .await;
        assert_eq!(s, 201, "{run}");
        let runner = enrol(&server, &env.alice, org).await;
        let job = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await
            .json::<Value>();
        let artifacts = &job["sandbox"]["artifacts"];
        assert_eq!(artifacts["caches"][0]["name"], "cargo", "{job}");
        let token = artifacts["token"].as_str().unwrap().to_string();
        let key1 = "a".repeat(32);
        let key2 = "b".repeat(32);
        let path =
            |name: &str, key: &str| format!("/api/v1/agent-transfer/caches/{name}?key={key}");
        let get = |name: &str, key: &str| {
            let (server, token, p) = (&server, token.clone(), path(name, key));
            async move { raw(server, "GET", &p, &token, None, &[], b"").await }
        };
        assert_eq!(
            get("cargo", &key1).await.status_code().as_u16(),
            404,
            "nothing saved yet"
        );
        let res = raw(
            &server,
            "PUT",
            &path("cargo", &key1),
            &token,
            None,
            &[],
            b"tar of deps",
        )
        .await;
        assert_eq!(res.status_code().as_u16(), 204, "{}", res.text());
        let exact = get("cargo", &key1).await;
        assert_eq!(exact.status_code().as_u16(), 200);
        assert_eq!(exact.header("x-pnex-cache-exact"), "true");
        assert_eq!(exact.as_bytes().to_vec(), b"tar of deps".to_vec());
        let older = get("cargo", &key2).await;
        assert_eq!(older.status_code().as_u16(), 200, "the latest of the cache");
        assert_eq!(older.header("x-pnex-cache-exact"), "false");
        assert_eq!(
            get("npm", &key1).await.status_code().as_u16(),
            404,
            "undeclared"
        );
        assert_eq!(get("cargo", "../x").await.status_code().as_u16(), 400);
        let (s, b) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-runs",
            Some(json!({ "environment_id": environment, "timeout_secs": 60,
                         "caches": [{ "name": "x", "path": "/workspace/target" }] })),
        )
        .await;
        assert_eq!(s, 400, "{b}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_environment_sends_its_runs_to_micro_vms_unless_they_need_services() {
    with_db(|server, env, db| async move {
        let org = personal_org(&server, &env.alice).await;
        let environment = runnable(&server, &db, &env.alice, org).await;
        let one = format!("/api/v1/environments/{environment}");
        let isolation = format!("{one}/isolation");
        let (_, e) = call(&server, &env.alice, org, "GET", &one, None).await;
        assert_eq!(e["isolation"], "pod", "a pod by default");
        let version = e["version"].as_i64().unwrap();

        // Another org does not see it; a member of this one may not weaken or harden it.
        let bob_org = personal_org(&server, &env.bob).await;
        let body = json!({ "isolation": "vm", "expected_version": version });
        let (s, d) = call(&server, &env.bob, bob_org, "PUT", &isolation, Some(body.clone())).await;
        assert_eq!(s, 404, "{d}");
        let res = server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "member" }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let (s, d) = call(&server, &env.bob, org, "PUT", &isolation, Some(body.clone())).await;
        assert_eq!(s, 403, "an Admin decides: {d}");

        let (s, e) = call(&server, &env.alice, org, "PUT", &isolation, Some(body.clone())).await;
        assert_eq!(s, 200, "{e}");
        assert_eq!(e["isolation"], "vm");
        let (s, d) = call(&server, &env.alice, org, "PUT", &isolation, Some(body)).await;
        assert_eq!(s, 409, "on the version read only: {d}");
        assert_eq!(d["error"], "environment-version-conflict");

        let required = |run: Value| {
            let db = db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!(
                        "SELECT j.required_capabilities::text AS c FROM agent_runs r \
                         JOIN runner_jobs j ON j.id = r.job_id WHERE r.id = '{}'",
                        run["id"].as_str().unwrap()
                    ),
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<String>("", "c")
                .unwrap()
            }
        };
        let plain = json!({ "environment_id": environment, "timeout_secs": 60 });
        let (s, run) = call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(plain)).await;
        assert_eq!(s, 201, "{run}");
        assert_eq!(required(run).await, r#"["sandbox:vm"]"#);
        // A pod runner never takes it: a VM run waits for a VM.
        let runner = enrol(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/jobs/claim")
            .add_header("Authorization", format!("Bearer {runner}"))
            .await;
        assert_eq!(res.status_code().as_u16(), 204, "{}", res.text());

        let digest = "0".repeat(64);
        let with_services = json!({
            "environment_id": environment, "timeout_secs": 60,
            "services": [{ "name": "kv", "image": format!("cgr.dev/chainguard/valkey@sha256:{digest}"),
                           "ready_command": ["valkey-cli", "ping"] }]
        });
        let (s, run) =
            call(&server, &env.alice, org, "POST", "/api/v1/agent-runs", Some(with_services)).await;
        assert_eq!(s, 201, "{run}");
        assert_eq!(required(run).await, r#"["sandbox:pod"]"#, "services run in a pod only");
    })
    .await;
}
