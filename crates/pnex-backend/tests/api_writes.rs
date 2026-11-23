//! Write routes no other suite reaches: what they change, and that another
//! org never reaches them (R1).
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

async fn org_of(server: &axum_test::TestServer, token: &str) -> i64 {
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
        "PUT" => server.put(path),
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
    (
        res.status_code().as_u16(),
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// A team renamed and deleted, an assignment removed: the rights it gave go
/// with it, and another org reaches neither.
#[tokio::test]
#[serial]
async fn a_team_and_an_assignment_are_changed_and_removed() {
    with_app(|server, env| async move {
        let org = org_of(&server, &env.alice).await;
        let other = org_of(&server, &env.bob).await;
        let (st, team) = call(
            &server,
            "POST",
            "/api/v1/iam/teams",
            &env.alice,
            org,
            Some(json!({"name": "Readers"})),
        )
        .await;
        assert_eq!(st, 201, "{team}");
        let path = format!("/api/v1/iam/teams/{}", team["id"].as_str().unwrap());

        let (st, _) = call(
            &server,
            "PATCH",
            &path,
            &env.bob,
            other,
            Some(json!({"name": "Mine"})),
        )
        .await;
        assert_eq!(st, 404, "another org does not see the team");
        let (st, t) = call(
            &server,
            "PATCH",
            &path,
            &env.alice,
            org,
            Some(json!({"name": "Auditors", "description": "Read only"})),
        )
        .await;
        assert_eq!((st, t["name"].as_str()), (200, Some("Auditors")), "{t}");
        assert_eq!(t["description"], "Read only");

        let (_, role) = call(
            &server,
            "POST",
            "/api/v1/iam/roles",
            &env.alice,
            org,
            Some(json!({
                "key": "no-topics", "name": "No topics",
                "statements": [{"effect": "deny", "permissions": ["taxonomies:write"]}]
            })),
        )
        .await;
        let (st, a) = call(
            &server,
            "POST",
            "/api/v1/iam/assignments",
            &env.alice,
            org,
            Some(json!({"team_id": team["id"], "role_id": role["id"]})),
        )
        .await;
        assert_eq!(st, 201, "{a}");
        let one = format!("/api/v1/iam/assignments/{}", a["id"].as_str().unwrap());
        let (st, _) = call(&server, "DELETE", &one, &env.bob, other, None).await;
        assert_eq!(st, 404, "another org does not see the assignment");
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "DELETE", &one, &env.alice, org, None).await;
        assert_eq!(st, 404, "already gone");

        let (st, _) = call(&server, "DELETE", &path, &env.bob, other, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "DELETE", &path, &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (_, list) = call(&server, "GET", "/api/v1/iam/teams", &env.alice, org, None).await;
        assert_eq!(list["count"], 0, "{list}");
    })
    .await;
}

/// A browser endpoint: its token never read back, an update without one
/// keeps it, a server-side test that cannot connect says so, and one default
/// at a time. Another org reaches none of it.
#[tokio::test]
#[serial]
async fn a_browser_endpoint_keeps_its_token_and_its_org() {
    with_app(|server, env| async move {
        let org = org_of(&server, &env.alice).await;
        let other = org_of(&server, &env.bob).await;
        let base = "/api/v1/browser-endpoints";
        let input = |name: &str, extra: Value| {
            let mut v = json!({"name": name, "cdp_url": "ws://browser.example.invalid:9222"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            v
        };
        let secret = "cdp-token-do-not-leak";
        let (st, e) = call(
            &server,
            "POST",
            base,
            &env.alice,
            org,
            Some(json!({"name": " ", "cdp_url": "not a url"})),
        )
        .await;
        assert_eq!(
            (st, e["error"].as_str()),
            (400, Some("field-invalid")),
            "{e}"
        );

        let (st, one) = call(
            &server,
            "POST",
            base,
            &env.alice,
            org,
            Some(input(
                "Main",
                json!({"is_default": true, "token": {"value": secret}}),
            )),
        )
        .await;
        assert_eq!(st, 201, "{one}");
        assert!(
            !one.to_string().contains(secret),
            "the token is never read back: {one}"
        );
        let path = format!("{base}/{}", one["id"].as_str().unwrap());

        let (st, e) = call(
            &server,
            "POST",
            base,
            &env.alice,
            org,
            Some(input("Main", json!({}))),
        )
        .await;
        assert_eq!(
            (st, e["error"].as_str()),
            (409, Some("browser-endpoint-name-taken")),
            "{e}"
        );

        // One default at a time.
        let (st, two) = call(
            &server,
            "POST",
            base,
            &env.alice,
            org,
            Some(input("Backup", json!({"is_default": true}))),
        )
        .await;
        assert_eq!(st, 201, "{two}");
        let (_, list) = call(&server, "GET", base, &env.alice, org, None).await;
        assert_eq!(list["count"], 2, "{list}");
        let defaults: Vec<&str> = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["is_default"] == true)
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(defaults, ["Backup"], "{list}");
        assert!(!list.to_string().contains(secret));

        // An update without a token keeps it.
        let (st, up) = call(
            &server,
            "PUT",
            &path,
            &env.alice,
            org,
            Some(json!({"name": "Renamed", "cdp_url": "ws://browser.example.invalid:9333"})),
        )
        .await;
        assert_eq!((st, up["name"].as_str()), (200, Some("Renamed")), "{up}");
        assert_eq!(
            up["token"]["secret_id"], one["token"]["secret_id"],
            "the token stays: {up}"
        );

        // The test runs from the server and reports what it could not reach.
        let (st, t) = call(
            &server,
            "POST",
            &format!("{path}/test"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!((st, t["ok"].as_bool()), (200, Some(false)), "{t}");

        // Another org.
        let (_, list) = call(&server, "GET", base, &env.bob, other, None).await;
        assert_eq!(list["count"], 0, "{list}");
        for (method, url, body) in [
            ("PUT", path.clone(), Some(input("Stolen", json!({})))),
            ("POST", format!("{path}/test"), None),
            ("DELETE", path.clone(), None),
        ] {
            let (st, e) = call(&server, method, &url, &env.bob, other, body).await;
            assert_eq!(
                (st, e["error"].as_str()),
                (404, Some("browser-endpoint-not-found")),
                "{method} {e}"
            );
        }

        let (st, _) = call(&server, "DELETE", &path, &env.alice, org, None).await;
        assert!(st < 300, "{st}");
        let (st, _) = call(
            &server,
            "POST",
            &format!("{path}/test"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 404, "gone");
    })
    .await;
}

/// PE5.5: a site rule is an org pipeline. Its check answers the rule, a
/// pipeline that is not one answers none, and the Web sites page counts it.
#[tokio::test]
#[serial]
async fn a_site_rule_is_an_org_pipeline() {
    with_app(|server, env| async move {
        let org = org_of(&server, &env.alice).await;
        let rule = |step: &str| {
            let r = pnex_backend::services::site_recipes::parse(&format!(
                "name: programme\nsite: www.example.org\nparams: [code]\nsteps:\n  - open: \"{step}/{{{{code}}}}\"\n"
            ))
            .unwrap();
            pnex_core::site_recipe::rule_yaml(&r)
        };
        let (st, check) = call(
            &server,
            "POST",
            "/api/v1/pipelines/validate",
            &env.alice,
            org,
            Some(json!({"document": rule("/one")})),
        )
        .await;
        assert_eq!(st, 200, "{check}");
        assert_eq!(check["rule"]["params"], json!(["code"]), "{check}");
        assert!(check["rights"].as_array().unwrap().contains(&json!("browse:use")), "{check}");

        let (st, created) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({"name": "programme", "document": rule("/one")})),
        )
        .await;
        assert_eq!(st, 201, "{created}");
        assert_eq!(created["rule"]["site"], "www.example.org", "{created}");
        let (_, sites) = call(&server, "GET", "/api/v1/web-sites", &env.alice, org, None).await;
        assert_eq!(sites["results"][0]["site"], "example.org", "{sites}");
        assert_eq!(sites["results"][0]["recipes"], 1, "{sites}");
        let id = created["id"].as_str().unwrap();
        let (st, flows) = call(
            &server,
            "GET",
            &format!("/api/v1/pipelines/{id}/flows"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!((st, flows["flows"].clone()), (200, json!([])), "{flows}");
    })
    .await;
}

/// The screen of a device: a rebuild is asked for, an unknown screen is
/// refused, and another org does not see the device.
#[tokio::test]
#[serial]
async fn the_screen_of_a_device_is_chosen_by_its_org() {
    with_app(|server, env| async move {
        let org = org_of(&server, &env.alice).await;
        let other = org_of(&server, &env.bob).await;
        let (st, dev) = call(
            &server,
            "POST",
            "/api/v1/devices",
            &env.alice,
            org,
            Some(json!({
                "device_id": "gen-screen", "predefined_device_name": "generic_esp8266"
            })),
        )
        .await;
        assert_eq!(st, 201, "{dev}");
        let path = format!("/api/v1/devices/{}/peripherals", dev["id"]);

        let (st, _) = call(
            &server,
            "PUT",
            &path,
            &env.bob,
            other,
            Some(json!({"screen": "ssd1306"})),
        )
        .await;
        assert_eq!(st, 404);
        let (st, e) = call(
            &server,
            "PUT",
            &path,
            &env.alice,
            org,
            Some(json!({"screen": "no-such-screen"})),
        )
        .await;
        assert_eq!(st, 400, "{e}");
        let (st, up) = call(
            &server,
            "PUT",
            &path,
            &env.alice,
            org,
            Some(json!({"screen": "ssd1306"})),
        )
        .await;
        assert_eq!(st, 200, "{up}");
        assert_eq!(up["rebuild_required"], true, "{up}");
    })
    .await;
}

/// One default web search provider at a time: naming a new one moves it.
#[tokio::test]
#[serial]
async fn the_default_web_search_provider_moves() {
    with_app(|server, env| async move {
        let org = org_of(&server, &env.alice).await;
        let base = "/api/v1/web-search/providers";
        let input = |name: &str, default: bool| {
            json!({"name": name, "kind": "searxng", "base_url": "https://search.example.org",
                   "is_default": default})
        };
        for (name, default) in [("One", true), ("Two", true)] {
            let (st, p) = call(
                &server,
                "POST",
                base,
                &env.alice,
                org,
                Some(input(name, default)),
            )
            .await;
            assert_eq!(st, 201, "{p}");
        }
        let (_, list) = call(&server, "GET", base, &env.alice, org, None).await;
        let defaults: Vec<&str> = list["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["is_default"] == true)
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(defaults, ["Two"], "{list}");
    })
    .await;
}
