//! Runners (agent-environments.md lot E0): enrolment, the job queue, leases and the
//! reaper, through the real routes. The `pnex-runner` crate tests the runner's side.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and the compose `valkey` service.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::runners;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};
use serial_test::serial;

const IMAGE: &str = "x@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    // These tests enrol many runners; tests/rate_limit.rs covers the limit itself.
    unsafe { std::env::set_var("PNEX_RATE_LIMIT", "off") };
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
            f(server, env, ctx).await;
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

/// A user's call in an org.
async fn user(
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

/// A runner's call, with its own token and no org header.
async fn machine(
    server: &axum_test::TestServer,
    token: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let req = server
        .post(path)
        .add_header("Authorization", format!("Bearer {token}"));
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    let text = res.text();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn join_token(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let (s, d) = user(
        server,
        token,
        org,
        "POST",
        "/api/v1/runners/join-tokens",
        Some(json!({ "name": "rack-1" })),
    )
    .await;
    assert_eq!(s, 200, "{d}");
    d["token"].as_str().unwrap().to_string()
}

/// `(runner id, runner token)`.
async fn enrol(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    capabilities: &[&str],
) -> (String, String) {
    let join = join_token(server, token, org).await;
    let res = server
        .post("/api/v1/runner/register")
        .json(&json!({ "join_token": join, "capabilities": capabilities }))
        .await;
    assert_eq!(res.status_code().as_u16(), 200);
    let d: Value = res.json();
    (
        d["runner_id"].as_str().unwrap().to_string(),
        d["token"].as_str().unwrap().to_string(),
    )
}

async fn queue(server: &axum_test::TestServer, token: &str, org: i64, required: &[&str]) -> String {
    let (s, d) = user(
        server,
        token,
        org,
        "POST",
        "/api/v1/runner-jobs",
        Some(json!({
            "kind": "probe",
            "spec": { "image": IMAGE, "command": ["true"], "timeout_secs": 60 },
            "required_capabilities": required,
        })),
    )
    .await;
    assert_eq!(s, 200, "{d}");
    assert_eq!(d["state"], "queued");
    d["id"].as_str().unwrap().to_string()
}

async fn claim(server: &axum_test::TestServer, runner: &str) -> Option<String> {
    let (s, d) = machine(server, runner, "/api/v1/runner/jobs/claim", None).await;
    match s {
        200 => Some(d["id"].as_str().unwrap().to_string()),
        204 => None,
        other => panic!("claim answered {other}: {d}"),
    }
}

async fn job_of(server: &axum_test::TestServer, token: &str, org: i64, id: &str) -> Value {
    let (s, d) = user(
        server,
        token,
        org,
        "GET",
        &format!("/api/v1/runner-jobs/{id}"),
        None,
    )
    .await;
    assert_eq!(s, 200, "{d}");
    d
}

async fn sql(ctx: &loco_rs::app::AppContext, text: String) {
    ctx.db
        .execute_raw(Statement::from_string(DatabaseBackend::Postgres, text))
        .await
        .unwrap();
}

#[tokio::test]
#[serial]
async fn a_join_token_enrols_one_runner_once() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let join = join_token(&server, &env.alice, org).await;
        assert!(join.starts_with("pnjoin_"));
        let register = |token: String| {
            let req = server
                .post("/api/v1/runner/register")
                .json(&json!({ "join_token": token, "capabilities": ["arch:amd64"] }));
            async move { req.await }
        };
        let first = register(join.clone()).await;
        assert_eq!(first.status_code().as_u16(), 200);
        let d: Value = first.json();
        assert!(d["token"].as_str().unwrap().starts_with("pnrun_"));

        let second = register(join).await;
        assert_eq!(second.status_code().as_u16(), 400, "single use");
        assert!(second.text().contains("runner-join-token-invalid"));
        let unknown = register("pnjoin_nope".to_string()).await;
        assert_eq!(unknown.status_code().as_u16(), 400);

        // Neither the token nor its hash ever comes back in a read (R4, R16).
        let (s, list) = user(&server, &env.alice, org, "GET", "/api/v1/runners", None).await;
        assert_eq!(s, 200);
        assert_eq!(list["count"], 1);
        let runner = &list["results"][0];
        assert_eq!(runner["name"], "rack-1");
        assert_eq!(runner["capabilities"], json!(["arch:amd64"]));
        assert_eq!(runner["online"], true);
        assert!(runner.get("token").is_none() && runner.get("token_hash").is_none());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_expired_join_token_and_invalid_capabilities_are_refused() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bad = join_token(&server, &env.alice, org).await;
        let res = server
            .post("/api/v1/runner/register")
            .json(&json!({ "join_token": bad, "capabilities": ["Not Valid"] }))
            .await;
        assert_eq!(res.status_code().as_u16(), 400);
        // A refused registration does not burn the token.
        let res = server
            .post("/api/v1/runner/register")
            .json(&json!({ "join_token": bad, "capabilities": [] }))
            .await;
        assert_eq!(res.status_code().as_u16(), 200);

        let late = join_token(&server, &env.alice, org).await;
        sql(
            &ctx,
            format!(
                "UPDATE runner_join_tokens SET expires_at = now() - interval '1 second' \
                 WHERE org_id = {org} AND used_at IS NULL"
            ),
        )
        .await;
        let res = server
            .post("/api/v1/runner/register")
            .json(&json!({ "join_token": late, "capabilities": [] }))
            .await;
        assert_eq!(res.status_code().as_u16(), 400);
    })
    .await;
}

