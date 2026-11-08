//! Internal agents (agent-unification.md D781, D785): the assistant's loop on an
//! `internal_llm` run, against a scripted fake model. Every write becomes a proposal; a
//! person's acceptance runs it with that person's rights.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and the test S3 (`docker compose up -d rustfs
//! rustfs-init`): the final answer is the `report` output.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::internal_agents;
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use serial_test::serial;
use uuid::Uuid;

type Script = Arc<Mutex<VecDeque<Value>>>;

/// Answers like the Messages API, one scripted reply per call; a plain text once empty.
async fn fake_model() -> (String, Script) {
    let script: Script = Arc::default();
    let next = script.clone();
    let app = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(move |_body: axum::Json<Value>| {
            let next = next.clone();
            async move {
                let reply = next
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| text("done"));
                axum::Json(reply)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), script)
}

fn text(t: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "stop_reason": "end_turn",
            "usage": { "input_tokens": 5, "output_tokens": 3 } })
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({ "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }],
            "stop_reason": "tool_use", "usage": { "input_tokens": 7, "output_tokens": 2 } })
}

struct Env {
    alice: String,
}

async fn with_db<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe { std::env::set_var("PNEX_RATE_LIMIT", "off") };
    unsafe { std::env::set_var("PNEX_AI_ENABLED", "true") };
    // The fake model listens on a loopback port.
    unsafe { std::env::set_var("PNEX_EGRESS", "open") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let env = Env {
        alice: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
    };
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env, ctx.clone()).await;
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

async fn agent(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
    role: &str,
    spec: Value,
) -> (u16, Value) {
    call(
        server,
        token,
        org,
        "POST",
        "/api/v1/agents",
        Some(json!({ "name": name, "role": role, "spec": spec })),
    )
    .await
}

