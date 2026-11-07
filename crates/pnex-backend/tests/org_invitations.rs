//! Org membership by invitation (SEC-72), org routes behind the IAM
//! permission (SEC-75) and the verified-email rule of provisioning (SEC-77).
//!
//! Requires PostgreSQL (DATABASE_URL / default pnex_test).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

struct Env {
    base: String,
    alice: String,
    bob: String,
    carol: String,
}

fn token(base: &str, sub: &str, name: &str, email: &str, verified: bool) -> String {
    common::mint_token(&common::TokenSpec {
        sub: sub.into(),
        preferred_username: name.into(),
        email: email.into(),
        given_name: name.into(),
        family_name: "Test".into(),
        issuer: format!("{base}/auth/v1/"),
        email_verified: verified,
        ..Default::default()
    })
}

const ALICE: &str = "00000000-0000-0000-0000-0000000000a1";
const BOB: &str = "00000000-0000-0000-0000-0000000000b1";
const CAROL: &str = "00000000-0000-0000-0000-0000000000c1";

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, sea_orm::DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe {
        std::env::set_var("RAUTHY_URL", &base);
        // Read once per process: set before the first sign-in.
        std::env::set_var("PNEX_PLATFORM_ADMIN_EMAILS", "root@example.com");
    }
    let env = Env {
        alice: token(&base, ALICE, "Alice", "alice@example.com", true),
        bob: token(&base, BOB, "Bob", "bob@example.com", true),
        carol: token(&base, CAROL, "Carol", "carol@example.com", true),
        base,
    };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env, ctx.db.clone()).await;
        },
    )
    .await;
}