/// R2: every org route, without the permission. Bob is a viewer of Alice's org.
#[tokio::test]
#[serial]
async fn managing_needs_the_permission_and_reading_does_not() {
    with_app(|server, env, _ctx| async move {
        let _ = personal_org(&server, &env.bob).await;
        let org = personal_org(&server, &env.alice).await;
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
        .await;
        let (runner, _) = enrol(&server, &env.alice, org, &[]).await;
        let job = queue(&server, &env.alice, org, &[]).await;

        for (method, path, body) in [
            (
                "POST",
                "/api/v1/runners/join-tokens".to_string(),
                Some(json!({ "name": "x" })),
            ),
            (
                "POST",
                "/api/v1/runner-jobs".to_string(),
                Some(json!({
                    "kind": "probe",
                    "spec": { "image": IMAGE, "timeout_secs": 60 }
                })),
            ),
            ("DELETE", format!("/api/v1/runners/{runner}"), None),
        ] {
            let (s, d) = user(&server, &env.bob, org, method, &path, body).await;
            assert_eq!(s, 403, "{method} {path}: {d}");
        }
        for path in [
            "/api/v1/runners".to_string(),
            "/api/v1/runner-jobs".to_string(),
            format!("/api/v1/runner-jobs/{job}"),
        ] {
            let (s, d) = user(&server, &env.bob, org, "GET", &path, None).await;
            assert_eq!(s, 200, "{path}: {d}");
        }
    })
    .await;
}

