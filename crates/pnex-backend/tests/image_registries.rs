//! Image registries (agent-environments.md D703): vault, CRUD, rights, and the test
//! against a stand-in registry speaking the bearer flow of the distribution spec.
//! `a_real_registry_answers` runs against a real one, see its comment.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL). The test config runs with the `open` egress
//! policy, so that a registry on loopback can be reached.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::{json, Value};
use serial_test::serial;

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

fn registry(name: &str, host: &str) -> Value {
    json!({
        "name": name,
        "host": host,
        "repository": "team/app",
        "tls": false,
        "read_username": "reader",
        "read_secret": { "value": "read-pass-1" },
        "write_username": "writer",
        "write_secret": { "value": "write-pass-1" },
    })
}

#[derive(Default)]
struct Seen {
    /// `Authorization` sent on the first `GET /v2/`, which must be none.
    hello_authorization: Vec<String>,
    cancelled: Vec<String>,
}

type Shared = Arc<Mutex<Seen>>;

fn basic_user(headers: &HeaderMap) -> Option<(String, String)> {
    let raw = headers.get("authorization")?.to_str().ok()?;
    let raw = raw.strip_prefix("Basic ")?;
    let raw = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
    let (user, secret) = std::str::from_utf8(&raw).ok()?.split_once(':')?;
    Some((user.to_string(), secret.to_string()))
}

