//! MCP server (D245). Guards of the exposure table: every operation of the
//! spec is exposed or excluded with a reason, exactly once; nothing exposed
//! deletes, acts physically or speaks anything but JSON. Then a live run:
//! handshake, listing, calls with the caller's rights, secrets masked.
//!
//! The live test needs PostgreSQL (TEST_DATABASE_URL).

mod common;

use std::collections::BTreeMap;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::mcp::exposure::{EXCLUDED, EXPOSED};
use serde_json::{json, Value};
use serial_test::serial;

/// operationId → (method, operation) of the generated spec.
fn operations() -> BTreeMap<String, (String, Value)> {
    let spec: Value = serde_json::from_str(&pnex_backend::openapi::spec_json()).unwrap();
    let mut ops = BTreeMap::new();
    for (_, item) in spec["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str() {
                ops.insert(id.to_string(), (method.clone(), op.clone()));
            }
        }
    }
    ops
}

#[test]
fn every_operation_is_exposed_or_excluded_once() {
    let ops = operations();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    for id in EXPOSED
        .iter()
        .chain(EXCLUDED.iter().flat_map(|(_, ids)| ids.iter()))
    {
        *seen.entry(id).or_default() += 1;
    }
    let missing: Vec<_> = ops
        .keys()
        .filter(|id| !seen.contains_key(id.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "operations neither exposed nor excluded in pnex-api-contract/src/exposure.rs \
         (add each to EXPOSED, or to EXCLUDED under its reason): {missing:?}"
    );
    let stale: Vec<_> = seen.keys().filter(|id| !ops.contains_key(**id)).collect();
    assert!(stale.is_empty(), "not in the spec any more: {stale:?}");
    let twice: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
    assert!(twice.is_empty(), "listed more than once: {twice:?}");
    for (reason, ids) in EXCLUDED {
        assert!(
            reason.contains(": ") && !ids.is_empty(),
            "bad exclusion `{reason}`"
        );
    }
}

#[test]
fn exposed_operations_are_safe_and_json() {
    let ops = operations();
    // Words of the writes the MCP never runs (D123, D144, D245).
    let forbidden = [
        "delete",
        "purge",
        "archive",
        "ota",
        "deploy",
        "stop",
        "restart",
        "rollback",
        "commands",
        "flash",
        "enroll",
        "enrollment",
        "secrets",
        "approve",
        "value",
    ];
    for id in EXPOSED {
        let (method, op) = &ops[*id];
        assert_ne!(method, "delete", "{id}: a DELETE is never exposed");
        assert!(
            method == "get" || !id.split('_').any(|w| forbidden.contains(&w)),
            "{id}: looks destructive or physical, exclude it with its reason"
        );
        let json_only = |content: &Value| {
            content
                .as_object()
                .is_none_or(|c| c.keys().all(|k| k == "application/json"))
        };
        assert!(
            json_only(&op["requestBody"]["content"]),
            "{id}: non-JSON request"
        );
        for (status, res) in op["responses"].as_object().unwrap() {
            if status.starts_with('2') {
                assert!(json_only(&res["content"]), "{id}: non-JSON answer");
            }
        }
    }
}

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

async fn rpc(server: &axum_test::TestServer, token: &str, org: Option<i64>, msg: Value) -> Value {
    let mut req = server
        .post("/api/v1/mcp")
        .add_header("Authorization", format!("Bearer {token}"));
    if let Some(org) = org {
        req = req.add_header("X-Org-Id", org.to_string());
    }
    let res = req.json(&msg).await;
    assert_eq!(res.status_code(), 200, "{}", res.text());
    res.json()
}

async fn call(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
    args: Value,
) -> Value {
    let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                     "params": {"name": name, "arguments": args}});
    rpc(server, token, Some(org), msg).await["result"].clone()
}

fn text(result: &Value) -> Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null)
}

#[tokio::test]
#[serial]
async fn mcp_serves_tools_with_the_callers_rights() {
    with_app(|server, env| async move {
        // No token: 401 before any JSON-RPC.
        let res = server.post("/api/v1/mcp").json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).await;
        assert_eq!(res.status_code(), 401);

        // OAuth discovery probes these before the issuer's own URL: a 404,
        // never the SPA's index.html (a 200 that is not JSON aborts the client).
        for probe in [
            "/.well-known/oauth-authorization-server/auth/v1",
            "/.well-known/openid-configuration/auth/v1",
            "/.well-known/oauth-protected-resource",
        ] {
            assert_eq!(server.get(probe).await.status_code(), 404, "{probe}");
        }

        let init = rpc(&server, &env.alice, None, json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}})).await;
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["capabilities"]["tools"].is_object());

        let note = server
            .post("/api/v1/mcp")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        assert_eq!(note.status_code(), 202);

        let list = rpc(&server, &env.alice, None, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), EXPOSED.len());
        let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"viz_widgets_create"));
        assert!(!names.contains(&"devices_commands") && !names.contains(&"flows_deploy"));

        // The org comes from user_info_get, like any client would.
        let me = call(&server, &env.alice, 0, "user_info_get", json!({})).await;
        let org = text(&me)["orgs"][0]["id"].as_i64().unwrap();

        // A write through the tool, then a read, with the connection's org.
        let created = call(&server, &env.alice, org, "viz_widgets_create", json!({"body": {
            "name": "Gauge", "kind": "gauge", "config": widget_config()}})).await;
        assert_eq!(created["isError"], false, "{created}");
        let listed = call(&server, &env.alice, org, "viz_widgets_list", json!({"kind": "gauge"})).await;
        assert_eq!(text(&listed)["count"], 1);

        // Handler errors come back as a tool error carrying the ErrorDetail.
        let bad = call(&server, &env.alice, org, "viz_widgets_create", json!({"body": {
            "name": "Gauge", "kind": "gauge", "config": widget_config()}})).await;
        assert_eq!(bad["isError"], true);
        assert!(bad["content"][0]["text"].as_str().unwrap().starts_with("HTTP 400"));

        // Bob, not a member: the handler's own 403, never a bypass.
        let denied = call(&server, &env.bob, org, "viz_widgets_list", json!({})).await;
        assert_eq!(denied["isError"], true);
        assert!(denied["content"][0]["text"].as_str().unwrap().contains("org-not-member"));

        // Secrets never reach the model.
        server
            .post("/api/v1/devices")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({"device_id": "esp-001", "predefined_device_name": "temp_sensor"}))
            .await;
        let devices = call(&server, &env.alice, org, "devices_list", json!({})).await;
        let devices = text(&devices);
        let device = &devices["results"][0];
        assert_eq!(device["device_id"], "esp-001", "{devices}");
        assert!(device["device_token"].is_null() || device["device_token"] == "[redacted]", "{device}");

        // Unknown or excluded tool: a JSON-RPC error, nothing runs.
        let msg = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                         "params": {"name": "devices_delete", "arguments": {"id": 1}}});
        let res = rpc(&server, &env.alice, Some(org), msg).await;
        assert_eq!(res["error"]["code"], -32602);
    })
    .await;
}

fn widget_config() -> Value {
    json!({"type": "gauge", "title": "T",
           "source": [{"role": "primary", "metric": "m", "device_id": "d", "window": "1h"}],
           "options": {"unit": "C", "min": 0.0, "max": 50.0, "decimals": 1}})
}