/// R1: the org is the runner's, and another org's ids answer 404 or 409.
#[tokio::test]
#[serial]
async fn the_org_is_a_wall() {
    with_app(|server, env, _ctx| async move {
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        assert_ne!(a, b);
        let (_, runner_b) = enrol(&server, &env.bob, b, &[]).await;
        let (runner_a_id, runner_a) = enrol(&server, &env.alice, a, &[]).await;
        let job = queue(&server, &env.alice, a, &[]).await;

        assert_eq!(claim(&server, &runner_b).await, None, "not its org's job");
        assert_eq!(claim(&server, &runner_a).await.as_deref(), Some(&*job));
        let path = format!("/api/v1/runner/jobs/{job}/heartbeat");
        assert_eq!(machine(&server, &runner_b, &path, None).await.0, 409);
        let path = format!("/api/v1/runner/jobs/{job}/finish");
        let body = json!({ "outcome": "succeeded", "exit_code": 0 });
        assert_eq!(machine(&server, &runner_b, &path, Some(body)).await.0, 409);

        let read = format!("/api/v1/runner-jobs/{job}");
        assert_eq!(user(&server, &env.bob, b, "GET", &read, None).await.0, 404);
        let revoke = format!("/api/v1/runners/{runner_a_id}");
        assert_eq!(
            user(&server, &env.bob, b, "DELETE", &revoke, None).await.0,
            404
        );
        // The org in the header is the caller's to prove, not a way in.
        assert_eq!(user(&server, &env.bob, a, "GET", &read, None).await.0, 403);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_job_only_reaches_a_runner_with_its_capabilities() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, plain) = enrol(&server, &env.alice, org, &["isolation:none"]).await;
        let (_, hard) = enrol(&server, &env.alice, org, &["isolation:pod", "gpu"]).await;
        let job = queue(&server, &env.alice, org, &["isolation:pod"]).await;
        assert_eq!(claim(&server, &plain).await, None, "no fallback on runc");
        assert_eq!(claim(&server, &hard).await.as_deref(), Some(&*job));
        assert_eq!(claim(&server, &hard).await, None, "taken only once");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_runner_never_leases_a_server_job() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, runner) = enrol(&server, &env.alice, org, &["isolation:none"]).await;
        let job = queue(&server, &env.alice, org, &[]).await;
        // Whatever capabilities it declares, a runner never gets a job the server runs (D781).
        for kind in ["server_browser", "internal_llm", "extension"] {
            sql(
                &ctx,
                format!("UPDATE runner_jobs SET kind = '{kind}' WHERE id = '{job}'"),
            )
            .await;
            assert_eq!(claim(&server, &runner).await, None, "{kind}");
        }
        sql(
            &ctx,
            format!("UPDATE runner_jobs SET kind = 'probe' WHERE id = '{job}'"),
        )
        .await;
        assert_eq!(claim(&server, &runner).await.as_deref(), Some(&*job));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn invalid_jobs_are_refused() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        for (spec, required, field) in [
            (
                json!({ "image": "x:latest", "timeout_secs": 60 }),
                json!([]),
                "spec.image",
            ),
            (
                json!({ "image": IMAGE, "timeout_secs": 0 }),
                json!([]),
                "spec.timeout_secs",
            ),
            (
                json!({ "image": IMAGE, "timeout_secs": 3601 }),
                json!([]),
                "spec.timeout_secs",
            ),
            (
                json!({ "image": IMAGE, "timeout_secs": 60 }),
                json!(["Bad One"]),
                "required_capabilities",
            ),
        ] {
            let (s, d) = user(
                &server,
                &env.alice,
                org,
                "POST",
                "/api/v1/runner-jobs",
                Some(json!({ "kind": "probe", "spec": spec, "required_capabilities": required })),
            )
            .await;
            assert_eq!(s, 400, "{field}: {d}");
            assert!(d.to_string().contains(field), "{field}: {d}");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_job_runs_to_its_end_and_a_retry_changes_nothing() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (runner_id, runner) = enrol(&server, &env.alice, org, &[]).await;
        let job = queue(&server, &env.alice, org, &[]).await;
        assert_eq!(claim(&server, &runner).await.as_deref(), Some(&*job));
        let d = job_of(&server, &env.alice, org, &job).await;
        assert_eq!(d["state"], "running");
        assert_eq!(d["runner_id"], json!(runner_id));
        assert_eq!(d["attempts"], 1);

        let beat = format!("/api/v1/runner/jobs/{job}/heartbeat");
        assert_eq!(machine(&server, &runner, &beat, None).await.0, 204);

        let finish = format!("/api/v1/runner/jobs/{job}/finish");
        let body = json!({ "outcome": "failed", "exit_code": 3 });
        assert_eq!(
            machine(&server, &runner, &finish, Some(body.clone()))
                .await
                .0,
            204
        );
        assert_eq!(machine(&server, &runner, &finish, Some(body)).await.0, 204);
        // The result stays the first one reported.
        let late = json!({ "outcome": "succeeded", "exit_code": 0 });
        assert_eq!(machine(&server, &runner, &finish, Some(late)).await.0, 204);
        let d = job_of(&server, &env.alice, org, &job).await;
        assert_eq!(d["state"], "failed");
        assert_eq!(d["outcome"], "failed");
        assert_eq!(d["exit_code"], 3);
        assert!(d["finished_at"].is_string());

        assert_eq!(
            machine(&server, &runner, &beat, None).await.0,
            409,
            "a finished job is not held"
        );
    })
    .await;
}

