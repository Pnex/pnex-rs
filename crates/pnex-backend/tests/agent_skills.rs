//! Skills of hosted agents (agent-environments.md D711, autonomous-agents.md D396): drafts,
//! activation by a person after the diff, secret refusal, pinning on agents.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

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

#[tokio::test]
#[serial]
async fn every_text_is_a_draft_until_a_person_activates_it() {
    with_db(|server, env, _db| async move {
        let org = personal_org(&server, &env.alice).await;
        let (s, skill) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-skills",
            Some(json!({ "name": "code-review", "description": "How we review",
                         "body": "Read the diff first.\n\nThen run the tests." })),
        )
        .await;
        assert_eq!(s, 201, "{skill}");
        assert_eq!(skill["latest_number"], 1);
        assert_eq!(skill["active_number"], Value::Null, "born a draft");
        let id = skill["id"].as_str().unwrap().to_string();
        let one = format!("/api/v1/agent-skills/{id}");

        // Names are file names, unique in the org.
        for (name, status) in [("code-review", 409), ("Code Review", 400), ("a/b", 400)] {
            let (s, _) = call(
                &server,
                &env.alice,
                org,
                "POST",
                "/api/v1/agent-skills",
                Some(json!({ "name": name, "body": "x" })),
            )
            .await;
            assert_eq!(s, status, "{name}");
        }

        // A text shaped like a credential is refused, never stored.
        let token = format!("ghp_{}", "a".repeat(36));
        let (s, e) = call(
            &server,
            &env.alice,
            org,
            "PATCH",
            &one,
            Some(json!({ "expected_version": 1, "body": format!("use {token}") })),
        )
        .await;
        assert_eq!(s, 422, "{e}");
        assert_eq!(e["error"], "agent-skill-secret");

        // A new text is the next draft, on the version that was read.
        let (s, v2) = call(
            &server,
            &env.alice,
            org,
            "PATCH",
            &one,
            Some(json!({ "expected_version": 1, "body": "Read the diff first.\n\nThen run all the tests." })),
        )
        .await;
        assert_eq!(s, 200, "{v2}");
        assert_eq!(v2["latest_number"], 2);
        assert_eq!(v2["version"], 2);
        let (s, e) = call(
            &server,
            &env.alice,
            org,
            "PATCH",
            &one,
            Some(json!({ "expected_version": 1, "body": "stale" })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "agent-skill-version-conflict");

        let (_, list) = call(&server, &env.alice, org, "GET", &format!("{one}/versions"), None).await;
        assert_eq!(list["count"], 2, "{list}");
        assert_eq!(list["results"][0]["number"], 2, "newest first");
        assert_eq!(list["results"][0]["activated"], false);
        let (s, d) = call(&server, &env.alice, org, "GET", &format!("{one}/diff?from=1&to=2"), None).await;
        assert_eq!(s, 200, "{d}");
        assert_eq!(d["changed"], 1, "{d}");
        assert_eq!(d["to_version"], 2);
        let (s, e) = call(&server, &env.alice, org, "GET", &format!("{one}/versions/9"), None).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "agent-skill-version-not-found");

        // Activation: on the version read, by a person.
        let (s, _) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &format!("{one}/activate"),
            Some(json!({ "number": 2, "expected_version": 1 })),
        )
        .await;
        assert_eq!(s, 409);
        let (s, active) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &format!("{one}/activate"),
            Some(json!({ "number": 2, "expected_version": 2 })),
        )
        .await;
        assert_eq!(s, 200, "{active}");
        assert_eq!(active["active_number"], 2);
        let (_, v) = call(&server, &env.alice, org, "GET", &format!("{one}/versions/2"), None).await;
        assert_eq!(v["activated"], true);
        assert!(v["sha256"].as_str().unwrap().starts_with("sha256:"));

        // Another org sees nothing of it.
        let other = personal_org(&server, &env.bob).await;
        let (s, _) = call(&server, &env.bob, other, "GET", &one, None).await;
        assert_eq!(s, 404);
        let (_, theirs) = call(&server, &env.bob, other, "GET", "/api/v1/agent-skills", None).await;
        assert_eq!(theirs["count"], 0);

        let (s, _) = call(&server, &env.alice, org, "DELETE", &one, None).await;
        assert_eq!(s, 204);
        let (s, e) = call(&server, &env.alice, org, "GET", &one, None).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "agent-skill-not-found");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn only_agents_write_drafts_or_activates_them() {
    with_db(|server, env, _db| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, skill) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/agent-skills",
            Some(json!({ "name": "triage", "body": "Sort by impact." })),
        )
        .await;
        let id = skill["id"].as_str().unwrap().to_string();
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "member",
        )
        .await;
        // A member reads, never writes nor activates (agents:write is Admin).
        let (s, _) = call(
            &server,
            &env.bob,
            org,
            "GET",
            &format!("/api/v1/agent-skills/{id}"),
            None,
        )
        .await;
        assert_eq!(s, 200);
        for (method, path, body) in [
            (
                "POST",
                "/api/v1/agent-skills".to_string(),
                json!({ "name": "x", "body": "y" }),
            ),
            (
                "PATCH",
                format!("/api/v1/agent-skills/{id}"),
                json!({ "expected_version": 1, "body": "z" }),
            ),
            (
                "POST",
                format!("/api/v1/agent-skills/{id}/activate"),
                json!({ "number": 1, "expected_version": 1 }),
            ),
        ] {
            let (s, e) = call(&server, &env.bob, org, method, &path, Some(body)).await;
            assert_eq!(s, 403, "{method} {path}");
            assert_eq!(e["error"], "access-denied");
        }
        let (s, _) = call(
            &server,
            &env.bob,
            org,
            "DELETE",
            &format!("/api/v1/agent-skills/{id}"),
            None,
        )
        .await;
        assert_eq!(s, 403);
    })
    .await;
}
