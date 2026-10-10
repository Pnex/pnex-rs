//! Topic taxonomies API (media-ingest.md D168): CRUD, append-only versions
//! with optimistic concurrency (409), org isolation (R1), viewer read-only
//! (R2) and the `topic_classify` deploy gate (engine OFF: a deploy answers
//! 503 only AFTER the gates, so 503 = "the gates let it through").
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
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
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-media-ingest-tests-{}", std::process::id()),
        )
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
    let body = serde_json::from_str(&res.text()).unwrap_or(Value::Null);
    (status, body)
}

fn topics() -> Value {
    json!([
        { "id": "economy", "label": "Economy", "definition": "Prices, jobs",
          "keywords": ["inflation", "pouvoir d'achat"] },
        { "id": "health", "label": "Health", "keywords": ["hôpital"] }
    ])
}

#[tokio::test]
#[serial]
async fn taxonomy_crud_and_versions() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (s, t) = call(
            &server,
            "POST",
            "/api/v1/taxonomies",
            &env.alice,
            org,
            Some(json!({ "name": "News", "description": "Radio topics" })),
        )
        .await;
        assert_eq!(s, 201, "{t}");
        assert_eq!(t["current_version"], 0);
        assert!(t.get("current").is_none(), "{t}");
        let id = t["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/taxonomies/{id}");
        let versions = format!("{path}/versions");

        // Name is unique in the org.
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/taxonomies",
            &env.alice,
            org,
            Some(json!({ "name": "News" })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "taxonomy-name-taken");

        // Versions append on top of the expected one.
        let body = |expected: i32, note: &str| {
            json!({ "topics": topics(), "note": note, "expected_version": expected })
        };
        let (s, v1) = call(&server, "POST", &versions, &env.alice, org, Some(body(0, "first"))).await;
        assert_eq!(s, 201, "{v1}");
        assert_eq!(v1["version"], 1);
        let (s, v2) = call(&server, "POST", &versions, &env.alice, org, Some(body(1, "second"))).await;
        assert_eq!(s, 201, "{v2}");
        assert_eq!(v2["version"], 2);
        // A stale expected version is a coded 409 naming the current one.
        let (s, e) = call(&server, "POST", &versions, &env.alice, org, Some(body(1, "stale"))).await;
        assert_eq!(s, 409, "{e}");
        assert_eq!(e["error"], "taxonomy-version-conflict");
        assert_eq!(e["errors"]["args"]["current"], "2", "{e}");

        // Field tokens: duplicate topic id, empty list.
        let dup = json!({ "topics": [
            { "id": "a", "label": "A" }, { "id": "a", "label": "B" }
        ], "expected_version": 2 });
        let (s, e) = call(&server, "POST", &versions, &env.alice, org, Some(dup)).await;
        assert_eq!(s, 400);
        assert_eq!(e["topics.1.id"], "invalid", "{e}");
        let empty = json!({ "topics": [], "expected_version": 2 });
        let (s, e) = call(&server, "POST", &versions, &env.alice, org, Some(empty)).await;
        assert_eq!(s, 400);
        assert_eq!(e["topics"], "required", "{e}");

        let (s, got) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(s, 200);
        assert_eq!(got["current_version"], 2);
        assert_eq!(got["current"]["note"], "second");
        assert_eq!(got["current"]["topics"][1]["keywords"][0], "hôpital");
        let (_, list) = call(&server, "GET", "/api/v1/taxonomies", &env.alice, org, None).await;
        assert_eq!(list[0]["current"]["topics"].as_array().map(Vec::len), Some(2), "{list}");
        let (_, hist) = call(&server, "GET", &versions, &env.alice, org, None).await;
        let numbers: Vec<i64> = hist.as_array().unwrap().iter().map(|v| v["version"].as_i64().unwrap()).collect();
        assert_eq!(numbers, vec![2, 1], "newest first");

        let (s, renamed) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({ "name": "Radio news" }))).await;
        assert_eq!(s, 200, "{renamed}");
        assert_eq!(renamed["name"], "Radio news");
        assert_eq!(renamed["current_version"], 2);

        let (s, _) = call(&server, "DELETE", &path, &env.alice, org, None).await;
        assert_eq!(s, 204);
        let (s, e) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "taxonomy-not-found");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn taxonomy_org_isolation_and_viewer_read_only() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (s, t) = call(
            &server,
            "POST",
            "/api/v1/taxonomies",
            &env.alice,
            org_a,
            Some(json!({ "name": "News" })),
        )
        .await;
        assert_eq!(s, 201);
        let id = t["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/taxonomies/{id}");
        let versions = format!("{path}/versions");

        // Bob, in his own org, neither sees nor touches alice's taxonomy.
        let (_, list) = call(&server, "GET", "/api/v1/taxonomies", &env.bob, org_b, None).await;
        assert_eq!(list, json!([]));
        for (m, p, body) in [
            ("GET", path.clone(), None),
            ("GET", versions.clone(), None),
            ("PATCH", path.clone(), Some(json!({ "name": "X" }))),
            ("DELETE", path.clone(), None),
            (
                "POST",
                versions.clone(),
                Some(json!({ "topics": topics(), "expected_version": 0 })),
            ),
        ] {
            let (s, e) = call(&server, m, &p, &env.bob, org_b, body).await;
            assert_eq!(s, 404, "{m} {p}");
            assert_eq!(e["error"], "taxonomy-not-found");
        }

        // Viewer of alice's org: reads, never writes.
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org_a}/members"),
            &env.alice,
            org_a,
            Some(json!({ "email": "bob@example.com", "role": "viewer" })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s}");
        let (s, _) = call(&server, "GET", &path, &env.bob, org_a, None).await;
        assert_eq!(s, 200);
        let (s, _) = call(&server, "GET", &versions, &env.bob, org_a, None).await;
        assert_eq!(s, 200);
        for (m, p, body) in [
            (
                "POST",
                "/api/v1/taxonomies".to_string(),
                Some(json!({ "name": "V" })),
            ),
            ("PATCH", path.clone(), Some(json!({ "name": "V" }))),
            ("DELETE", path.clone(), None),
            (
                "POST",
                versions.clone(),
                Some(json!({ "topics": topics(), "expected_version": 0 })),
            ),
        ] {
            let (s, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(s, 403, "{m} {p}");
            assert_eq!(e["error"], "taxonomy-write-forbidden");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn topic_classify_deploy_needs_a_version_of_the_org() {
    with_app(|server, env| async move {
        let alice_org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let mut ids = Vec::new();
        for (token, org) in [(&env.alice, alice_org), (&env.bob, bob_org)] {
            let (s, t) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                token,
                org,
                Some(json!({ "name": "News" })),
            )
            .await;
            assert_eq!(s, 201, "{t}");
            let id = t["id"].as_str().unwrap().to_string();
            let (s, v) = call(
                &server,
                "POST",
                &format!("/api/v1/taxonomies/{id}/versions"),
                token,
                org,
                Some(json!({ "topics": topics(), "expected_version": 0 })),
            )
            .await;
            assert_eq!(s, 201, "{v}");
            ids.push(id);
        }
        let flow = |taxonomy: &str, version: i32| {
            json!({ "name": "topics", "graph": { "nodes": [
                { "id": "tc", "kind": "topic_classify",
                  "config": { "taxonomy_id": taxonomy, "version": version },
                  "outputs": [{ "port": 0, "targets": ["dbg"] }] },
                { "id": "dbg", "kind": "debug" }
            ]}})
        };
        let deploy = |body: Value| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                let (s, created) = call(
                    server,
                    "POST",
                    "/api/v1/flows",
                    &token,
                    alice_org,
                    Some(body),
                )
                .await;
                assert_eq!(s, 201, "{created}");
                let id = created["id"].as_i64().expect("flow id");
                call(
                    server,
                    "POST",
                    &format!("/api/v1/flows/{id}/deploy"),
                    &token,
                    alice_org,
                    Some(json!({})),
                )
                .await
            }
        };

        // Bob's taxonomy, and a version alice's does not have: refused.
        for (taxonomy, version) in [(&ids[1], 1), (&ids[0], 2)] {
            let (s, body) = deploy(flow(taxonomy, version)).await;
            assert_eq!(s, 400, "{body}");
            let violations = body["violations"].as_array().expect("violations");
            assert_eq!(violations.len(), 1, "{body}");
            assert_eq!(violations[0]["code"], "taxonomy-unknown");
            assert_eq!(violations[0]["node_id"], "tc");
        }

        // Own version: the gate lets it through (503 = engine off).
        let (s, body) = deploy(flow(&ids[0], 1)).await;
        assert_eq!(s, 503, "{body}");
    })
    .await;
}