/// The proof of lot E0: a dead runner's job goes to another one, and the dead one,
/// when it comes back, learns it lost it.
#[tokio::test]
#[serial]
async fn a_dead_runner_loses_its_job_to_another() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, dead) = enrol(&server, &env.alice, org, &[]).await;
        let (other_id, other) = enrol(&server, &env.alice, org, &[]).await;
        let job = queue(&server, &env.alice, org, &[]).await;
        assert_eq!(claim(&server, &dead).await.as_deref(), Some(&*job));

        // Within the lease nothing moves.
        assert_eq!(runners::reap(&ctx.db).await.unwrap().requeued, 0);
        assert_eq!(claim(&server, &other).await, None);

        sql(
            &ctx,
            format!(
                "UPDATE runner_jobs SET lease_expires_at = now() - interval '1 second' \
                 WHERE id = '{job}'"
            ),
        )
        .await;
        assert_eq!(runners::reap(&ctx.db).await.unwrap().requeued, 1);
        assert_eq!(claim(&server, &other).await.as_deref(), Some(&*job));
        let d = job_of(&server, &env.alice, org, &job).await;
        assert_eq!(d["runner_id"], json!(other_id));
        assert_eq!(d["attempts"], 2);

        let beat = format!("/api/v1/runner/jobs/{job}/heartbeat");
        let (s, d) = machine(&server, &dead, &beat, None).await;
        assert_eq!(s, 409, "{d}");
        assert!(d.to_string().contains("runner-job-not-held"));
        let finish = format!("/api/v1/runner/jobs/{job}/finish");
        let body = json!({ "outcome": "succeeded", "exit_code": 0 });
        assert_eq!(machine(&server, &dead, &finish, Some(body)).await.0, 409);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_job_gives_up_after_its_attempts_and_a_queue_expires() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, runner) = enrol(&server, &env.alice, org, &[]).await;
        let job = queue(&server, &env.alice, org, &[]).await;
        for attempt in 1..=3 {
            assert_eq!(claim(&server, &runner).await.as_deref(), Some(&*job));
            sql(
                &ctx,
                format!(
                    "UPDATE runner_jobs SET lease_expires_at = now() - interval '1 second' \
                     WHERE id = '{job}'"
                ),
            )
            .await;
            let reaped = runners::reap(&ctx.db).await.unwrap();
            assert_eq!(reaped.failed, u64::from(attempt == 3), "attempt {attempt}");
        }
        let d = job_of(&server, &env.alice, org, &job).await;
        assert_eq!(d["state"], "failed");
        assert_eq!(d["outcome"], "lease_expired");
        assert_eq!(claim(&server, &runner).await, None);

        // Nobody compatible: it expires, it is never moved elsewhere.
        let waiting = queue(&server, &env.alice, org, &["gpu"]).await;
        sql(
            &ctx,
            format!(
                "UPDATE runner_jobs SET queue_expires_at = now() - interval '1 second' \
                 WHERE id = '{waiting}'"
            ),
        )
        .await;
        assert_eq!(runners::reap(&ctx.db).await.unwrap().expired, 1);
        let d = job_of(&server, &env.alice, org, &waiting).await;
        assert_eq!(d["state"], "expired");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn revoking_stops_the_token_and_gives_back_the_job() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (id, runner) = enrol(&server, &env.alice, org, &[]).await;
        let (_, other) = enrol(&server, &env.alice, org, &[]).await;
        let job = queue(&server, &env.alice, org, &[]).await;
        assert_eq!(claim(&server, &runner).await.as_deref(), Some(&*job));

        let path = format!("/api/v1/runners/{id}");
        assert_eq!(
            user(&server, &env.alice, org, "DELETE", &path, None)
                .await
                .0,
            204
        );
        let (s, d) = machine(&server, &runner, "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 401, "{d}");
        assert!(d.to_string().contains("runner-unauthorized"));
        assert_eq!(claim(&server, &other).await.as_deref(), Some(&*job));
        assert_eq!(
            user(&server, &env.alice, org, "DELETE", &path, None)
                .await
                .0,
            404
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn no_token_no_entry() {
    with_app(|server, _env, _ctx| async move {
        let res = server.post("/api/v1/runner/jobs/claim").await;
        assert_eq!(res.status_code().as_u16(), 401);
        let (s, _) = machine(&server, "pnrun_nope", "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 401);
    })
    .await;
}
