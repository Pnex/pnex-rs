//! IAM action registry (iam.md D289). Guards of
//! `pnex-api-contract/src/iam.rs`: every operation of the spec and every
//! assistant tool is listed exactly once, every permission they name
//! exists and is used (by an operation, a tool or a handler gate), and no
//! write hides behind a `read` verb.

use std::collections::{BTreeMap, BTreeSet};

use pnex_api_contract::iam::{Access, GATES, OPERATIONS, PERMISSIONS, TOOLS};
use serde_json::Value;

/// operationId → (method, path) of the generated spec.
fn operations() -> BTreeMap<String, (String, String)> {
    let spec: Value = serde_json::from_str(&pnex_backend::openapi::spec_json()).unwrap();
    let mut ops = BTreeMap::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if let Some(id) = op["operationId"].as_str() {
                ops.insert(id.to_string(), (method.clone(), path.clone()));
            }
        }
    }
    ops
}

fn once<'a>(names: impl Iterator<Item = &'a str>) -> BTreeSet<&'a str> {
    let mut seen = BTreeSet::new();
    let twice: Vec<_> = names.filter(|n| !seen.insert(*n)).collect();
    assert!(twice.is_empty(), "listed more than once: {twice:?}");
    seen
}

#[test]
fn every_operation_is_registered_once() {
    let ops = operations();
    let listed = once(OPERATIONS.iter().map(|(id, _)| *id));
    let missing: Vec<_> = ops
        .keys()
        .filter(|id| !listed.contains(id.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "operations missing from pnex-api-contract/src/iam.rs (give each its \
         permission, or the principal kind that calls it): {missing:?}"
    );
    let stale: Vec<_> = listed.iter().filter(|id| !ops.contains_key(**id)).collect();
    assert!(stale.is_empty(), "not in the spec any more: {stale:?}");
}

#[test]
fn every_assistant_tool_is_registered_once() {
    // The dispatch table of the assistant is the source of truth.
    let src = include_str!("../src/services/ai/tools.rs");
    let start = src.find("pub async fn execute(").expect("execute()");
    let end = start + src[start..].find("\n}\n").expect("end of execute()");
    let dispatched: BTreeSet<&str> = src[start..end]
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split_once("\" =>"))
        .map(|(name, _)| name)
        .collect();
    let listed = once(TOOLS.iter().map(|(t, _)| *t));
    assert_eq!(
        listed, dispatched,
        "pnex-api-contract/src/iam.rs TOOLS must list exactly the tools of services/ai/tools.rs::execute"
    );
}

#[test]
fn permissions_exist_and_are_used() {
    let known = once(PERMISSIONS.iter().map(|(p, _)| *p));
    let used: BTreeSet<&str> = OPERATIONS
        .iter()
        .filter_map(|(_, a)| match a {
            Access::Org(p) => Some(*p),
            _ => None,
        })
        .chain(TOOLS.iter().filter_map(|(_, p)| *p))
        .chain(GATES.iter().map(|(p, _)| *p))
        .collect();
    let unknown: Vec<_> = used.difference(&known).collect();
    assert!(
        unknown.is_empty(),
        "permissions not in PERMISSIONS: {unknown:?}"
    );
    let unused: Vec<_> = known.difference(&used).collect();
    assert!(unused.is_empty(), "permissions nothing needs: {unused:?}");
    for p in &known {
        let (service, verb) = p.split_once(':').expect("service:verb");
        assert!(
            !service.is_empty()
                && service
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c == '.')
                && verb.chars().all(|c| c.is_ascii_lowercase()),
            "malformed permission `{p}`"
        );
    }
}

#[test]
fn no_write_hides_behind_a_read_verb() {
    let ops = operations();
    // Reads sent as POST: complex queries (`…/query`) and dry runs that
    // store nothing (tests, validation, previews).
    let read_post = |id: &str, path: &str| {
        path.ends_with("/query")
            || [
                "_test",
                "_validate",
                "_preview",
                "_check",
                "_expand",
                "_test_image",
                "_test_live",
            ]
            .iter()
            .any(|s| id.ends_with(s))
    };
    for (id, access) in OPERATIONS {
        let Access::Org(permission) = access else {
            continue;
        };
        let (method, path) = &ops[*id];
        let verb = permission.split_once(':').unwrap().1;
        if verb == "read" {
            assert!(
                method == "get" || read_post(id, path),
                "{id}: a {method} cannot need `{permission}`"
            );
        }
        if method == "delete" {
            assert!(
                matches!(verb, "delete" | "operate"),
                "{id}: a DELETE needs a `delete` (or `operate`) permission, not `{permission}`"
            );
        }
    }
}

