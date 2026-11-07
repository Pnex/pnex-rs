//! Agent environments (agent-environments.md D702, D704): versions in the database.
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

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_db(|server, env, _db| f(server, env)).await;
}

async fn with_db<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, sea_orm::DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    // These tests enrol runners; tests/rate_limit.rs covers the limit itself.
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

async fn registry(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let (s, r) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/image-registries",
        Some(json!({ "name": name, "host": "registry.example.org", "repository": "team/app" })),
    )
    .await;
    assert_eq!(s, 201, "{r}");
    r["id"].as_str().unwrap().to_string()
}

#[tokio::test]
#[serial]
async fn versions_pile_up_unchanged() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let reg = registry(&server, &env.alice, org, "r").await;
        let (s, e) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/environments",
            Some(json!({ "name": "coder", "registry_id": reg })),
        )
        .await;
        assert_eq!(s, 201, "{e}");
        assert!(e["latest_version"].is_null());
        let id = e["id"].as_str().unwrap();
        let versions = format!("/api/v1/environments/{id}/versions");

        let source = json!({
            "dockerfile": "FROM cgr.dev/chainguard/wolfi-base\nCOPY setup.sh /\n",
            "files": { "setup.sh": "echo ok" },
            "build_args": { "VERSION": "1" },
        });
        let (s, v1) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &versions,
            Some(source.clone()),
        )
        .await;
        assert_eq!(s, 201, "{v1}");
        assert_eq!(v1["number"], 1);
        let print = v1["fingerprint"].as_str().unwrap().to_string();
        assert!(print.starts_with("sha256:"), "{print}");

        // Another save needs to name what it was based on.
        let (s, d) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &versions,
            Some(source.clone()),
        )
        .await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-version-conflict", "{d}");

        let mut next = source.clone();
        next["dockerfile"] = json!("FROM cgr.dev/chainguard/wolfi-base\n");
        next["expected_latest"] = json!(1);
        let (s, v2) = call(&server, &env.alice, org, "POST", &versions, Some(next)).await;
        assert_eq!(s, 201, "{v2}");
        assert_eq!(v2["number"], 2);
        assert_ne!(v2["fingerprint"], print.as_str());

        // Version 1 stays what it was.
        let (s, got) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &format!("{versions}/1"),
            None,
        )
        .await;
        assert_eq!(s, 200, "{got}");
        assert_eq!(got["dockerfile"], source["dockerfile"]);
        assert_eq!(got["files"], source["files"]);
        assert_eq!(got["build_args"], source["build_args"]);
        assert_eq!(got["fingerprint"], print.as_str());

        let (s, listed) = call(&server, &env.alice, org, "GET", &versions, None).await;
        assert_eq!(s, 200, "{listed}");
        assert_eq!(listed["count"], 2, "{listed}");
        assert_eq!(listed["results"][0]["number"], 2, "newest first: {listed}");
        assert!(
            listed["results"][0]["dockerfile"].is_null(),
            "no source in a list"
        );

        let (s, d) = call(
            &server,
            &env.alice,
            org,
            "GET",
            &format!("{versions}/9"),
            None,
        )
        .await;
        assert_eq!(s, 404, "{d}");
        assert_eq!(d["error"], "environment-version-not-found", "{d}");

        // A registry in use stays; once free, it goes.
        let one_registry = format!("/api/v1/image-registries/{reg}");
        let (s, d) = call(&server, &env.alice, org, "DELETE", &one_registry, None).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "image-registry-in-use", "{d}");
        let one = format!("/api/v1/environments/{id}");
        assert_eq!(
            call(&server, &env.alice, org, "DELETE", &one, None).await.0,
            204
        );
        assert_eq!(
            call(&server, &env.alice, org, "DELETE", &one_registry, None)
                .await
                .0,
            204
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_credential_in_the_source_is_refused_without_being_echoed() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let reg = registry(&server, &env.alice, org, "r").await;
        let (_, e) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/environments",
            Some(json!({ "name": "coder", "registry_id": reg })),
        )
        .await;
        let versions = format!(
            "/api/v1/environments/{}/versions",
            e["id"].as_str().unwrap()
        );
        let token = format!("ghp_{}", "a".repeat(36));
        for (source, at) in [
            (
                json!({ "dockerfile": format!("FROM x\nENV T={token}") }),
                "dockerfile",
            ),
            (
                json!({ "dockerfile": "FROM x", "files": { "env.sh": token } }),
                "env.sh",
            ),
            (
                json!({ "dockerfile": "FROM x", "build_args": { "TOKEN": token } }),
                "TOKEN",
            ),
        ] {
            let (s, d) = call(&server, &env.alice, org, "POST", &versions, Some(source)).await;
            assert_eq!(s, 400, "{d}");
            assert_eq!(d["error"], "environment-secret-in-dockerfile", "{d}");
            assert_eq!(d["errors"]["args"]["at"], at, "{d}");
            assert!(!d.to_string().contains(&token), "{d}");
        }
        let (_, listed) = call(&server, &env.alice, org, "GET", &versions, None).await;
        assert_eq!(listed["count"], 0, "nothing saved: {listed}");

        // A path out of the context.
        let (s, d) = call(
            &server,
            &env.alice,
            org,
            "POST",
            &versions,
            Some(json!({ "dockerfile": "FROM x", "files": { "../x": "y" } })),
        )
        .await;
        assert_eq!(s, 400, "{d}");
    })
    .await;
}