/// Grants what the login may do: the reader pulls, the writer also pushes.
async fn token(
    headers: HeaderMap,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    assert_eq!(q.get("service").map(String::as_str), Some("stand-in"));
    let scope = q.get("scope").cloned().unwrap_or_default();
    assert!(scope.starts_with("repository:team/app:"), "{scope}");
    let granted = match basic_user(&headers) {
        Some((user, secret)) if user == "reader" && secret == "read-pass-1" => "pull",
        Some((user, secret)) if user == "writer" && secret == "write-pass-1" => "push",
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    axum::Json(json!({ "token": granted })).into_response()
}

async fn v2(
    State((seen, realm)): State<(Shared, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    if uri.path() == "/v2/" {
        seen.lock().unwrap().hello_authorization.push(bearer);
        return (
            StatusCode::UNAUTHORIZED,
            [(
                "www-authenticate",
                format!(r#"Bearer realm="{realm}",service="stand-in""#),
            )],
        )
            .into_response();
    }
    let can_push = bearer == "Bearer push";
    let can_pull = can_push || bearer == "Bearer pull";
    match (method.as_str(), uri.path()) {
        ("GET", "/v2/team/app/tags/list") if can_pull => StatusCode::NOT_FOUND.into_response(),
        ("POST", "/v2/team/app/blobs/uploads/") if can_push => (
            StatusCode::ACCEPTED,
            [("location", "/v2/team/app/blobs/uploads/session-1")],
        )
            .into_response(),
        ("DELETE", "/v2/team/app/blobs/uploads/session-1") if can_push => {
            seen.lock().unwrap().cancelled.push(uri.path().to_string());
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// A registry on loopback, and what it saw. Returns its `host:port`.
async fn stand_in() -> (String, Shared) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Shared::default();
    let app = axum::Router::new()
        .route("/token", axum::routing::get(token))
        .fallback(v2)
        .with_state((seen.clone(), format!("http://{addr}/token")));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr.to_string(), seen)
}

#[tokio::test]
#[serial]
async fn the_test_proves_each_right_with_its_own_secret() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (host, seen) = stand_in().await;
        let (s, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(registry("good", &host)),
        )
        .await;
        assert_eq!(s, 201, "{r}");
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (s, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(s, 200, "{t}");
        assert_eq!(t["read"]["ok"], true, "{t}");
        assert_eq!(t["write"]["ok"], true, "{t}");
        {
            let seen = seen.lock().unwrap();
            assert!(
                seen.hello_authorization.iter().all(String::is_empty),
                "nothing is sent before the challenge"
            );
            assert_eq!(seen.cancelled.len(), 1, "the upload is cancelled");
        }

        // A wrong secret is a refused login.
        let mut wrong = registry("wrong", &host);
        wrong["read_secret"] = json!({ "value": "nope" });
        wrong["write_secret"] = json!({ "value": "nope-either" });
        let (_, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(wrong),
        )
        .await;
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["code"], "image-registry-auth-failed", "{t}");
        assert_eq!(t["write"]["code"], "image-registry-auth-failed", "{t}");

        // A login that only pulls, given as the write one, fails on the right and not
        // on the login. Without a write secret, no write check.
        let mut weak = registry("weak", &host);
        weak["write_username"] = json!("reader");
        weak["write_secret"] = json!({ "value": "read-pass-1" });
        let (_, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(weak),
        )
        .await;
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["ok"], true, "{t}");
        assert_eq!(t["write"]["code"], "image-registry-access-denied", "{t}");

        let mut pull_only = registry("pull-only", &host);
        pull_only.as_object_mut().unwrap().remove("write_username");
        pull_only.as_object_mut().unwrap().remove("write_secret");
        let (_, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(pull_only),
        )
        .await;
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["ok"], true, "{t}");
        assert!(t["write"].is_null(), "{t}");

        // Nothing listens there.
        let (_, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(registry("down", "127.0.0.1:9")),
        )
        .await;
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["code"], "image-registry-unreachable", "{t}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn secrets_stay_apart_in_the_vault_and_out_of_answers() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let list = "/api/v1/image-registries";
        let (s, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            list,
            Some(registry("agents", "registry.example.org")),
        )
        .await;
        assert_eq!(s, 201, "{r}");
        let id = r["id"].as_str().unwrap().to_string();
        let one = format!("{list}/{id}");
        assert_eq!(r["has_read_secret"], true);
        assert_eq!(r["has_write_secret"], true);
        let (_, listed) = call(&server, &env.alice, org, "GET", list, None).await;
        let text = format!("{r}{listed}");
        assert!(!text.contains("read-pass-1") && !text.contains("write-pass-1"));
        assert!(!text.contains("secret_id"), "{text}");

        // Two secrets in the vault, each with its usage.
        let usages = ctx
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT field FROM secret_usages WHERE consumer_kind = 'image-registry' \
                 AND consumer_id = $1 ORDER BY field",
                vec![id.clone().into()],
            ))
            .await
            .unwrap();
        let fields: Vec<String> = usages
            .iter()
            .map(|u| u.try_get("", "field").unwrap())
            .collect();
        assert_eq!(fields, ["read", "write"]);

        // The name is unique in the org.
        let (s, d) = call(
            &server,
            &env.alice,
            org,
            "POST",
            list,
            Some(registry("agents", "registry.example.org")),
        )
        .await;
        assert_eq!(s, 409, "{d}");

        // A user without a secret, and a secret without a user, cannot log in.
        let mut lone = registry("lone", "registry.example.org");
        lone.as_object_mut().unwrap().remove("read_secret");
        assert_eq!(
            call(&server, &env.alice, org, "POST", list, Some(lone))
                .await
                .0,
            400
        );

        // One secret cannot fill both roles (D703).
        let (_, v) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/secrets",
            Some(json!({ "name": "shared-robot", "value": "x" })),
        )
        .await;
        let shared = v["id"].as_str().expect("secret created").to_string();
        let mut both = registry("both", "registry.example.org");
        both["read_secret"] = json!({ "secret_id": shared });
        both["write_secret"] = json!({ "secret_id": shared });
        assert_eq!(
            call(&server, &env.alice, org, "POST", list, Some(both))
                .await
                .0,
            400
        );

        // Moving the registry asks for its secrets again; otherwise they are kept.
        let mut moved = registry("agents", "elsewhere.example.org");
        moved["expected_version"] = json!(1);
        let keep = moved.as_object_mut().unwrap();
        keep.remove("read_secret");
        keep.remove("write_secret");
        let (s, d) = call(&server, &env.alice, org, "PUT", &one, Some(moved)).await;
        assert_eq!(s, 400, "{d}");

        let mut renamed = registry("renamed", "registry.example.org");
        renamed["expected_version"] = json!(1);
        let keep = renamed.as_object_mut().unwrap();
        keep.remove("read_secret");
        keep.remove("write_secret");
        let (s, u) = call(&server, &env.alice, org, "PUT", &one, Some(renamed.clone())).await;
        assert_eq!(s, 200, "{u}");
        assert_eq!(u["version"], 2);
        assert_eq!(u["has_read_secret"], true);

        // A stale version is refused.
        let (s, d) = call(&server, &env.alice, org, "PUT", &one, Some(renamed)).await;
        assert_eq!(s, 409, "{d}");
        assert_eq!(d["error"], "image-registry-version-conflict", "{d}");

        // Dropping the registry drops its dedicated secrets.
        assert_eq!(
            call(&server, &env.alice, org, "DELETE", &one, None).await.0,
            204
        );
        let left = ctx
            .db
            .query_all_raw(Statement::from_string(
                DatabaseBackend::Postgres,
                format!("SELECT 1 AS x FROM org_secrets WHERE name LIKE 'image-registry/{id}/%'"),
            ))
            .await
            .unwrap();
        assert!(left.is_empty(), "dedicated secrets remain");
    })
    .await;
}

