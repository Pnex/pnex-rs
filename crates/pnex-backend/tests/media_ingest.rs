//! Media ingest API (media-ingest.md D159, D164): stream CRUD, frozen
//! slug, quota, URL rules, secret bound to the URL origin (R9), org
//! isolation (R1) and viewer read-only (R2). Egress refusals are unit
//! tested in `services::media_ingest::streams` (tests run with `open`).
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

fn stream(name: &str, url: &str) -> Value {
    json!({ "name": name, "kind": "icecast", "url": url })
}

#[tokio::test]
#[serial]
async fn stream_crud_slug_and_quota() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let url = "https://icecast.radio.example/inter.mp3";

        let (s, a) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("France Inter", url)),
        )
        .await;
        assert_eq!(s, 201, "{a}");
        assert_eq!(a["slug"], "france_inter");
        assert_eq!(a["enabled"], false);
        assert_eq!(a["capture_state"], "stopped");
        assert_eq!(a["has_secret"], false);
        let id = a["id"].as_str().unwrap().to_string();

        // Same name → next free slug; renaming never changes the slug.
        let (s, b) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("France Inter", url)),
        )
        .await;
        assert_eq!(s, 201);
        assert_eq!(b["slug"], "france_inter_2");
        let (s, renamed) = call(
            &server,
            "PATCH",
            &format!("/api/v1/media/streams/{id}"),
            &env.alice,
            org,
            Some(json!({ "name": "Inter" })),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(renamed["name"], "Inter");
        assert_eq!(renamed["slug"], "france_inter");

        // Default quota: 3 streams per org.
        let (s, _) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("Culture", url)),
        )
        .await;
        assert_eq!(s, 201);
        let (s, over) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("Musique", url)),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(over["error"], "media-stream-limit");

        let (s, list) = call(
            &server,
            "GET",
            "/api/v1/media/streams",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(list["count"], 3);

        // Bounds and enum fields.
        for body in [
            json!({ "segment_secs": 5 }),
            json!({ "overlap_secs": 4 }),
            json!({ "audio_retention": "days:0" }),
            json!({ "timezone": "Europe/Paris; x" }),
            json!({ "capture_on": "gpu" }),
        ] {
            let (s, _) = call(
                &server,
                "PATCH",
                &format!("/api/v1/media/streams/{id}"),
                &env.alice,
                org,
                Some(body.clone()),
            )
            .await;
            assert_eq!(s, 400, "{body}");
        }
        let (s, e) = call(
            &server,
            "PATCH",
            &format!("/api/v1/media/streams/{id}"),
            &env.alice,
            org,
            Some(json!({ "capture_on": "worker" })),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["error"], "media-capture-unsupported");

        // Enabling needs a profile with a valid ASR model.
        let (s, e) = call(
            &server,
            "PATCH",
            &format!("/api/v1/media/streams/{id}"),
            &env.alice,
            org,
            Some(json!({ "enabled": true })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "media-asr-model-invalid");

        // Segments of a fresh stream: empty page.
        let (s, segs) = call(
            &server,
            "GET",
            &format!("/api/v1/media/streams/{id}/segments"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(segs["count"], 0);

        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/media/streams/{id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 204);
        let (s, _) = call(
            &server,
            "GET",
            &format!("/api/v1/media/streams/{id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 404);

        // The deleted stream is a tombstone: it frees its quota slot, but
        // its slug (the name of its O2 streams) is never given again.
        let (s, again) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("France Inter", url)),
        )
        .await;
        assert_eq!(s, 201, "{again}");
        assert_eq!(again["slug"], "france_inter_3");
        let (_, list) = call(
            &server,
            "GET",
            "/api/v1/media/streams",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(list["count"], 3);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn url_rules() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        for (url, code) in [
            (
                "https://u:p@radio.example/live.mp3",
                "media-url-has-credentials",
            ),
            (
                "https://radio.example/live.m3u8?token=abc",
                "media-url-has-credentials",
            ),
        ] {
            let (s, e) = call(
                &server,
                "POST",
                "/api/v1/media/streams",
                &env.alice,
                org,
                Some(stream("X", url)),
            )
            .await;
            assert_eq!(s, 400, "{url}");
            assert_eq!(e["error"], code, "{url}");
        }
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(stream("X", "file:///etc/passwd")),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["url"], "invalid");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn secret_stays_bound_to_the_url_origin() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let _ = personal_org(&server, &env.bob).await;
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({ "email": "bob@example.com", "role": "member" })),
        )
        .await;
        assert!(s == 200 || s == 201, "{s}");

        let mut body = stream("Privée", "https://radio.example/private.mp3");
        body["auth_secret"] = json!({ "value": "Bearer s3cr3t" });
        let (s, a) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org,
            Some(body),
        )
        .await;
        assert_eq!(s, 201, "{a}");
        assert_eq!(a["has_secret"], true);
        assert!(!a.to_string().contains("s3cr3t"), "no secret value in DTOs");
        let id = a["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/media/streams/{id}");

        // A member keeps the origin: same host, other path is fine…
        let (s, _) = call(
            &server,
            "PATCH",
            &path,
            &env.bob,
            org,
            Some(json!({ "url": "https://radio.example/other.mp3" })),
        )
        .await;
        assert_eq!(s, 200);
        // …another host or scheme is not.
        for url in [
            "https://evil.example/x.mp3",
            "http://radio.example/private.mp3",
        ] {
            let (s, e) = call(
                &server,
                "PATCH",
                &path,
                &env.bob,
                org,
                Some(json!({ "url": url })),
            )
            .await;
            assert_eq!(s, 403, "{url}");
            assert_eq!(e["error"], "secret-destination-locked");
        }
        // The owner may move it.
        let (s, _) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(json!({ "url": "https://cdn.radio.example/private.mp3" })),
        )
        .await;
        assert_eq!(s, 200);
        // Clearing the secret is free.
        let (s, cleared) = call(
            &server,
            "PATCH",
            &path,
            &env.bob,
            org,
            Some(json!({ "clear_secret": true })),
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(cleared["has_secret"], false);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_viewer_read_only() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, a) = call(
            &server,
            "POST",
            "/api/v1/media/streams",
            &env.alice,
            org_a,
            Some(stream("Inter", "https://radio.example/a.mp3")),
        )
        .await;
        let id = a["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/media/streams/{id}");

        // Bob on his own org: empty list, hidden 404 on the stream and its
        // segments, no write.
        let (_, list) = call(
            &server,
            "GET",
            "/api/v1/media/streams",
            &env.bob,
            org_b,
            None,
        )
        .await;
        assert_eq!(list["count"], 0);
        for (m, p) in [
            ("GET", path.clone()),
            ("PATCH", path.clone()),
            ("DELETE", path.clone()),
            ("GET", format!("{path}/segments")),
        ] {
            let body = (m == "PATCH").then(|| json!({ "name": "x" }));
            let (s, _) = call(&server, m, &p, &env.bob, org_b, body).await;
            assert_eq!(s, 404, "{m} {p}");
        }

        // A profile cannot point at a model of another org (or no model).
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/asr/profiles",
            &env.alice,
            org_a,
            Some(json!({ "name": "P", "asr_model_id": "00000000-0000-0000-0000-000000000001" })),
        )
        .await;
        assert_eq!(s, 400);
        assert_eq!(e["asr_model_id"], "invalid");

        // Viewer: reads, never writes.
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
        for (m, p, body) in [
            (
                "POST",
                "/api/v1/media/streams".to_string(),
                Some(stream("V", "https://radio.example/v.mp3")),
            ),
            ("PATCH", path.clone(), Some(json!({ "name": "V" }))),
            ("DELETE", path.clone(), None),
            (
                "POST",
                "/api/v1/asr/profiles".to_string(),
                Some(json!({ "name": "V" })),
            ),
        ] {
            let (s, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(s, 403, "{m} {p}");
            assert_eq!(e["error"], "media-stream-write-forbidden");
        }
    })
    .await;
}