/// R1 and R2: another org sees nothing, a registry of another org cannot be named, and a
/// viewer only reads.
#[tokio::test]
#[serial]
async fn rights_and_org_isolation() {
    with_app(|server, env| async move {
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        let reg = registry(&server, &env.alice, a, "r").await;
        let list = "/api/v1/environments";
        let body = json!({ "name": "coder", "registry_id": reg });
        let (_, e) = call(&server, &env.alice, a, "POST", list, Some(body.clone())).await;
        let one = format!("{list}/{}", e["id"].as_str().unwrap());
        let versions = format!("{one}/versions");
        let source = json!({ "dockerfile": "FROM x" });
        let (s, d) = call(
            &server,
            &env.alice,
            a,
            "POST",
            &versions,
            Some(source.clone()),
        )
        .await;
        assert_eq!(s, 201, "{d}");

        // Bob in his own org.
        let (s, d) = call(&server, &env.bob, b, "POST", list, Some(body.clone())).await;
        assert_eq!(s, 400, "another org's registry: {d}");
        for (method, path, body) in [
            ("GET", &one, None),
            ("DELETE", &one, None),
            ("GET", &versions, None),
            ("POST", &versions, Some(source.clone())),
        ] {
            let (s, d) = call(&server, &env.bob, b, method, path, body).await;
            assert_eq!(s, 404, "{method} {path}: {d}");
        }
        let (s, d) = call(&server, &env.bob, b, "GET", &format!("{versions}/1"), None).await;
        assert_eq!(s, 404, "{d}");

        // Bob as a viewer of Alice's org.
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            a,
            "bob@example.com",
            "viewer",
        )
        .await;
        assert_eq!(call(&server, &env.bob, a, "GET", &one, None).await.0, 200);
        assert_eq!(
            call(&server, &env.bob, a, "GET", &versions, None).await.0,
            200
        );
        for (method, path, body) in [
            ("POST", list, Some(body.clone())),
            ("PUT", one.as_str(), Some(body.clone())),
            ("DELETE", one.as_str(), None),
            ("POST", versions.as_str(), Some(source.clone())),
        ] {
            let (s, d) = call(&server, &env.bob, a, method, path, body).await;
            assert_eq!(s, 403, "{method} {path}: {d}");
        }
    })
    .await;
}

const BUILDER: &str =
    "r.example/builder@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DIGEST: &str = "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

async fn enrol(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    capabilities: &[&str],
) -> String {
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
        .json(&json!({ "join_token": t["token"], "capabilities": capabilities }))
        .await;
    assert!(res.status_code().is_success(), "{}", res.text());
    res.json::<Value>()["token"].as_str().unwrap().to_string()
}