/// Starts a run of `agent`, then plays its job here, as the backend worker would.
async fn run(
    server: &axum_test::TestServer,
    ctx: &loco_rs::app::AppContext,
    token: &str,
    org: i64,
    agent: &str,
) -> Value {
    let (s, run) = call(
        server,
        token,
        org,
        "POST",
        &format!("/api/v1/agents/{agent}/runs"),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, 201, "{run}");
    assert_eq!(run["engine"], "internal_llm");
    assert!(run["environment_id"].is_null());
    let job = Uuid::parse_str(run["job_id"].as_str().unwrap()).unwrap();
    let claimed = internal_agents::claim(&ctx.db, Some(job))
        .await
        .unwrap()
        .expect("queued internal job");
    internal_agents::run_job(ctx, claimed).await;
    let id = run["id"].as_str().unwrap();
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
async fn an_internal_agent_reads_proposes_its_writes_and_reports() {
    with_db(|server, env, ctx| async move {
        let (model, script) = fake_model().await;
        let org = personal_org(&server, &env.alice).await;
        let (s, p) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/ai/providers",
            Some(
                json!({ "name": "agents", "kind": "anthropic", "base_url": model,
                         "model": "m", "api_key": { "value": "sk-test-key-value" },
                         "usages": ["agent"] }),
            ),
        )
        .await;
        assert_eq!(s, 201, "{p}");

        // The registry, with the write flag.
        let (s, tools) = call(
            &server,
            &env.alice,
            org,
            "GET",
            "/api/v1/agents/tools?limit=100",
            None,
        )
        .await;
        assert_eq!(s, 200, "{tools}");
        let listed = tools["results"].as_array().unwrap();
        let flag = |n: &str| {
            listed
                .iter()
                .find(|t| t["name"] == n)
                .map(|t| t["write"].clone())
        };
        assert_eq!(flag("list_flows"), Some(json!(false)));
        assert_eq!(flag("create_flow"), Some(json!(true)));

        let spec = json!({ "level": "internal", "timeout_secs": 120, "llm_calls_max": 10,
                           "tools": ["list_flows", "create_flow"], "max_steps": 5 });
        // A viewer agent gets no write tool; unknown tools and sandbox fields are refused.
        let (s, b) = agent(&server, &env.alice, org, "viewer", "viewer", spec.clone()).await;
        assert_eq!(s, 400, "{b}");
        assert!(b.to_string().contains("write_needs_member"), "{b}");
        let mut unknown = spec.clone();
        unknown["tools"] = json!(["drop_database"]);
        let (s, b) = agent(&server, &env.alice, org, "u", "member", unknown).await;
        assert_eq!(s, 400);
        assert!(b.to_string().contains("unknown_tool"), "{b}");
        let mut sandboxed = spec.clone();
        sandboxed["command"] = json!(["sh"]);
        let (s, b) = agent(&server, &env.alice, org, "s", "member", sandboxed).await;
        assert_eq!(s, 400);
        assert!(b.to_string().contains("hosted_only"), "{b}");

        let (s, a) = agent(&server, &env.alice, org, "helper", "member", spec.clone()).await;
        assert_eq!(s, 201, "{a}");
        let id = a["id"].as_str().unwrap().to_string();

        // A read, then a write the agent may only propose, then its report.
        let flows_before: i64 = ctx
            .db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT count(*) AS n FROM flows WHERE org_id = {org}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap();
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "list_flows", json!({})));
            s.push_back(tool(
                "t2",
                "create_flow",
                json!({ "name": "empty", "graph": { "nodes": [] } }),
            ));
            s.push_back(tool(
                "t3",
                "create_flow",
                json!({ "name": "from agent", "graph": { "nodes": [
                    { "id": "n1", "kind": "inject", "config": { "repeat_secs": 30 },
                      "outputs": [{ "port": 0, "targets": ["n2"] }] },
                    { "id": "n2", "kind": "metric", "config": { "metric_name": "x" } }
                ] } }),
            ));
            s.push_back(text("I proposed two flows."));
        }
        let done = run(&server, &ctx, &env.alice, org, &id).await;
        assert_eq!(done["state"], "succeeded", "{done}");
        assert_eq!(done["llm_calls"], 4);
        let count = |db: &sea_orm::DatabaseConnection| {
            let db = db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT count(*) AS n FROM flows WHERE org_id = {org}"),
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "n")
                .unwrap()
            }
        };
        assert_eq!(
            count(&ctx.db).await,
            flows_before,
            "the agent wrote nothing"
        );
        let run_id = done["id"].as_str().unwrap();
        let (_, outputs) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &format!("/api/v1/agent-runs/{run_id}/artifacts"),
            None,
        )
        .await;
        assert!(
            outputs.to_string().contains("\"report\"") && outputs.to_string().contains("ready"),
            "{outputs}"
        );

        let (_, inbox) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &format!("/api/v1/proposals?origin=agent:{id}"),
            None,
        )
        .await;
        let found = inbox["results"].as_array().unwrap();
        assert_eq!(found.len(), 2, "{inbox}");
        assert!(found
            .iter()
            .all(|p| p["kind"] == "tool_call" && p["values"]["run_id"] == run_id));
        let named = |n: &str| {
            let p = found
                .iter()
                .find(|p| p["values"]["arguments"]["name"] == n)
                .unwrap();
            format!("/api/v1/proposals/{}/decision", p["id"].as_str().unwrap())
        };
        // A tool that refuses with the person's rights leaves the proposal open.
        let refused = named("empty");
        let (s, b) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &refused,
            Some(json!({ "decision": "accept" })),
        )
        .await;
        assert_eq!(s, 409, "{b}");
        assert_eq!(b["error"], "proposal-tool-failed");
        let (_, still) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &refused.replace("/decision", ""),
            None,
        )
        .await;
        assert_eq!(still["status"], "proposed");
        let decide = named("from agent");
        // The arguments run as proposed: no correction.
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &decide,
            Some(json!({ "decision": "accept", "values": { "name": "x" } })),
        )
        .await;
        assert_eq!(s, 400);
        // The person accepts: the write runs with their rights.
        let (s, accepted) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &decide,
            Some(json!({ "decision": "accept" })),
        )
        .await;
        assert_eq!(s, 200, "{accepted}");
        assert_eq!(accepted["status"], "accepted");
        assert_eq!(count(&ctx.db).await, flows_before + 1);
        // Accepting again changes nothing.
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &decide,
            Some(json!({ "decision": "accept" })),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(count(&ctx.db).await, flows_before + 1);

        // A tool outside its list is refused, never run; the same call three times ends the run.
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("u1", "list_devices", json!({})));
            for n in 0..3 {
                s.push_back(tool(&format!("l{n}"), "list_flows", json!({ "a": 1 })));
            }
        }
        let looped = run(&server, &ctx, &env.alice, org, &id).await;
        assert_eq!(looped["state"], "failed", "{looped}");
        assert_eq!(looped["error"], "agent-loop-detected");

        // A stop between steps ends the run as stopped.
        let (s, started) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &format!("/api/v1/agents/{id}/runs"),
            Some(json!({})),
        )
        .await;
        assert_eq!(s, 201);
        let job = Uuid::parse_str(started["job_id"].as_str().unwrap()).unwrap();
        let claimed = internal_agents::claim(&ctx.db, Some(job))
            .await
            .unwrap()
            .unwrap();
        let sid = started["id"].as_str().unwrap();
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &format!("/api/v1/agent-runs/{sid}/stop"),
            None,
        )
        .await;
        assert_eq!(s, 200);
        internal_agents::run_job(&ctx, claimed).await;
        let (_, stopped) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &format!("/api/v1/agent-runs/{sid}"),
            None,
        )
        .await;
        assert_eq!(stopped["outcome"], "stopped", "{stopped}");
        assert_eq!(stopped["llm_calls"], 0, "no model call after the stop");
    })
    .await;
}

