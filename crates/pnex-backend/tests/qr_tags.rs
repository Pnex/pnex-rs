//! QR tags (parts-docs.md D962–D963): blank sheets linked on site, the tag
//! of an object, resolution inside the caller's org only, revocation, and
//! the permissions (R1, R2).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

struct Who {
    token: String,
    org: i64,
}

async fn call(
    server: &axum_test::TestServer,
    who: &Who,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {}", who.token))
    .add_header("X-Org-Id", who.org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let st = res.status_code().as_u16();
    let text = res.text();
    (
        st,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

#[tokio::test]
#[serial]
async fn qr_tags_open_their_object_in_their_org_only() {
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
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
    loco_rs::testing::request::request_with_config::<App, _, _>(config, move |server, ctx| async move {
        common::seed_catalogue(&ctx.db).await;
        let org_of = |token: String| {
            let server = &server;
            async move {
                server
                    .get("/api/v1/user-info")
                    .add_header("Authorization", format!("Bearer {token}"))
                    .await
                    .json::<Value>()["orgs"][0]["id"]
                    .as_i64()
                    .unwrap()
            }
        };
        let a = Who { org: org_of(alice.clone()).await, token: alice.clone() };
        let b = Who { org: org_of(bob.clone()).await, token: bob.clone() };
        let (st, r) = call(&server, &a, "POST", "/api/v1/ontology/packs/maintenance/install", None).await;
        assert_eq!(st, 200, "{r}");
        let (_, pump) = call(&server, &a, "POST", "/api/v1/ontology/objects", Some(json!({
            "type_key": "pump", "title": "Pump P1", "properties": { "serial": "SN-1" }
        }))).await;
        let pump = pump["id"].as_str().unwrap().to_string();

        // A blank sheet, printed ahead.
        let (st, sheet) = call(&server, &a, "POST", "/api/v1/qr-tags/sheets", Some(json!({ "count": 3 }))).await;
        assert_eq!(st, 201, "{sheet}");
        let tags = sheet["tags"].as_array().unwrap();
        assert!(sheet["sheet_id"].as_str().is_some_and(|s| s.len() == 36), "{sheet}");
        assert_eq!(tags.len(), 3);
        assert!(tags.iter().all(|t| t["state"] == "blank" && t["token"].as_str().unwrap().len() == 26));
        let token = tags[0]["token"].as_str().unwrap().to_string();
        let (st, _) = call(&server, &a, "POST", "/api/v1/qr-tags/sheets", Some(json!({ "count": 121 }))).await;
        assert_eq!(st, 400);

        // Scanned on site and linked to the pump; it then opens it.
        let (st, t) = call(&server, &a, "POST", &format!("/api/v1/qr-tags/{token}/link"), Some(json!({ "object_id": pump }))).await;
        assert_eq!(st, 200, "{t}");
        let (_, t) = call(&server, &a, "GET", &format!("/api/v1/qr-tags/{token}"), None).await;
        assert_eq!((t["state"].as_str(), t["object_id"].as_str(), t["object_title"].as_str()), (Some("linked"), Some(pump.as_str()), Some("Pump P1")), "{t}");
        let (st, e) = call(&server, &a, "POST", &format!("/api/v1/qr-tags/{token}/link"), Some(json!({ "object_id": pump }))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("qr-tag-not-blank")), "{e}");

        // The object's tag is the one linked; another object gets a new one.
        let (st, own) = call(&server, &a, "POST", &format!("/api/v1/qr-tags/objects/{pump}"), None).await;
        assert_eq!((st, own["token"].as_str()), (200, Some(token.as_str())), "{own}");
        let (_, other) = call(&server, &a, "POST", "/api/v1/ontology/objects", Some(json!({
            "type_key": "pump", "title": "Pump P2", "properties": { "serial": "SN-2" }
        }))).await;
        let (st, made) = call(&server, &a, "POST", &format!("/api/v1/qr-tags/objects/{}", other["id"].as_str().unwrap()), None).await;
        assert_eq!((st, made["state"].as_str()), (201, Some("linked")), "{made}");

        // Another org neither resolves, links nor revokes it (R1).
        let (st, _) = call(&server, &b, "GET", &format!("/api/v1/qr-tags/{token}"), None).await;
        assert_eq!(st, 404);
        let blank = tags[1]["token"].as_str().unwrap();
        let (st, _) = call(&server, &b, "POST", &format!("/api/v1/qr-tags/{blank}/link"), Some(json!({ "object_id": pump }))).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, &a, "GET", "/api/v1/qr-tags/not-a-token", None).await;
        assert_eq!(st, 404);

        // Revoked: it no longer opens the pump.
        let (st, t) = call(&server, &a, "POST", &format!("/api/v1/qr-tags/{token}/revoke"), None).await;
        assert_eq!((st, t["state"].as_str()), (200, Some("revoked")), "{t}");
        let (_, l) = call(&server, &a, "GET", "/api/v1/qr-tags?state=blank", None).await;
        assert_eq!(l["count"], 2, "{l}");
        let (_, l) = call(&server, &a, "GET", &format!("/api/v1/qr-tags?object_id={pump}"), None).await;
        assert_eq!(l["count"], 1, "{l}");

        // R2: qr-tags:read reads, never writes.
        let (_, sa) = call(&server, &a, "POST", "/api/v1/iam/service-accounts", Some(json!({ "name": "scanner" }))).await;
        let (_, role) = call(&server, &a, "POST", "/api/v1/iam/roles", Some(json!({
            "key": "scanner", "name": "Scanner", "statements": [{ "effect": "allow", "permissions": ["qr-tags:read"] }]
        }))).await;
        call(&server, &a, "POST", "/api/v1/iam/assignments", Some(json!({ "service_account_id": sa["id"], "role_id": role["id"] }))).await;
        let exp = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
        let (_, key) = call(&server, &a, "POST", &format!("/api/v1/iam/service-accounts/{}/keys", sa["id"].as_str().unwrap()), Some(json!({ "expires_at": exp }))).await;
        let secret = key["secret"].as_str().unwrap();
        let res = server.get(&format!("/api/v1/qr-tags/{blank}")).add_header("Authorization", format!("Bearer {secret}")).await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let res = server
            .post(&format!("/api/v1/qr-tags/{blank}/link"))
            .add_header("Authorization", format!("Bearer {secret}"))
            .json(&json!({ "object_id": pump }))
            .await;
        assert_eq!(res.status_code().as_u16(), 403, "{}", res.text());
        assert!(res.text().contains("qr-tags:write"), "{}", res.text());
    })
    .await;
}