#[test]
fn iam_routes_never_reach_the_mcp() {
    // iam.md §7/§14: neither the MCP nor the assistant manages access.
    use pnex_backend::services::mcp::exposure::EXPOSED;
    for id in EXPOSED {
        assert!(
            !id.starts_with("iam_"),
            "{id}: IAM operations are never exposed to the MCP"
        );
    }
    for (tool, permission) in TOOLS {
        assert!(
            !permission.is_some_and(|p| p.starts_with("iam")),
            "{tool}: the assistant never holds an IAM permission"
        );
    }
}

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Alice owns a fresh org where Bob is a viewer: both read their rights,
/// only the owner reads the catalogue (viewer → 403 `access-denied`).
#[tokio::test]
#[serial]
async fn me_permissions_follow_the_managed_role() {
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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
        move |server, _ctx| async move {
            // Bob signs in once so he can be added by email.
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(&bob))
                .await;
            let org = server
                .post("/api/v1/orgs")
                .add_header("Authorization", bearer(&alice))
                .json(&serde_json::json!({ "name": "Atelier IAM" }))
                .await
                .json::<Value>()["id"]
                .as_i64()
                .unwrap();
            let res = server
                .post(&format!("/api/v1/orgs/{org}/members"))
                .add_header("Authorization", bearer(&alice))
                .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
                .await;
            assert!(res.status_code().is_success(), "{}", res.text());

            let me = |token: String| {
                let server = &server;
                async move {
                    let res = server
                        .get("/api/v1/iam/me/permissions")
                        .add_header("Authorization", bearer(&token))
                        .add_header("X-Org-Id", org.to_string())
                        .await;
                    assert_eq!(res.status_code(), 200, "{}", res.text());
                    res.json::<Value>()
                }
            };
            let has =
                |v: &Value, p: &str| v["permissions"].as_array().unwrap().iter().any(|x| x == p);
            let owner = me(alice.clone()).await;
            assert_eq!(owner["role"], "owner");
            assert_eq!(
                owner["permissions"].as_array().unwrap().len(),
                PERMISSIONS.len()
            );
            let viewer = me(bob.clone()).await;
            assert_eq!(viewer["role"], "viewer");
            assert!(has(&viewer, "flows:read"));
            assert!(!has(&viewer, "flows:write"));
            assert!(!has(&viewer, "iam:read"));

            let res = server
                .get("/api/v1/iam/permissions?limit=100")
                .add_header("Authorization", bearer(&bob))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 403);
            let body = res.json::<Value>();
            assert_eq!(body["error"], "access-denied");
            assert_eq!(body["errors"]["args"]["permission"], "iam:read");

            let res = server
                .get("/api/v1/iam/permissions?search=flows:")
                .add_header("Authorization", bearer(&alice))
                .add_header("X-Org-Id", org.to_string())
                .await;
            assert_eq!(res.status_code(), 200, "{}", res.text());
            let page = res.json::<Value>();
            let flows = PERMISSIONS
                .iter()
                .filter(|(p, _)| p.starts_with("flows:"))
                .count();
            assert_eq!(page["count"].as_u64().unwrap() as usize, flows);
            let read = page["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["key"] == "flows:read")
                .expect("flows:read listed");
            assert_eq!(read["level"], "viewer");
        },
    )
    .await;
}

#[test]
fn every_checked_permission_exists() {
    // `org.can("…")` (backend) and `access::can("…")` / `role_can(_, "…")`
    // (front) must name a permission of the registry.
    let known: BTreeSet<&str> = PERMISSIONS.iter().map(|(p, _)| *p).collect();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let re = |src: &str| -> Vec<String> {
        let mut out = Vec::new();
        for (i, _) in src.match_indices("can(") {
            let tail = &src[i + 4..];
            let tail = tail
                .split_once(", ")
                .filter(|(head, _)| !head.contains('"'))
                .map_or(tail, |(_, t)| t);
            if let Some(rest) = tail.strip_prefix('"') {
                if let Some((lit, _)) = rest.split_once('"') {
                    if lit.contains(':') {
                        out.push(lit.to_string());
                    }
                }
            }
        }
        out
    };
    let mut stack = vec![
        root.join("pnex-backend/src"),
        root.join("pnex-frontend/src"),
    ];
    let mut checked = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).unwrap();
                // Unit tests name unknown permissions on purpose.
                let src = src.split("#[cfg(test)]").next().unwrap_or_default();
                for p in re(src) {
                    assert!(
                        known.contains(p.as_str()),
                        "{}: unknown permission `{p}`",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked > 40,
        "the scan found only {checked} permission checks"
    );
}