/// R1 and R2: another org sees nothing, and a viewer of the org can do nothing.
#[tokio::test]
#[serial]
async fn rights_and_org_isolation() {
    with_app(|server, env, _ctx| async move {
        let a = personal_org(&server, &env.alice).await;
        let b = personal_org(&server, &env.bob).await;
        let list = "/api/v1/image-registries";
        let (_, r) = call(
            &server,
            &env.alice,
            a,
            "POST",
            list,
            Some(registry("agents", "registry.example.org")),
        )
        .await;
        let one = format!("{list}/{}", r["id"].as_str().unwrap());
        let test = format!("{one}/test");

        // Bob in his own org: a registry of Alice's does not exist.
        for (method, path) in [("GET", &one), ("DELETE", &one), ("POST", &test)] {
            let (s, d) = call(&server, &env.bob, b, method, path, None).await;
            assert_eq!(s, 404, "{method} {path}: {d}");
        }
        let (_, mine) = call(&server, &env.bob, b, "GET", list, None).await;
        assert_eq!(mine["count"], 0, "{mine}");

        // Bob as a viewer of Alice's org.
        let res = server
            .post(&format!("/api/v1/orgs/{a}/members"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", a.to_string())
            .json(&json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        for (method, path, body) in [
            ("GET", list.to_string(), None),
            ("POST", list.to_string(), Some(registry("x", "r.example"))),
            ("GET", one.clone(), None),
            ("PUT", one.clone(), Some(registry("x", "r.example"))),
            ("DELETE", one.clone(), None),
            ("POST", test.clone(), None),
        ] {
            let (s, d) = call(&server, &env.bob, a, method, &path, body).await;
            assert_eq!(s, 403, "{method} {path}: {d}");
        }
    })
    .await;
}

/// Against a real registry, for instance a local one:
/// `PNEX_TEST_REGISTRY_HOST=127.0.0.1:5000 PNEX_TEST_REGISTRY_USER=u PNEX_TEST_REGISTRY_SECRET=p
/// cargo test -p pnex-backend --test image_registries -- --ignored`
/// `PNEX_TEST_REGISTRY_TLS=1` for HTTPS.
#[tokio::test]
#[serial]
#[ignore = "needs a real registry, see the comment"]
async fn a_real_registry_answers() {
    with_app(|server, env, _ctx| async move {
        let var = |n: &str| std::env::var(n).unwrap_or_else(|_| panic!("{n} is required"));
        let org = personal_org(&server, &env.alice).await;
        let body = json!({
            "name": "real",
            "host": var("PNEX_TEST_REGISTRY_HOST"),
            "repository": std::env::var("PNEX_TEST_REGISTRY_REPOSITORY")
                .unwrap_or_else(|_| "pnex-test/app".into()),
            "tls": std::env::var("PNEX_TEST_REGISTRY_TLS").is_ok(),
            "read_username": var("PNEX_TEST_REGISTRY_USER"),
            "read_secret": { "value": var("PNEX_TEST_REGISTRY_SECRET") },
            "write_username": var("PNEX_TEST_REGISTRY_USER"),
            "write_secret": { "value": var("PNEX_TEST_REGISTRY_SECRET") },
        });
        let (s, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(body),
        )
        .await;
        assert_eq!(s, 201, "{r}");
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["ok"], true, "{t}");
        assert_eq!(t["write"]["ok"], true, "{t}");

        // The same login with a wrong secret is refused.
        let bad = json!({
            "name": "real-bad",
            "host": var("PNEX_TEST_REGISTRY_HOST"),
            "repository": "pnex-test/app",
            "tls": std::env::var("PNEX_TEST_REGISTRY_TLS").is_ok(),
            "read_username": var("PNEX_TEST_REGISTRY_USER"),
            "read_secret": { "value": "definitely-not-the-secret" },
        });
        let (_, r) = call(
            &server,
            &env.alice,
            org,
            "POST",
            "/api/v1/image-registries",
            Some(bad),
        )
        .await;
        let test = format!(
            "/api/v1/image-registries/{}/test",
            r["id"].as_str().unwrap()
        );
        let (_, t) = call(&server, &env.alice, org, "POST", &test, None).await;
        assert_eq!(t["read"]["code"], "image-registry-auth-failed", "{t}");
    })
    .await;
}