/// (status, body) of a JSON call; `org` goes in `X-Org-Id` when set.
async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: Option<i64>,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PATCH" => server.patch(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"));
    if let Some(org) = org {
        req = req.add_header("X-Org-Id", org.to_string());
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let res = req.await;
    let status = res.status_code().as_u16();
    let text = res.text();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn user_id(server: &axum_test::TestServer, token: &str) -> i64 {
    call(server, "GET", "/api/v1/user-info", token, None, None)
        .await
        .1["id"]
        .as_i64()
        .expect("user id")
}

async fn new_org(server: &axum_test::TestServer, token: &str, name: &str) -> i64 {
    let (s, org) = call(
        server,
        "POST",
        "/api/v1/orgs",
        token,
        None,
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(s, 201, "{org}");
    org["id"].as_i64().unwrap()
}

async fn invite(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    email: &str,
    role: &str,
) -> (u16, Value) {
    call(
        server,
        "POST",
        &format!("/api/v1/orgs/{org}/invitations"),
        token,
        Some(org),
        Some(json!({ "email": email, "role": role })),
    )
    .await
}

async fn mine(server: &axum_test::TestServer, token: &str) -> Vec<Value> {
    let (s, page) = call(server, "GET", "/api/v1/invitations", token, None, None).await;
    assert_eq!(s, 200, "{page}");
    page["results"].as_array().cloned().unwrap_or_default()
}

async fn member_ids(server: &axum_test::TestServer, token: &str, org: i64) -> Vec<i64> {
    let (s, page) = call(
        server,
        "GET",
        &format!("/api/v1/orgs/{org}/members"),
        token,
        Some(org),
        None,
    )
    .await;
    assert_eq!(s, 200, "{page}");
    page["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["user_id"].as_i64())
        .collect()
}

#[tokio::test]
#[serial]
async fn invite_accept_decline_and_expiry() {
    with_app(|server, env, db| async move {
        let org = new_org(&server, &env.alice, "Invite Co").await;
        let bob_id = user_id(&server, &env.bob).await;

        // Invite → nobody is added before the answer.
        let (s, inv) = invite(&server, &env.alice, org, " Bob@Example.com ", "member").await;
        assert_eq!(s, 202, "{inv}");
        assert_eq!(inv["email"], "bob@example.com");
        assert!(inv.get("user_id").is_none(), "{inv}");
        assert!(!member_ids(&server, &env.alice, org).await.contains(&bob_id));
        let (s, pending) = call(
            &server,
            "GET",
            &format!("/api/v1/orgs/{org}/invitations"),
            &env.alice,
            Some(org),
            None,
        )
        .await;
        assert_eq!((s, pending["count"].as_i64()), (200, Some(1)), "{pending}");

        // Someone else neither sees nor accepts it.
        assert!(mine(&server, &env.carol).await.is_empty());
        let id = inv["id"].as_i64().unwrap();
        let (s, e) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &env.carol,
            None,
            None,
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (404, Some("org-invitation-not-found"))
        );

        // Not with an email the IdP did not verify either.
        let bob_unverified = token(&env.base, BOB, "Bob", "bob@example.com", false);
        assert!(mine(&server, &bob_unverified).await.is_empty());
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &bob_unverified,
            None,
            None,
        )
        .await;
        assert_eq!(s, 404);

        // The invitee sees it and accepts: member with the invited role.
        let listed = mine(&server, &env.bob).await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0]["org_name"], "Invite Co");
        assert_eq!(listed[0]["invited_by"], "Alice Test");
        let (s, joined) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &env.bob,
            None,
            None,
        )
        .await;
        assert_eq!(
            (s, joined["role"].as_str()),
            (200, Some("member")),
            "{joined}"
        );
        assert!(member_ids(&server, &env.alice, org).await.contains(&bob_id));
        assert!(mine(&server, &env.bob).await.is_empty());
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &env.bob,
            None,
            None,
        )
        .await;
        assert_eq!(s, 404, "used up");
        let (s, e) = invite(&server, &env.alice, org, "bob@example.com", "viewer").await;
        assert_eq!(
            (s, e["error"].as_str()),
            (409, Some("org-member-duplicate"))
        );

        // Decline: no membership, the invitation is gone.
        let carol_id = user_id(&server, &env.carol).await;
        let (_, inv) = invite(&server, &env.alice, org, "carol@example.com", "viewer").await;
        let id = inv["id"].as_i64().unwrap();
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/decline"),
            &env.carol,
            None,
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(!member_ids(&server, &env.alice, org)
            .await
            .contains(&carol_id));
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &env.carol,
            None,
            None,
        )
        .await;
        assert_eq!(s, 404);

        // Expiry: an expired invitation is neither listed nor accepted.
        let (_, inv) = invite(&server, &env.alice, org, "carol@example.com", "viewer").await;
        let id = inv["id"].as_i64().unwrap();
        use sea_orm::ConnectionTrait;
        db.execute_unprepared(&format!(
            "UPDATE org_invitations SET expires_at = now() - interval '1 minute' WHERE id = {id}"
        ))
        .await
        .unwrap();
        assert!(mine(&server, &env.carol).await.is_empty());
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &env.carol,
            None,
            None,
        )
        .await;
        assert_eq!(s, 404);
        // Inviting again renews it.
        let (s, again) = invite(&server, &env.alice, org, "carol@example.com", "viewer").await;
        assert_eq!((s, again["id"].as_i64()), (202, Some(id)));
        assert_eq!(mine(&server, &env.carol).await.len(), 1);

        // Revoke: gone for the invitee.
        let (s, _) = call(
            &server,
            "DELETE",
            &format!("/api/v1/orgs/{org}/invitations/{id}"),
            &env.alice,
            Some(org),
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(mine(&server, &env.carol).await.is_empty());

        // Leave: bob goes, the last owner cannot.
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/leave"),
            &env.bob,
            None,
            None,
        )
        .await;
        assert_eq!(s, 204);
        assert!(!member_ids(&server, &env.alice, org).await.contains(&bob_id));
        let (s, e) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/leave"),
            &env.alice,
            None,
            None,
        )
        .await;
        assert_eq!((s, e["error"].as_str()), (409, Some("org-last-owner")));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unknown_email_gets_the_same_answer_and_waits_for_sign_up() {
    with_app(|server, env, _db| async move {
        let org = new_org(&server, &env.alice, "Waiting Co").await;
        user_id(&server, &env.bob).await;
        let (s1, known) = invite(&server, &env.alice, org, "bob@example.com", "viewer").await;
        let (s2, unknown) = invite(&server, &env.alice, org, "dave@example.com", "viewer").await;
        assert_eq!((s1, s2), (202, 202));
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&known), keys(&unknown));
        assert_eq!(known["role"], unknown["role"]);

        // Dave signs up later with that verified email: the invitation is his.
        let dave = token(
            &env.base,
            "00000000-0000-0000-0000-0000000000d1",
            "Dave",
            "dave@example.com",
            true,
        );
        let listed = mine(&server, &dave).await;
        assert_eq!(listed.len(), 1, "{listed:?}");
        let id = listed[0]["id"].as_i64().unwrap();
        let (s, _) = call(
            &server,
            "POST",
            &format!("/api/v1/invitations/{id}/accept"),
            &dave,
            None,
            None,
        )
        .await;
        assert_eq!(s, 200);
        let dave_id = user_id(&server, &dave).await;
        assert!(member_ids(&server, &env.alice, org)
            .await
            .contains(&dave_id));
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_routes_check_the_permission_and_the_path_org() {
    with_app(|server, env, _db| async move {
        let org = new_org(&server, &env.alice, "Perm Co").await;
        let alice_personal = call(&server, "GET", "/api/v1/orgs", &env.alice, None, None)
            .await
            .1["results"][0]["id"]
            .as_i64()
            .unwrap();
        let bob_id = user_id(&server, &env.bob).await;
        let carol_id = user_id(&server, &env.carol).await;
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
        .await;
        common::join_org(
            &server,
            &env.alice,
            &env.carol,
            org,
            "carol@example.com",
            "admin",
        )
        .await;

        // A viewer reads, writes nothing: refused by the permission (SEC-75).
        let (s, d) = call(
            &server,
            "GET",
            &format!("/api/v1/orgs/{org}"),
            &env.bob,
            Some(org),
            None,
        )
        .await;
        assert_eq!((s, d["role"].as_str()), (200, Some("viewer")));
        for (method, path, body, permission) in [
            (
                "PATCH",
                format!("/api/v1/orgs/{org}"),
                Some(json!({ "name": "Hack" })),
                "orgs:write",
            ),
            ("DELETE", format!("/api/v1/orgs/{org}"), None, "orgs:delete"),
            (
                "POST",
                format!("/api/v1/orgs/{org}/invitations"),
                Some(json!({ "email": "x@example.com" })),
                "orgs.members:write",
            ),
            (
                "GET",
                format!("/api/v1/orgs/{org}/invitations"),
                None,
                "orgs.invitations:read",
            ),
            (
                "PATCH",
                format!("/api/v1/orgs/{org}/members/{bob_id}"),
                Some(json!({ "role": "owner" })),
                "orgs.members:write",
            ),
            (
                "DELETE",
                format!("/api/v1/orgs/{org}/members/{carol_id}"),
                None,
                "orgs.members:delete",
            ),
        ] {
            let (s, e) = call(&server, method, &path, &env.bob, Some(org), body).await;
            assert_eq!(
                (s, e["error"].as_str()),
                (403, Some("access-denied")),
                "{method} {path}: {e}"
            );
            assert_eq!(e["errors"]["args"]["permission"], permission);
        }

        // An admin manages members but no owner act.
        let (s, e) = invite(&server, &env.carol, org, "x@example.com", "owner").await;
        assert_eq!(
            (s, e["error"].as_str()),
            (403, Some("org-owner-grant-forbidden"))
        );
        let (s, _) = invite(&server, &env.carol, org, "x@example.com", "member").await;
        assert_eq!(s, 202);
        let alice_id = user_id(&server, &env.alice).await;
        let (s, e) = call(
            &server,
            "DELETE",
            &format!("/api/v1/orgs/{org}/members/{alice_id}"),
            &env.carol,
            Some(org),
            None,
        )
        .await;
        assert_eq!(
            (s, e["error"].as_str()),
            (403, Some("org-owner-remove-forbidden"))
        );
        let (s, _) = call(
            &server,
            "PATCH",
            &format!("/api/v1/orgs/{org}/members/{bob_id}"),
            &env.carol,
            Some(org),
            Some(json!({ "role": "member" })),
        )
        .await;
        assert_eq!(s, 200);

        // The path org must be the X-Org-Id one; a non-member is refused.
        let (s, e) = call(
            &server,
            "PATCH",
            &format!("/api/v1/orgs/{org}"),
            &env.alice,
            Some(alice_personal),
            Some(json!({ "name": "X" })),
        )
        .await;
        assert_eq!((s, e["error"].as_str()), (403, Some("org-path-mismatch")));
        let (s, e) = call(
            &server,
            "GET",
            &format!("/api/v1/orgs/{alice_personal}"),
            &env.bob,
            Some(alice_personal),
            None,
        )
        .await;
        assert_eq!((s, e["error"].as_str()), (403, Some("org-not-member")));
        let (s, _) = call(
            &server,
            "GET",
            &format!("/api/v1/orgs/{org}"),
            &env.alice,
            None,
            None,
        )
        .await;
        assert_eq!(s, 400, "X-Org-Id required");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn unverified_email_neither_promotes_nor_resyncs() {
    with_app(|server, env, _db| async move {
        // Platform admin email, not verified: no promotion (SEC-77).
        let root = "00000000-0000-0000-0000-0000000000e1";
        let unverified = token(&env.base, root, "Root", "root@example.com", false);
        let (_, me) = call(&server, "GET", "/api/v1/user-info", &unverified, None, None).await;
        assert_eq!(me["platform_admin"], false, "{me}");
        // A new name skips the per-pod user cache.
        let verified = token(&env.base, root, "Rooted", "root@example.com", true);
        let (_, me) = call(&server, "GET", "/api/v1/user-info", &verified, None, None).await;
        assert_eq!(me["platform_admin"], true, "{me}");

        // An unverified new email never replaces the account email.
        let moved = token(&env.base, root, "Root", "other@example.com", false);
        let (_, me) = call(&server, "GET", "/api/v1/user-info", &moved, None, None).await;
        assert_eq!(me["email"], "root@example.com", "{me}");
        let moved = token(&env.base, root, "Root", "other@example.com", true);
        let (_, me) = call(&server, "GET", "/api/v1/user-info", &moved, None, None).await;
        assert_eq!(me["email"], "other@example.com", "{me}");
    })
    .await;
}