/// JSON call with the caller's token and org; returns (status, body).
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
        other => panic!("{other}"),
    }
    .add_header("Authorization", bearer(token))
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

/// Lot I3: a custom role adds to the membership role, a deny through a
/// team takes away, nobody gives what they do not hold, access management
/// stays with owners and admins, and a member who leaves leaves their roles.
#[tokio::test]
#[serial]
async fn custom_roles_teams_and_assignments() {
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let token = |n: &str, c: char| {
        common::valid_token(
            &base,
            &format!("00000000-0000-0000-0000-00000000000{c}"),
            n,
            &format!("{n}@example.com"),
        )
    };
    let (alice, bob, carol) = (token("alice", 'a'), token("bob", 'b'), token("carol", 'c'));
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, _ctx| async move {
            let mut ids = BTreeMap::new();
            for (name, t) in [("bob", &bob), ("carol", &carol)] {
                let me = server
                    .get("/api/v1/user-info")
                    .add_header("Authorization", bearer(t))
                    .await
                    .json::<Value>();
                ids.insert(name, me["id"].as_i64().expect("user id"));
            }
            let org = server
                .post("/api/v1/orgs")
                .add_header("Authorization", bearer(&alice))
                .json(&serde_json::json!({ "name": "Atelier IAM 3" }))
                .await
                .json::<Value>()["id"]
                .as_i64()
                .unwrap();
            for (email, role) in [
                ("bob@example.com", "viewer"),
                ("carol@example.com", "admin"),
            ] {
                let res = server
                    .post(&format!("/api/v1/orgs/{org}/members"))
                    .add_header("Authorization", bearer(&alice))
                    .json(&serde_json::json!({ "email": email, "role": role }))
                    .await;
                assert!(res.status_code().is_success(), "{}", res.text());
            }
            let taxonomy = |name: &str| Some(serde_json::json!({ "name": name }));

            // A viewer writes nothing and never reads the access settings.
            let (s, e) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                &bob,
                org,
                taxonomy("T0"),
            )
            .await;
            assert_eq!(
                (s, e["errors"]["args"]["permission"].as_str()),
                (403, Some("taxonomies:write"))
            );
            let (s, _) = call(&server, "GET", "/api/v1/iam/roles", &bob, org, None).await;
            assert_eq!(s, 403);

            // A custom role never holds access management.
            let (s, e) = call(
                &server,
                "POST",
                "/api/v1/iam/roles",
                &alice,
                org,
                Some(serde_json::json!({
                    "key": "sneaky", "name": "Sneaky",
                    "statements": [{ "effect": "allow", "permissions": ["iam:write"] }]
                })),
            )
            .await;
            assert_eq!(s, 400, "{e}");
            assert_eq!(
                e["errors"]["fields"]["statements.0.permissions"],
                "iam_permission:iam:write"
            );

            // Custom role given to the viewer: taxonomies, and nothing else.
            let (s, role) = call(
                &server,
                "POST",
                "/api/v1/iam/roles",
                &alice,
                org,
                Some(serde_json::json!({
                    "key": "topic-editor", "name": "Topic editor",
                    "statements": [{ "effect": "allow", "permissions": ["taxonomies:*"] }]
                })),
            )
            .await;
            assert_eq!(s, 201, "{role}");
            let role_id = role["id"].as_str().unwrap().to_string();
            let (s, a) = call(
                &server,
                "POST",
                "/api/v1/iam/assignments",
                &alice,
                org,
                Some(serde_json::json!({
                    "user_id": ids["bob"], "role_id": role_id
                })),
            )
            .await;
            assert_eq!(s, 201, "{a}");
            assert_eq!(a["role_key"], "topic-editor");
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/iam/assignments",
                &alice,
                org,
                Some(serde_json::json!({
                    "user_id": ids["bob"], "role_id": role_id
                })),
            )
            .await;
            assert_eq!(s, 409, "same role twice");
            let (s, t) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                &bob,
                org,
                taxonomy("T1"),
            )
            .await;
            assert_eq!(s, 201, "{t}");
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/resources/folders",
                &bob,
                org,
                Some(serde_json::json!({ "name": "F" })),
            )
            .await;
            assert_eq!(s, 403, "only what the role gives");
            let (_, me) = call(
                &server,
                "GET",
                "/api/v1/iam/me/permissions",
                &bob,
                org,
                None,
            )
            .await;
            assert_eq!(me["role"], "viewer");
            assert!(me["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p == "taxonomies:write"));

            // Optimistic concurrency, then the change applies at once.
            let (s, _) = call(
                &server,
                "PATCH",
                &format!("/api/v1/iam/roles/{role_id}"),
                &alice,
                org,
                Some(serde_json::json!({
                    "statements": [], "expected_version": 7
                })),
            )
            .await;
            assert_eq!(s, 409);
            let (s, r) = call(
                &server,
                "PATCH",
                &format!("/api/v1/iam/roles/{role_id}"),
                &alice,
                org,
                Some(serde_json::json!({
                    "statements": [], "expected_version": 1
                })),
            )
            .await;
            assert_eq!((s, r["version"].as_i64()), (200, Some(2)), "{r}");
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                &bob,
                org,
                taxonomy("T2"),
            )
            .await;
            assert_eq!(s, 403, "an emptied role gives nothing any more");

            // Anti-escalation: an admin cannot give what only an owner holds.
            let (s, e) = call(
                &server,
                "POST",
                "/api/v1/iam/roles",
                &carol,
                org,
                Some(serde_json::json!({
                    "key": "org-killer", "name": "Org killer",
                    "statements": [{ "effect": "allow", "permissions": ["orgs:*"] }]
                })),
            )
            .await;
            assert_eq!(
                (s, e["error"].as_str()),
                (403, Some("iam-escalation")),
                "{e}"
            );
            assert_eq!(e["errors"]["args"]["permission"], "orgs:delete");

            // A deny through a team takes away even from an admin.
            let (s, team) = call(
                &server,
                "POST",
                "/api/v1/iam/teams",
                &alice,
                org,
                Some(serde_json::json!({ "name": "Readers" })),
            )
            .await;
            assert_eq!(s, 201, "{team}");
            let team_id = team["id"].as_str().unwrap().to_string();
            let (_, deny) = call(
                &server,
                "POST",
                "/api/v1/iam/roles",
                &alice,
                org,
                Some(serde_json::json!({
                    "key": "no-topics", "name": "No topics",
                    "statements": [{ "effect": "deny", "permissions": ["taxonomies:write"] }]
                })),
            )
            .await;
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/iam/assignments",
                &alice,
                org,
                Some(serde_json::json!({
                    "team_id": team_id, "role_id": deny["id"]
                })),
            )
            .await;
            assert_eq!(s, 201);
            let (s, t) = call(
                &server,
                "POST",
                &format!("/api/v1/iam/teams/{team_id}/members"),
                &alice,
                org,
                Some(serde_json::json!({ "user_id": ids["carol"] })),
            )
            .await;
            assert_eq!(
                (s, t["members"].as_array().map(Vec::len)),
                (200, Some(1)),
                "{t}"
            );
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                &carol,
                org,
                taxonomy("T3"),
            )
            .await;
            assert_eq!(s, 403, "denied through the team");
            let (s, _) = call(&server, "GET", "/api/v1/iam/roles", &carol, org, None).await;
            assert_eq!(s, 200, "a deny never locks access management");
            let (s, _) = call(
                &server,
                "DELETE",
                &format!("/api/v1/iam/teams/{team_id}/members/{}", ids["carol"]),
                &alice,
                org,
                None,
            )
            .await;
            assert_eq!(s, 200);
            let (s, _) = call(
                &server,
                "POST",
                "/api/v1/taxonomies",
                &carol,
                org,
                taxonomy("T4"),
            )
            .await;
            assert_eq!(s, 201, "out of the team, out of the deny");

            // Roles list: four managed first, then the custom ones.
            let (_, page) = call(&server, "GET", "/api/v1/iam/roles", &alice, org, None).await;
            let keys: Vec<&str> = page["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["key"].as_str().unwrap())
                .collect();
            assert_eq!(
                keys,
                [
                    "viewer",
                    "member",
                    "admin",
                    "owner",
                    "no-topics",
                    "topic-editor"
                ]
            );

            // Leaving the org takes the member's roles with them.
            let res = server
                .delete(&format!("/api/v1/orgs/{org}/members/{}", ids["bob"]))
                .add_header("Authorization", bearer(&alice))
                .await;
            assert!(res.status_code().is_success(), "{}", res.text());
            let (_, page) = call(
                &server,
                "GET",
                &format!("/api/v1/iam/assignments?user_id={}", ids["bob"]),
                &alice,
                org,
                None,
            )
            .await;
            assert_eq!(page["count"], 0);
            let (s, _) = call(
                &server,
                "DELETE",
                &format!("/api/v1/iam/roles/{role_id}"),
                &alice,
                org,
                None,
            )
            .await;
            assert_eq!(s, 204);
        },
    )
    .await;
}