async fn runner_post(
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

/// An environment with one version, on a registry with a write secret. Returns its id.
async fn buildable(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let (s, r) = call(
        server,
        token,
        org,
        "POST",
        "/api/v1/image-registries",
        Some(json!({
            "name": "pushable",
            "host": "registry.example.org",
            "repository": "team/app",
            "write_username": "robot",
            "write_secret": { "value": "push-secret-value" },
        })),
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
        Some(json!({ "dockerfile": "FROM x", "files": { "a/b.sh": "echo ok" } })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    id
}

#[tokio::test]
#[serial]
async fn a_build_goes_through_a_runner_and_ends_on_a_digest() {
    with_app(|server, env| async move {
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        let id = buildable(&server, &env.alice, a).await;
        let start = format!("/api/v1/environments/{id}/versions/1/builds");

        // No builder image on this deployment.
        unsafe { std::env::remove_var("PNEX_IMAGE_BUILDER") };
        let (s, d) = call(&server, &env.alice, a, "POST", &start, None).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-build-unavailable", "{d}");

        // A tag is no builder image.
        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", "r.example/builder:latest") };
        let (s, d) = call(&server, &env.alice, a, "POST", &start, None).await;
        assert_eq!(s, 409, "{d}");

        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", BUILDER) };
        let (s, build) = call(&server, &env.alice, a, "POST", &start, None).await;
        assert_eq!(s, 201, "{build}");
        assert_eq!(build["state"], "building");
        assert!(build["image"].is_null());
        let one = format!(
            "/api/v1/environments/{id}/builds/{}",
            build["id"].as_str().unwrap()
        );

        // One at a time.
        let (s, d) = call(&server, &env.alice, a, "POST", &start, None).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-build-running", "{d}");

        // The job kept no credential, and nobody queues a build by hand.
        let (_, jobs) = call(&server, &env.alice, a, "GET", "/api/v1/runner-jobs", None).await;
        assert!(!jobs.to_string().contains("push-secret-value"), "{jobs}");
        let (s, d) = call(
            &server,
            &env.alice,
            a,
            "POST",
            "/api/v1/runner-jobs",
            Some(json!({
                "kind": "environment_build",
                "spec": { "image": BUILDER, "timeout_secs": 60 }
            })),
        )
        .await;
        assert_eq!(s, 400, "{d}");

        // Neither a runner that cannot build, nor one of another org, gets it.
        let plain = enrol(&server, &env.alice, a, &["arch:amd64"]).await;
        let (s, _) = runner_post(&server, &plain, "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 204);
        let other = enrol(&server, &env.bob, b, &["builder:buildkit"]).await;
        let (s, _) = runner_post(&server, &other, "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 204);

        // The right runner gets the source, the target and the write secret.
        let builder = enrol(&server, &env.alice, a, &["builder:buildkit"]).await;
        let (s, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 200, "{job}");
        assert_eq!(job["kind"], "environment_build");
        assert_eq!(job["spec"]["image"], BUILDER);
        let input = &job["build"];
        assert_eq!(input["target"], "registry.example.org/team/app:v1");
        assert_eq!(input["registry_host"], "registry.example.org");
        assert_eq!(input["username"], "robot");
        assert_eq!(input["secret"], "push-secret-value");
        assert_eq!(input["source"]["dockerfile"], "FROM x");
        assert_eq!(input["source"]["files"]["a/b.sh"], "echo ok");
        let job_id = job["id"].as_str().unwrap();
        let finish = format!("/api/v1/runner/jobs/{job_id}/finish");

        // Another runner of the org cannot finish it.
        let body = json!({ "outcome": "succeeded", "exit_code": 0, "image_digest": DIGEST });
        let (s, _) = runner_post(&server, &plain, &finish, Some(body.clone())).await;
        assert_eq!(s, 409);
        let (_, still) = call(&server, &env.alice, a, "GET", &one, None).await;
        assert_eq!(still["state"], "building", "{still}");

        let (s, d) = runner_post(&server, &builder, &finish, Some(body)).await;
        assert_eq!(s, 204, "{d}");
        let (_, done) = call(&server, &env.alice, a, "GET", &one, None).await;
        assert_eq!(done["state"], "ready", "{done}");
        assert_eq!(
            done["image"],
            format!("registry.example.org/team/app@{DIGEST}")
        );

        // A success without a usable digest is a failure, and frees the next build.
        for digest in [json!(null), json!("sha256:nope")] {
            let (s, again) = call(&server, &env.alice, a, "POST", &start, None).await;
            assert_eq!(s, 201, "{again}");
            let (_, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
            let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
            let body = json!({ "outcome": "succeeded", "exit_code": 0, "image_digest": digest });
            let (s, d) = runner_post(&server, &builder, &finish, Some(body)).await;
            assert_eq!(s, 204, "{d}");
            let (_, got) = call(
                &server,
                &env.alice,
                a,
                "GET",
                &format!(
                    "/api/v1/environments/{id}/builds/{}",
                    again["id"].as_str().unwrap()
                ),
                None,
            )
            .await;
            assert_eq!(got["state"], "failed", "{got}");
            assert!(got["image"].is_null(), "{got}");
        }

        let (_, listed) = call(
            &server,
            &env.alice,
            a,
            "GET",
            &format!("/api/v1/environments/{id}/builds"),
            None,
        )
        .await;
        assert_eq!(listed["count"], 3, "{listed}");
        assert!(!listed.to_string().contains("push-secret-value"));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn building_needs_its_right_and_a_write_secret() {
    with_app(|server, env| async move {
        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", BUILDER) };
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        let id = buildable(&server, &env.alice, a).await;
        let start = format!("/api/v1/environments/{id}/versions/1/builds");

        // Bob in his org: nothing there. A version that does not exist: nothing either.
        assert_eq!(
            call(&server, &env.bob, b, "POST", &start, None).await.0,
            404
        );
        let (s, d) = call(
            &server,
            &env.alice,
            a,
            "POST",
            &format!("/api/v1/environments/{id}/versions/9/builds"),
            None,
        )
        .await;
        assert_eq!(s, 404, "{d}");

        // A registry that cannot push.
        let reg = registry(&server, &env.alice, a, "read-only").await;
        let (_, e) = call(
            &server,
            &env.alice,
            a,
            "POST",
            "/api/v1/environments",
            Some(json!({ "name": "stuck", "registry_id": reg })),
        )
        .await;
        let stuck = e["id"].as_str().unwrap();
        let (s, _) = call(
            &server,
            &env.alice,
            a,
            "POST",
            &format!("/api/v1/environments/{stuck}/versions"),
            Some(json!({ "dockerfile": "FROM x" })),
        )
        .await;
        assert_eq!(s, 201);
        let (s, d) = call(
            &server,
            &env.alice,
            a,
            "POST",
            &format!("/api/v1/environments/{stuck}/versions/1/builds"),
            None,
        )
        .await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-build-no-write-secret", "{d}");

        // Bob as a viewer reads, and cannot build.
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            a,
            "bob@example.com",
            "viewer",
        )
        .await;
        assert_eq!(
            call(&server, &env.bob, a, "POST", &start, None).await.0,
            403
        );
        let builds = format!("/api/v1/environments/{id}/builds");
        assert_eq!(
            call(&server, &env.bob, a, "GET", &builds, None).await.0,
            200
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_person_activates_a_ready_build_and_it_is_scanned_again() {
    use sea_orm::ConnectionTrait;
    with_db(|server, env, db| async move {
        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", BUILDER) };
        unsafe { std::env::remove_var("PNEX_IMAGE_SCANNER") };
        let _ = personal_org(&server, &env.bob).await;
        let a = personal_org(&server, &env.alice).await;
        let id = buildable(&server, &env.alice, a).await;
        let start = format!("/api/v1/environments/{id}/versions/1/builds");
        let one = format!("/api/v1/environments/{id}");
        let (_, build) = call(&server, &env.alice, a, "POST", &start, None).await;
        let activate = format!("{one}/builds/{}/activate", build["id"].as_str().unwrap());
        let v1 = json!({ "expected_version": 1 });

        // Still building: refused.
        let (s, d) = call(&server, &env.alice, a, "POST", &activate, Some(v1.clone())).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-build-not-ready", "{d}");

        let builder = enrol(&server, &env.alice, a, &["builder:buildkit"]).await;
        let (_, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let body = json!({ "outcome": "succeeded", "image_digest": DIGEST });
        runner_post(&server, &builder, &finish, Some(body)).await;

        // A viewer cannot, a stale version cannot.
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            a,
            "bob@example.com",
            "viewer",
        )
        .await;
        let (s, _) = call(&server, &env.bob, a, "POST", &activate, Some(v1.clone())).await;
        assert_eq!(s, 403);
        let stale = json!({ "expected_version": 9 });
        let (s, d) = call(&server, &env.alice, a, "POST", &activate, Some(stale)).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "environment-version-conflict", "{d}");

        let (s, e) = call(&server, &env.alice, a, "POST", &activate, Some(v1)).await;
        assert_eq!(s, 200, "{e}");
        assert_eq!(e["active_build_id"], build["id"]);
        assert_eq!(
            e["active_image"],
            format!("registry.example.org/team/app@{DIGEST}")
        );
        assert_eq!(e["version"], 2);

        // Without a scanner, nothing is due. With one, the active build is, then not again.
        use pnex_backend::services::environments::rescan_due;
        assert_eq!(rescan_due(&db, None).await.unwrap(), 0);
        assert_eq!(rescan_due(&db, Some(SCANNER)).await.unwrap(), 1);
        assert_eq!(rescan_due(&db, Some(SCANNER)).await.unwrap(), 0);
        let built = format!("{one}/builds/{}", build["id"].as_str().unwrap());
        let (_, seen) = call(&server, &env.alice, a, "GET", &built, None).await;
        assert_eq!(seen["scan_state"], "scanning", "{seen}");

        // A day later it is due again.
        db.execute_unprepared(
            "UPDATE environment_scans SET created_at = now() - interval '25 hours'",
        )
        .await
        .unwrap();
        assert_eq!(rescan_due(&db, Some(SCANNER)).await.unwrap(), 1);
    })
    .await;
}

const SCANNER: &str =
    "r.example/scanner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
#[serial]
async fn a_finished_build_gets_scanned() {
    with_app(|server, env| async move {
        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", BUILDER) };
        unsafe { std::env::set_var("PNEX_IMAGE_SCANNER", SCANNER) };
        let a = personal_org(&server, &env.alice).await;
        let id = buildable(&server, &env.alice, a).await;
        let start = format!("/api/v1/environments/{id}/versions/1/builds");
        let (_, build) = call(&server, &env.alice, a, "POST", &start, None).await;
        let one = format!(
            "/api/v1/environments/{id}/builds/{}",
            build["id"].as_str().unwrap()
        );
        let scan_of = format!("{one}/scan");
        let (s, d) = call(&server, &env.alice, a, "GET", &scan_of, None).await;
        assert_eq!(s, 404, "{d}");
        assert_eq!(d["error"], "environment-scan-not-found", "{d}");

        let builder = enrol(&server, &env.alice, a, &["builder:buildkit"]).await;
        let scanner = enrol(&server, &env.alice, a, &["scanner:trivy"]).await;
        let (_, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let body = json!({ "outcome": "succeeded", "exit_code": 0, "image_digest": DIGEST });
        let (s, _) = runner_post(&server, &builder, &finish, Some(body)).await;
        assert_eq!(s, 204);

        let (_, seen) = call(&server, &env.alice, a, "GET", &one, None).await;
        assert_eq!(seen["state"], "ready", "{seen}");
        assert_eq!(seen["scan_state"], "scanning", "{seen}");

        // The scan reads the pinned image, with no write secret anywhere.
        let (s, job) = runner_post(&server, &scanner, "/api/v1/runner/jobs/claim", None).await;
        assert_eq!(s, 200, "{job}");
        assert_eq!(job["kind"], "environment_scan");
        assert_eq!(job["spec"]["image"], SCANNER);
        let command = job["spec"]["command"].as_array().unwrap();
        assert_eq!(command[0], "sh");
        assert_eq!(
            command.last().unwrap(),
            &json!(format!("registry.example.org/team/app@{DIGEST}"))
        );
        assert!(!job.to_string().contains("push-secret-value"), "{job}");
        assert!(
            job["scan"].is_null(),
            "this registry has no read login: {job}"
        );

        // A report out of bounds is dropped: the scan failed, the build stays ready.
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let finding = json!({
            "id": "CVE-2021-36159", "package": "apk-tools", "installed": "2.10.6-r0",
            "fixed": "2.10.7-r0", "severity": "critical"
        });
        let mut wrong = finding.clone();
        wrong["severity"] = json!("nope");
        let body = json!({ "outcome": "succeeded", "exit_code": 0,
            "scan_report": { "components": [], "findings": [wrong] } });
        let (s, _) = runner_post(&server, &scanner, &finish, Some(body)).await;
        assert_eq!(s, 204);
        let (_, seen) = call(&server, &env.alice, a, "GET", &one, None).await;
        assert_eq!(seen["state"], "ready", "{seen}");
        assert_eq!(seen["scan_state"], "failed", "{seen}");

        // The next build goes all the way.
        let (_, build) = call(&server, &env.alice, a, "POST", &start, None).await;
        let one = format!(
            "/api/v1/environments/{id}/builds/{}",
            build["id"].as_str().unwrap()
        );
        let (_, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let body = json!({ "outcome": "succeeded", "image_digest": DIGEST });
        runner_post(&server, &builder, &finish, Some(body)).await;
        let (_, job) = runner_post(&server, &scanner, "/api/v1/runner/jobs/claim", None).await;
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let body = json!({ "outcome": "succeeded", "exit_code": 0,
            "scan_report": { "components": [], "findings": [finding] } });
        let (s, _) = runner_post(&server, &scanner, &finish, Some(body)).await;
        assert_eq!(s, 204);

        let (_, seen) = call(&server, &env.alice, a, "GET", &one, None).await;
        assert_eq!(seen["scan_state"], "done", "{seen}");
        assert_eq!(seen["scan_counts"]["critical"], 1, "{seen}");
        assert_eq!(seen["scan_counts"]["fixable"], 1, "{seen}");
        assert!(
            seen.get("report").is_none(),
            "no findings on the build: {seen}"
        );
        let (s, scan) = call(&server, &env.alice, a, "GET", &format!("{one}/scan"), None).await;
        assert_eq!(s, 200, "{scan}");
        assert_eq!(scan["report"]["findings"][0]["id"], "CVE-2021-36159");
        assert_eq!(scan["report"]["findings"][0]["fixed"], "2.10.7-r0");

        // A scan is no job to queue by hand, and another org sees nothing of it.
        let (s, _) = call(
            &server,
            &env.alice,
            a,
            "POST",
            "/api/v1/runner-jobs",
            Some(json!({
                "kind": "environment_scan",
                "spec": { "image": SCANNER, "timeout_secs": 60 }
            })),
        )
        .await;
        assert_eq!(s, 400);
        let b = personal_org(&server, &env.bob).await;
        let (s, _) = call(&server, &env.bob, b, "GET", &format!("{one}/scan"), None).await;
        assert_eq!(s, 404);
        unsafe { std::env::remove_var("PNEX_IMAGE_SCANNER") };
    })
    .await;
}

/// The SBOM goes to the journal, and the scan into the ontology.
#[tokio::test]
#[serial]
async fn the_sbom_is_stored_and_traced_in_the_ontology() {
    use pnex_backend::services::ontology::objects::find_by_key;
    use pnex_backend::services::software_supply::project_scan;
    with_db(|server, env, db| async move {
        unsafe { std::env::set_var("PNEX_IMAGE_BUILDER", BUILDER) };
        unsafe { std::env::set_var("PNEX_IMAGE_SCANNER", SCANNER) };
        let a = personal_org(&server, &env.alice).await;
        let id = buildable(&server, &env.alice, a).await;
        let start = format!("/api/v1/environments/{id}/versions/1/builds");
        let (_, build) = call(&server, &env.alice, a, "POST", &start, None).await;
        let builder = enrol(&server, &env.alice, a, &["builder:buildkit"]).await;
        let scanner = enrol(&server, &env.alice, a, &["scanner:trivy"]).await;
        let (_, job) = runner_post(&server, &builder, "/api/v1/runner/jobs/claim", None).await;
        let finish = format!("/api/v1/runner/jobs/{}/finish", job["id"].as_str().unwrap());
        let body = json!({ "outcome": "succeeded", "image_digest": DIGEST });
        runner_post(&server, &builder, &finish, Some(body)).await;
        let (_, scan_job) = runner_post(&server, &scanner, "/api/v1/runner/jobs/claim", None).await;
        let job_id = scan_job["id"].as_str().unwrap().to_string();
        let upload = format!("/api/v1/runner/jobs/{job_id}/sbom");

        // Only the runner holding the job uploads.
        let document = r#"{"bomFormat":"CycloneDX","specVersion":"1.6"}"#;
        let res = server
            .post(&upload)
            .add_header("Authorization", format!("Bearer {builder}"))
            .add_header("Content-Type", "application/json")
            .bytes(document.as_bytes().to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 409, "{}", res.text());
        // As the runner does: a just-provisioned org may not take its first record yet.
        let mut stored = 0;
        for _ in 0..20 {
            let res = server
                .post(&upload)
                .add_header("Authorization", format!("Bearer {scanner}"))
                .add_header("Content-Type", "application/json")
                .bytes(document.as_bytes().to_vec().into())
                .await;
            stored = res.status_code().as_u16();
            if stored == 204 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        assert_eq!(stored, 204);

        let component =
            |purl: &str, name: &str| json!({ "purl": purl, "name": name, "version": "1" });
        let finding = |advisory: &str, package: &str| {
            json!({
                "id": advisory, "purl": null, "package": package, "installed": "1",
                "fixed": "2", "severity": "critical"
            })
        };
        let report = json!({
            "components": [component("pkg:apk/a@1", "a"), component("pkg:apk/b@1", "b")],
            "findings": [finding("CVE-1", "a"), finding("CVE-1", "b"), finding("CVE-2", "a")],
        });
        let finish = format!("/api/v1/runner/jobs/{job_id}/finish");
        let body = json!({ "outcome": "succeeded", "scan_report": report });
        let (s, d) = runner_post(&server, &scanner, &finish, Some(body)).await;
        assert_eq!(s, 204, "{d}");

        // With OpenObserve (`PNEX_O2_URL`), the document comes back whole as an attachment
        // and the scan shows in the follow-up. Without, a scan still ends.
        let one = format!(
            "/api/v1/environments/{id}/builds/{}",
            build["id"].as_str().unwrap()
        );
        let scans = format!("/api/v1/environments/{id}/scans");
        if std::env::var("PNEX_O2_URL").is_ok_and(|u| !u.is_empty()) {
            let mut res = None;
            for _ in 0..40 {
                let got = server
                    .get(&format!("{one}/sbom"))
                    .add_header("Authorization", format!("Bearer {}", env.alice))
                    .add_header("X-Org-Id", a.to_string())
                    .await;
                if got.status_code().as_u16() == 200 {
                    res = Some(got);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let res = res.expect("the SBOM comes back");
            assert_eq!(res.text(), document);
            let disposition = res.header("content-disposition");
            assert!(disposition.to_str().unwrap().starts_with("attachment"));
            let mut followed = Value::Null;
            for _ in 0..40 {
                let (s, v) = call(&server, &env.alice, a, "GET", &scans, None).await;
                assert_eq!(s, 200, "{v}");
                if v["count"].as_i64().unwrap_or(0) > 0 {
                    followed = v;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            assert_eq!(
                followed["results"][0]["counts"]["critical"], 3,
                "{followed}"
            );
            assert_eq!(followed["results"][0]["components"], 2, "{followed}");
        } else {
            let (s, d) = call(&server, &env.alice, a, "GET", &format!("{one}/sbom"), None).await;
            assert_eq!(s, 404, "{d}");
            let (s, d) = call(&server, &env.alice, a, "GET", &scans, None).await;
            assert_eq!(s, 503, "{d}");
            assert_eq!(d["error"], "events-unavailable", "{d}");
        }

        // Objects, on their natural keys. Direct call: same effect as the background one.
        let job = uuid::Uuid::parse_str(&job_id).unwrap();
        let mut projected = project_scan(&db, a, job).await;
        for _ in 0..50 {
            if projected.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            projected = project_scan(&db, a, job).await;
        }
        projected.expect("projected");
        let reference = json!(format!("registry.example.org/team/app@{DIGEST}"));
        let by_key = |t: &'static str, k: &'static str, v: Value| {
            let db = db.clone();
            async move { find_by_key(&db, a, t, k, &v).await.unwrap() }
        };
        let image = by_key("software_image", "reference", reference.clone())
            .await
            .expect("image");
        let bom = by_key("software_bom", "image_reference", reference.clone())
            .await
            .expect("sbom");
        assert_eq!(bom.properties["component_count"], 2);
        assert_eq!(bom.properties["format"], "CycloneDX");
        assert!(bom.properties["document"]
            .as_str()
            .unwrap()
            .ends_with("/sbom"));
        by_key("software_component", "purl", json!("pkg:apk/a@1"))
            .await
            .expect("component");
        let cve1 = by_key("software_vulnerability", "advisory", json!("CVE-1"))
            .await
            .expect("flaw");
        assert_eq!(cve1.properties["severity"], "critical");

        let links = |object: uuid::Uuid| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (s, v) = call(
                    server,
                    &token,
                    a,
                    "GET",
                    &format!("/api/v1/ontology/objects/{object}/links"),
                    None,
                )
                .await;
                assert_eq!(s, 200, "{v}");
                v.to_string()
            }
        };
        let of_image = links(image.id).await;
        assert!(of_image.contains("image_has_bom"), "{of_image}");
        assert!(of_image.contains("image_exposed_to"), "{of_image}");
        assert!(of_image.contains("a, b"), "packages gathered: {of_image}");
        assert!(links(bom.id).await.contains("bom_lists_component"));

        // Again: nothing doubles, and a flaw no longer found closes.
        project_scan(&db, a, job).await.expect("idempotent");
        let later = json!({
            "components": [component("pkg:apk/a@1", "a"), component("pkg:apk/b@1", "b")],
            "findings": [finding("CVE-2", "a")],
        });
        pnex_backend::services::environments::record_scan(
            &db,
            a,
            job,
            &serde_json::from_value(later).unwrap(),
        )
        .await
        .unwrap();
        project_scan(&db, a, job).await.expect("reprojected");
        let open: Value = serde_json::from_str(&links(image.id).await).unwrap();
        let exposures = open
            .as_array()
            .or_else(|| open["results"].as_array())
            .unwrap()
            .iter()
            .filter(|l| l.to_string().contains("image_exposed_to"))
            .count();
        assert_eq!(exposures, 1, "{open}");
        assert!(by_key("software_vulnerability", "advisory", json!("CVE-1"))
            .await
            .is_some());
        unsafe { std::env::remove_var("PNEX_IMAGE_SCANNER") };
    })
    .await;
}

/// On the compose `rustfs`: credentials work inside their prefix and nowhere else.
#[tokio::test]
#[serial]
async fn cache_credentials_stop_at_their_prefix() {
    use pnex_backend::services::artifact_store::S3Config;
    use pnex_backend::services::build_cache::{self, SigningKeys};

    let config = S3Config {
        endpoint: "http://localhost:9000".into(),
        bucket: "pnex-test".into(),
        region: String::new(),
        access_key: "rustfsadmin".into(),
        secret_key: "rustfsadmin".into(),
        path_style: true,
    };
    let inside = "org_1/buildcache/env-a/";
    let cache = build_cache::for_prefix(&config, "http://pod-view:9000", inside, 900)
        .await
        .expect("temporary credentials");
    assert_eq!(cache.endpoint, "http://pod-view:9000");
    assert_eq!(cache.bucket, "pnex-test");
    assert_ne!(cache.access_key, "rustfsadmin");

    let keys = SigningKeys {
        access_key: &cache.access_key,
        secret_key: &cache.secret_key,
        session_token: Some(&cache.session_token),
    };
    let root = SigningKeys {
        access_key: "rustfsadmin",
        secret_key: "rustfsadmin",
        session_token: None,
    };
    let client = reqwest::Client::new();
    let call = |method: reqwest::Method, key: &str, by: &SigningKeys<'_>| {
        let url = reqwest::Url::parse(&format!("http://localhost:9000/pnex-test/{key}")).unwrap();
        build_cache::signed(
            &client,
            method,
            &url,
            ("s3", "us-east-1"),
            by,
            None,
            b"x".to_vec(),
        )
    };
    use reqwest::Method;
    let other = "org_2/buildcache/env-a/victim";
    let sibling = "org_1/buildcache/env-b/victim";
    for key in [other, sibling] {
        let put = call(Method::PUT, key, &root).send().await.unwrap();
        assert!(put.status().is_success(), "{}", put.status());
    }
    let mine = format!("{inside}layer");
    let put = call(Method::PUT, &mine, &keys).send().await.unwrap();
    assert!(put.status().is_success(), "inside: {}", put.status());

    let list = {
        let url = reqwest::Url::parse("http://localhost:9000/pnex-test/?list-type=2").unwrap();
        build_cache::signed(
            &client,
            Method::GET,
            &url,
            ("s3", "us-east-1"),
            &keys,
            None,
            vec![],
        )
    };
    assert_eq!(list.send().await.unwrap().status(), 403, "no listing");
    for key in [other, sibling] {
        for method in [Method::GET, Method::PUT, Method::DELETE] {
            let res = call(method.clone(), key, &keys).send().await.unwrap();
            assert_eq!(res.status(), 403, "{method} {key}");
        }
    }
    for key in [other, sibling, mine.as_str()] {
        let _ = call(Method::DELETE, key, &root).send().await;
    }
}