/// Plays the validation run of request `p` (validation-gate.md D836).
async fn play_validator(
    server: &axum_test::TestServer,
    ctx: &loco_rs::app::AppContext,
    token: &str,
    org: i64,
    p: &Value,
) {
    let run = p["values"]["validator"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("handed to the validator: {p}"));
    let (_, r) = call(
        server,
        token,
        org,
        "GET",
        &format!("/api/v1/agent-runs/{run}"),
        None,
    )
    .await;
    let job = Uuid::parse_str(r["job_id"].as_str().unwrap()).unwrap();
    let claimed = internal_agents::claim(&ctx.db, Some(job))
        .await
        .unwrap()
        .expect("queued validation job");
    internal_agents::run_job(ctx, claimed).await;
}

/// The newest request of agent `id`, whatever its status.
async fn last_request(
    server: &axum_test::TestServer,
    ctx: &loco_rs::app::AppContext,
    token: &str,
    org: i64,
    id: &str,
) -> Value {
    let pid: Uuid = ctx
        .db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "SELECT id FROM proposals WHERE org_id = {org} AND origin = 'agent:{id}' \
                 ORDER BY created_at DESC LIMIT 1"
            ),
        ))
        .await
        .unwrap()
        .expect("a request")
        .try_get("", "id")
        .unwrap();
    call(
        server,
        token,
        org,
        "GET",
        &format!("/api/v1/proposals/{pid}"),
        None,
    )
    .await
    .1
}

