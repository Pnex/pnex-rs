//! Review of a media file (media-review.md D272, D273): spans attributed to
//! persons of the ontology without any voice recognition — a person only,
//! merged spans of one person, optimistic concurrency, viewer refused (R2),
//! org isolation (R1), attributions shown by the transcript read.
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
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, Env { alice, bob }).await;
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
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A WAV header is enough for the library (content checked by its magic).
fn wav() -> Vec<u8> {
    let mut b = b"RIFF\x24\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\0\0\0\0".to_vec();
    b.resize(64, 0);
    b
}

async fn audio(server: &axum_test::TestServer, token: &str, org: i64) -> String {
    let res = server
        .post("/api/v1/media?filename=debate.wav&kind=audio")
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(wav().into())
        .await;
    assert!(res.status_code().is_success(), "{}", res.text());
    res.json::<Value>()["id"].as_str().unwrap().to_string()
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
) -> String {
    let (st, o) = call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": {}})),
    )
    .await;
    assert_eq!(st, 201, "{o}");
    o["id"].as_str().unwrap().to_string()
}

fn spans(list: &[(i64, i64)]) -> Value {
    list.iter()
        .map(|(s, e)| json!({"start_ms": s, "end_ms": e}))
        .collect()
}

#[tokio::test]
#[serial]
async fn spans_are_attributed_to_persons_without_voice() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/investigation/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(st < 300, "{p}");
        let media = audio(&server, &env.alice, org).await;
        let alice_p = object(&server, &env.alice, org, "person", "Alice Martin").await;
        let bob_p = object(&server, &env.alice, org, "person", "Bob Durand").await;
        let place = object(&server, &env.alice, org, "place", "Studio").await;
        let path = format!("/api/v1/media/{media}/attributions");

        // Only a person of the org, only sound spans.
        let (st, e) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": place, "spans": spans(&[(0, 1_000)])})),
        )
        .await;
        assert_eq!(st, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["object_id"], "invalid");
        for bad in [vec![], vec![(2_000, 1_000)], vec![(-1, 10)]] {
            let (st, e) = call(
                &server,
                "POST",
                &path,
                &env.alice,
                org,
                Some(json!({"object_id": alice_p, "spans": spans(&bad)})),
            )
            .await;
            assert_eq!(st, 400, "{bad:?}: {e}");
            assert_eq!(e["errors"]["fields"]["spans"], "invalid");
        }

        // "From 12:30 to 14:10, this is Alice"; then S3 = Bob, two turns.
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": alice_p, "spans": spans(&[(750_000, 850_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 1);
        let a = &all["results"][0];
        assert_eq!(a["person"], "Alice Martin");
        assert_eq!(a["origin"], "manual");
        assert_eq!(a["status"], "confirmed");
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": bob_p, "spans": spans(&[(0, 5_000), (9_000, 12_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 3);
        // A span of Alice touching hers joins it: one continuous span.
        let (st, all) = call(
            &server,
            "POST",
            &path,
            &env.alice,
            org,
            Some(json!({"object_id": alice_p, "spans": spans(&[(840_000, 900_000)])})),
        )
        .await;
        assert_eq!(st, 201, "{all}");
        assert_eq!(all["count"], 3, "{all}");
        let mine: Vec<&Value> = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["person"] == "Alice Martin")
            .collect();
        assert_eq!(
            (mine[0]["start_ms"].as_i64(), mine[0]["end_ms"].as_i64()),
            (Some(750_000), Some(900_000))
        );

        // The ontology knows who speaks in the media (D278): one open
        // `appears_in` per person, never one per span.
        let open_appearances = |person: String| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                let (st, links) = call(
                    server,
                    "GET",
                    &format!("/api/v1/ontology/objects/{person}/links"),
                    &token,
                    org,
                    None,
                )
                .await;
                assert_eq!(st, 200, "{links}");
                links["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|l| l["link_type"] == "appears_in" && l["valid_to"].is_null())
                    .count()
            }
        };
        assert_eq!(open_appearances(alice_p.clone()).await, 1);
        assert_eq!(open_appearances(bob_p.clone()).await, 1);

        // The transcript read carries them (no run yet: no text).
        let (st, t) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{media}/transcript"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{t}");
        assert_eq!(t["attributions"].as_array().unwrap().len(), 3);
        assert!(t["runs"].as_array().unwrap().is_empty());

        // Optimistic concurrency, then reject and delete.
        let aid = mine[0]["id"].as_str().unwrap().to_string();
        let one = format!("{path}/{aid}");
        let (st, a) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 1, "end_ms": 880_000})),
        )
        .await;
        assert_eq!(st, 200, "{a}");
        assert_eq!(a["version"], 2);
        let (st, e) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 1, "status": "rejected"})),
        )
        .await;
        assert_eq!(st, 409, "{e}");
        assert_eq!(e["error"], "media-attribution-conflict");
        let (st, a) = call(
            &server,
            "PATCH",
            &one,
            &env.alice,
            org,
            Some(json!({"expected_version": 2, "status": "rejected"})),
        )
        .await;
        assert_eq!((st, a["status"].as_str()), (200, Some("rejected")), "{a}");
        assert_eq!(
            open_appearances(alice_p.clone()).await,
            0,
            "no confirmed span left"
        );

        // A viewer reads, never writes (R2); another org sees nothing (R1).
        let bob_org = personal_org(&server, &env.bob).await;
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({"email": "bob@example.com", "role": "viewer"})),
        )
        .await;
        assert!(st < 300);
        let (st, list) = call(&server, "GET", &path, &env.bob, org, None).await;
        assert_eq!((st, list["count"].as_i64()), (200, Some(3)), "{list}");
        let (st, e) = call(
            &server,
            "POST",
            &path,
            &env.bob,
            org,
            Some(json!({"object_id": bob_p, "spans": spans(&[(0, 1)])})),
        )
        .await;
        assert_eq!(st, 403, "{e}");
        assert_eq!(e["error"], "media-write-forbidden");
        let (st, _) = call(&server, "DELETE", &one, &env.bob, org, None).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "GET", &path, &env.bob, bob_org, None).await;
        assert_eq!(st, 404, "an asset of another org");
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 404);
    })
    .await;
}