/// Lot I6: a service account holds nothing but its assignments, reads with
/// its access key and nothing more, never reaches the human routes nor
/// another org, and loses access at once when its key is revoked, expired
/// or the account deleted. The secret is shown once.
#[tokio::test]
#[serial]
async fn service_accounts_and_access_keys() {
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            let me = server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(&alice))
                .await
                .json::<Value>();
            let personal = me["orgs"][0]["id"].as_i64().unwrap();
            let org = server
                .post("/api/v1/orgs")
                .add_header("Authorization", bearer(&alice))
                .json(&serde_json::json!({ "name": "Atelier IAM 6" }))
                .await
                .json::<Value>()["id"]
                .as_i64()
                .unwrap();
            let (s, sa) = call(&server, "POST", "/api/v1/iam/service-accounts", &alice, org, Some(serde_json::json!({ "name": "site-app" }))).await;
            assert_eq!(s, 201, "{sa}");
            let sa_id = sa["id"].as_str().unwrap().to_string();
            let keys = format!("/api/v1/iam/service-accounts/{sa_id}/keys");
            let in_days = |d: i64| (chrono::Utc::now() + chrono::Duration::days(d)).to_rfc3339();

            let (s, e) = call(&server, "POST", &keys, &alice, org, Some(serde_json::json!({ "expires_at": in_days(400) }))).await;
            assert_eq!((s, e["errors"]["fields"]["expires_at"].as_str()), (400, Some("max_days:366")));
            let (s, k1) = call(&server, "POST", &keys, &alice, org, Some(serde_json::json!({ "expires_at": in_days(30) }))).await;
            assert_eq!(s, 201, "{k1}");
            let secret = k1["secret"].as_str().unwrap().to_string();
            assert!(secret.starts_with("pnex_sk_") && secret.starts_with(k1["key"]["prefix"].as_str().unwrap()));
            let (_, list) = call(&server, "GET", &keys, &alice, org, None).await;
            assert_eq!(list["count"], 1);
            assert!(!list.to_string().contains(&secret[16..]), "the secret is never read back");

            // The key holder: no X-Org-Id needed, nothing held yet.
            let with_key = |path: String, org_header: Option<i64>| {
                let server = &server;
                let secret = secret.clone();
                async move {
                    let mut req = server.get(&path).add_header("Authorization", format!("Bearer {secret}"));
                    if let Some(o) = org_header {
                        req = req.add_header("X-Org-Id", o.to_string());
                    }
                    let res = req.await;
                    (res.status_code().as_u16(), serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null))
                }
            };
            let (s, e) = with_key("/api/v1/dashboards".into(), None).await;
            assert_eq!((s, e["error"].as_str()), (403, Some("access-denied")), "nothing until assigned");

            // Never access management, through a managed role either.
            let (s, e) = call(&server, "POST", "/api/v1/iam/assignments", &alice, org, Some(serde_json::json!({
                "service_account_id": sa_id, "managed_role": "admin"
            }))).await;
            assert_eq!((s, e["errors"]["fields"]["managed_role"].as_str()), (400, Some("not_for_service_account")));
            let (_, role) = call(&server, "POST", "/api/v1/iam/roles", &alice, org, Some(serde_json::json!({
                "key": "site-reader", "name": "Site reader",
                "statements": [{ "effect": "allow", "permissions": ["dashboards:read", "taxonomies:*"] }]
            }))).await;
            let (s, a) = call(&server, "POST", "/api/v1/iam/assignments", &alice, org, Some(serde_json::json!({
                "service_account_id": sa_id, "role_id": role["id"]
            }))).await;
            assert_eq!(s, 201, "{a}");
            assert_eq!(a["service_account_id"].as_str(), Some(sa_id.as_str()));

            let (s, page) = with_key("/api/v1/dashboards".into(), None).await;
            assert_eq!(s, 200, "{page}");
            let (s, _) = with_key("/api/v1/dashboards".into(), Some(org)).await;
            assert_eq!(s, 200, "its own org may be named");
            let (s, e) = with_key("/api/v1/dashboards".into(), Some(personal)).await;
            assert_eq!((s, e["error"].as_str()), (403, Some("org-not-member")), "never another org");
            let (s, _) = with_key("/api/v1/flows".into(), None).await;
            assert_eq!(s, 403, "only what its role gives");
            let (s, e) = with_key("/api/v1/user-info".into(), None).await;
            assert_eq!((s, e["error"].as_str()), (403, Some("access-key-not-allowed")));
            let (s, _) = with_key("/api/v1/iam/roles".into(), None).await;
            assert_eq!(s, 403);
            // Read-only in V1, whatever the role allows.
            let res = server
                .post("/api/v1/taxonomies")
                .add_header("Authorization", format!("Bearer {secret}"))
                .json(&serde_json::json!({ "name": "nope" }))
                .await;
            assert_eq!(res.status_code().as_u16(), 403, "{}", res.text());

            // Never a member: not listed, never added by email.
            let members = server
                .get(&format!("/api/v1/orgs/{org}/members"))
                .add_header("Authorization", bearer(&alice))
                .await
                .text();
            assert!(!members.contains("service-accounts.invalid"), "{members}");
            let res = server
                .post(&format!("/api/v1/orgs/{personal}/members"))
                .add_header("Authorization", bearer(&alice))
                .json(&serde_json::json!({ "email": format!("sa-{sa_id}@service-accounts.invalid"), "role": "viewer" }))
                .await;
            assert_eq!(res.status_code().as_u16(), 404, "{}", res.text());

            // Rotation: two keys at most, revocation is immediate.
            let (s, k2) = call(&server, "POST", &keys, &alice, org, Some(serde_json::json!({ "expires_at": in_days(60) }))).await;
            assert_eq!(s, 201);
            let (s, e) = call(&server, "POST", &keys, &alice, org, Some(serde_json::json!({ "expires_at": in_days(60) }))).await;
            assert_eq!((s, e["error"].as_str()), (409, Some("iam-access-key-limit")));
            let k1_id = k1["key"]["id"].as_str().unwrap();
            let (s, _) = call(&server, "DELETE", &format!("{keys}/{k1_id}"), &alice, org, None).await;
            assert_eq!(s, 204);
            let (s, _) = with_key("/api/v1/dashboards".into(), None).await;
            assert_eq!(s, 401, "revoked");

            // Expired, then deleted with its account.
            let secret2 = k2["secret"].as_str().unwrap().to_string();
            let get2 = || {
                server
                    .get("/api/v1/dashboards")
                    .add_header("Authorization", format!("Bearer {secret2}"))
            };
            assert_eq!(get2().await.status_code().as_u16(), 200);
            use sea_orm::ConnectionTrait;
            ctx.db
                .execute_unprepared(&format!(
                    "UPDATE iam_access_keys SET expires_at = now() - interval '1 second' WHERE id = '{}'",
                    k2["key"]["id"].as_str().unwrap()
                ))
                .await
                .unwrap();
            assert_eq!(get2().await.status_code().as_u16(), 401, "expired");
            let (s, _) = call(&server, "DELETE", &format!("/api/v1/iam/service-accounts/{sa_id}"), &alice, org, None).await;
            assert_eq!(s, 204);
            let (_, page) = call(&server, "GET", "/api/v1/iam/assignments", &alice, org, None).await;
            assert_eq!(page["count"], 0, "its assignments go with it");
        },
    )
    .await;
}