#[tokio::test]
#[serial]
async fn the_gate_runs_validates_or_hands_writes_to_a_person() {
    with_db(|server, env, ctx| async move {
        let (model, script) = fake_model().await;
        let org = personal_org(&server, &env.alice).await;
        let a = env.alice.as_str();
        let (s, p) = call(&server, a, org, "POST", "/api/v1/ai/providers", Some(json!({
            "name": "agents", "kind": "anthropic", "base_url": model, "model": "m",
            "api_key": { "value": "sk-test-key-value" }, "usages": ["agent"] }))).await;
        assert_eq!(s, 201, "{p}");
        let flows = |name: &'static str| {
            let db = ctx.db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(
                    sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT count(*) AS n FROM flows WHERE org_id = {org} AND name = '{name}'"),
                ))
                .await
                .unwrap()
                .unwrap()
                .try_get::<i64>("", "n")
                .unwrap()
            }
        };
        let graph = json!({ "nodes": [
            { "id": "n1", "kind": "inject", "config": { "repeat_secs": 30 },
              "outputs": [{ "port": 0, "targets": ["n2"] }] },
            { "id": "n2", "kind": "metric", "config": { "metric_name": "x" } } ] });

        let checker = json!({ "level": "internal", "timeout_secs": 120, "llm_calls_max": 10,
                              "tools": ["list_flows"], "max_steps": 3 });
        let (s, v) = agent(&server, a, org, "checker", "viewer", checker).await;
        assert_eq!(s, 201, "{v}");
        let vid = v["id"].as_str().unwrap().to_string();

        // The record: a mode on a write tool of the agent, a validator when one is needed.
        let base = json!({ "level": "internal", "timeout_secs": 120, "llm_calls_max": 10,
                           "tools": ["list_flows", "create_flow"], "max_steps": 5 });
        let mut read_gate = base.clone();
        read_gate["gate"] = json!({ "list_flows": "autonomous" });
        let (s, b) = agent(&server, a, org, "x1", "member", read_gate).await;
        assert!(s == 400 && b.to_string().contains("not_write_tool"), "{b}");
        let mut no_validator = base.clone();
        no_validator["gate"] = json!({ "create_flow": "chain" });
        let (s, b) = agent(&server, a, org, "x2", "member", no_validator).await;
        assert!(s == 400 && b.to_string().contains("validator_id"), "{b}");

        // Autonomous: runs at once with the agent's service account, recorded as decided.
        let mut auto = base.clone();
        auto["gate"] = json!({ "create_flow": "autonomous" });
        let (s, w) = agent(&server, a, org, "writer", "member", auto).await;
        assert_eq!(s, 201, "{w}");
        let wid = w["id"].as_str().unwrap().to_string();
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "create_flow", json!({ "name": "auto", "graph": graph })));
            s.push_back(text("done"));
        }
        let done = run(&server, &ctx, a, org, &wid).await;
        assert_eq!(done["state"], "succeeded", "{done}");
        assert_eq!(flows("auto").await, 1, "written without a person");
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!((p["status"].as_str(), p["reason"].as_str()), (Some("accepted"), Some("autonomous")), "{p}");
        assert_eq!(p["values"]["gate"], "autonomous");

        // A validator that accepts: the write runs with the asking agent's account.
        let put = |gate: &'static str, version: i64| {
            let mut spec = base.clone();
            spec["gate"] = json!({ "create_flow": gate });
            spec["validator_id"] = json!(vid);
            json!({ "spec": spec, "expected_version": version })
        };
        let (s, w) = call(&server, a, org, "PUT", &format!("/api/v1/agents/{wid}"), Some(put("validator", 1))).await;
        assert_eq!(s, 200, "{w}");
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "create_flow", json!({ "name": "checked", "graph": graph })));
            s.push_back(text("done"));
        }
        run(&server, &ctx, a, org, &wid).await;
        assert_eq!(flows("checked").await, 0, "waits for the validator");
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!(p["values"]["gate_stage"], "validator", "{p}");
        script.lock().unwrap().push_back(text("ACCEPT\nA small metric flow."));
        play_validator(&server, &ctx, a, org, &p).await;
        assert_eq!(flows("checked").await, 1, "accepted by the validator");
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!(p["status"], "accepted", "{p}");
        assert_eq!(p["values"]["validator"]["decision"], "accept");

        // A chain: the validator refuses → rejected; accepts → a person still decides.
        let (s, w) = call(&server, a, org, "PUT", &format!("/api/v1/agents/{wid}"), Some(put("chain", 2))).await;
        assert_eq!(s, 200, "{w}");
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "create_flow", json!({ "name": "refused", "graph": graph })));
            s.push_back(text("done"));
        }
        run(&server, &ctx, a, org, &wid).await;
        let p = last_request(&server, &ctx, a, org, &wid).await;
        script.lock().unwrap().push_back(text("REJECT: the name is not allowed"));
        play_validator(&server, &ctx, a, org, &p).await;
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!(p["status"], "rejected", "{p}");
        assert!(p["reason"].as_str().unwrap().contains("not allowed"), "{p}");
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "create_flow", json!({ "name": "chained", "graph": graph })));
            s.push_back(text("done"));
        }
        run(&server, &ctx, a, org, &wid).await;
        let p = last_request(&server, &ctx, a, org, &wid).await;
        script.lock().unwrap().push_back(text("ACCEPT"));
        play_validator(&server, &ctx, a, org, &p).await;
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!((p["status"].as_str(), p["values"]["gate_stage"].as_str()), (Some("proposed"), Some("human")), "{p}");
        assert_eq!(flows("chained").await, 0, "a person still decides");

        // A tripped validator: requests go to a person until it is re-armed.
        ctx.db
            .execute_unprepared(&format!(
                "UPDATE agents SET validator_tripped_at = now(), validator_tripped_reason = 'rate' WHERE id = '{vid}'"
            ))
            .await
            .unwrap();
        {
            let mut s = script.lock().unwrap();
            s.push_back(tool("t1", "create_flow", json!({ "name": "tripped", "graph": graph })));
            s.push_back(text("done"));
        }
        run(&server, &ctx, a, org, &wid).await;
        let p = last_request(&server, &ctx, a, org, &wid).await;
        assert_eq!(p["values"]["gate_stage"], "human", "{p}");
        assert!(p["values"].get("validator").is_none(), "{p}");
        let (_, v) = call(&server, a, org, "GET", &format!("/api/v1/agents/{vid}"), None).await;
        assert_eq!(v["validator_tripped"], "rate");
        let (s, v) = call(&server, a, org, "POST", &format!("/api/v1/agents/{vid}/validator/rearm"), None).await;
        assert_eq!(s, 200, "{v}");
        assert!(v.get("validator_tripped").is_none(), "{v}");
    })
    .await;
}
