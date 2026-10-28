//! Multi-tenant isolation at the HTTP level: two users (mock tokens
//! signed by a fake Rauthy) must never see or modify each other's
//! organisations; role rules apply.
//!
//! Requires PostgreSQL (DATABASE_URL / default pnex_test) — the test database
//! is created/dropped by the loco framework.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    base: String,
    alice: String,
    bob: String,
}

/// Boots the app on a test database with a fake Rauthy, runs the callback.
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    // Makes the extractor's warnings visible (JWT rejection, provisioning).
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    // settings.rauthy.base_url reads RAUTHY_URL — set before the boot.
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let env = Env {
        base: base.clone(),
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
            // The test database is blank: we put the Free tier in it (the full
            // seed is a task, out of scope here).
            use pnex_backend::models::_entities::subscription_tiers as tiers;
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            if tiers::Entity::find()
                .filter(tiers::Column::Name.eq("Free"))
                .one(&ctx.db)
                .await
                .expect("lookup tier Free")
                .is_none()
            {
                tiers::ActiveModel {
                    name: Set("Free".into()),
                    max_sensor_devices: Set(3),
                    max_actuator_devices: Set(1),
                    max_mixed_devices: Set(0),
                    min_build_interval_secs: Set(300),
                    data_retention_secs: Set(Some(86_400)),
                    ..Default::default()
                }
                .insert(&ctx.db)
                .await
                .expect("insert tier Free");
            }
            f(server, env).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
#[serial]
async fn sans_token_toute_l_api_est_refusee() {
    with_app(|server, _env| async move {
        let res = server.get("/api/v1/user-info").await;
        assert_eq!(res.status_code(), 401);
        let res = server.get("/api/v1/orgs").await;
        assert_eq!(res.status_code(), 401);
        let res = server.post("/api/v1/orgs").await;
        assert_eq!(res.status_code(), 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn jit_provisioning_cree_user_profil_et_org_owner() {
    with_app(|server, env| async move {
        let res = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        let orgs = body["orgs"].as_array().expect("orgs");
        assert_eq!(orgs.len(), 1, "personal org created: {orgs:?}");
        assert_eq!(orgs[0]["role"], "owner");
        assert_eq!(
            orgs[0]["subscription_tier"]["name"], "Free",
            "personal org on the Free tier"
        );
        let profile = body["profile"].as_object().expect("profile created");
        assert_eq!(profile["language"], "en");
        assert_eq!(profile["timezone"], "UTC");

        // Idempotent: second call = same user, no duplicate org.
        let res2 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        let body2: serde_json::Value = res2.json();
        assert_eq!(body2["id"], body["id"]);
        assert_eq!(body2["orgs"].as_array().unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sub_change_avec_meme_email_relie_le_meme_user() {
    with_app(|server, env| async move {
        // alice provisions herself with a first sub.
        let sub_a = "00000000-0000-0000-0000-0000000000aa";
        let token_a = common::valid_token(&env.base, sub_a, "alice", "relink@example.com");
        let first: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token_a))
            .await
            .json();
        assert_eq!(first["username"], "alice");

        // The same email comes back with an unknown sub (migrated IdP — e.g.
        // reimported, IdP migration): the users row must be RE-LINKED,
        // not duplicated (unique email) — same id, same orgs.
        let sub_b = "00000000-0000-0000-0000-0000000000ab";
        let token_b = common::valid_token(&env.base, sub_b, "alice", "relink@example.com");
        let second = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token_b))
            .await;
        assert_eq!(second.status_code(), 200, "re-link by email, no 500");
        let second: serde_json::Value = second.json();
        assert_eq!(second["id"], first["id"], "same user re-linked");
        assert_eq!(
            second["orgs"].as_array().unwrap().len(),
            first["orgs"].as_array().unwrap().len(),
            "no duplicated personal org"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sso_register_et_reset_pointent_vers_les_pages_rauthy() {
    with_app(|server, env| async move {
        // register: Rauthy sign-up UI page (not an OIDC endpoint,
        // activation by mail — no OAuth2 params).
        let res = server
            .get("/api/v1/oauth2/sso?code_challenge=abc&code_challenge_method=S256&action=register")
            .await;
        assert_eq!(res.status_code(), 302);
        let location = res
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(location.contains("/auth/v1/users/register"), "{location}");

        // D239: a missing PKCE challenge is a `field-invalid` ErrorDetail.
        let res = server.get("/api/v1/oauth2/sso").await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["errors"]["fields"]["code_challenge"], "required");

        // SEC-41: `prompt=login` is relayed to the authorize request, any
        // other prompt value is dropped.
        let location_of = |res: axum_test::TestResponse| {
            res.headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let forced = location_of(
            server
                .get(
                    "/api/v1/oauth2/sso?code_challenge=abc&code_challenge_method=S256&prompt=login",
                )
                .await,
        );
        assert!(forced.contains("prompt=login"), "{forced}");
        let other = location_of(
            server
                .get("/api/v1/oauth2/sso?code_challenge=abc&code_challenge_method=S256&prompt=none")
                .await,
        );
        assert!(!other.contains("prompt="), "{other}");

        // logout: auto-submitted HTML form POSTing to the Rauthy end-session
        // (top-level navigation — Rauthy's SPA logout page never hands
        // control back to the app, see the handler's doc-comment).
        let res = server
            .get("/api/v1/oauth2/logout?id_token=jwt.jwt.jwt")
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        let html = res.text();
        assert!(html.contains(r#"method="POST""#), "{html}");
        assert!(
            html.contains(&format!(r#"action="{}/auth/v1/oidc/logout""#, env.base)),
            "{html}"
        );
        assert!(
            html.contains(r#"name="id_token_hint" value="jwt.jwt.jwt""#),
            "{html}"
        );
        assert!(
            html.contains(r#"name="post_logout_redirect_uri""#),
            "{html}"
        );

        // reset: Rauthy account page (the password change lives in the
        // IdP — equivalent of Keycloak's kc_action=UPDATE_PASSWORD).
        let res = server
            .get("/api/v1/oauth2/sso?code_challenge=abc&code_challenge_method=S256&action=reset")
            .await;
        let location = res
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(location.contains("/auth/v1/account"), "{location}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn token_lean_rauthy_username_reprend_l_email() {
    with_app(|server, env| async move {
        // Actual shape of a Rauthy access token (D19): without
        // preferred_username (the claim lives in the id_token). The user-info
        // response must NEVER serialise `username: null` — the shared
        // UserInfo contract expects a string (otherwise the front crashes:
        // "unreadable response: invalid type: null").
        let token = common::lean_token(
            &env.base,
            "00000000-0000-0000-0000-00000000000c",
            "lean@example.com",
        );
        let body: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&token))
            .await
            .json();
        assert_eq!(body["username"], "lean@example.com");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn un_tenant_ne_voit_pas_les_orgs_de_l_autre() {
    with_app(|server, env| async move {
        // Provisions alice then bob.
        for token in [&env.alice, &env.bob] {
            let res = server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
            assert_eq!(res.status_code(), 200);
        }

        // Alice's personal org.
        let alice_orgs: serde_json::Value = server
            .get("/api/v1/orgs")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let alice_org_id = alice_orgs["results"][0]["id"]
            .as_i64()
            .expect("id org alice");

        // Bob doesn't see alice's org, neither for reading nor for writing.
        let res = server
            .get(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 404, "alice's org invisible to bob");
        let res = server
            .patch(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "name": "Hack" }))
            .await;
        assert_eq!(res.status_code(), 404, "alice's org not modifiable by bob");
        let res = server
            .delete(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 404);

        // Bob can't add himself to alice's org either.
        let res = server
            .post(&format!("/api/v1/orgs/{alice_org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "email": "bob@example.com" }))
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn roles_viewer_owner_et_garde_fous() {
    with_app(|server, env| async move {
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }

        // Alice creates a shared org and adds bob as viewer.
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Co" }))
            .await
            .json();
        let org_id = created["id"].as_i64().expect("id Atelier Co");
        // The UI decodes the creation as an OrgSummary (role required).
        serde_json::from_value::<pnex_core::api::OrgSummary>(created.clone())
            .expect("creation response is an OrgSummary");

        let added: serde_json::Value = server
            .post(&format!("/api/v1/orgs/{org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await
            .json();
        assert_eq!(added["role"], "viewer", "lowercase role on input/output");

        // O1: bob's orgs listed by increasing id (JIT personal org
        // first) — deterministic order for the front's org fallback,
        // which could otherwise land on a viewer org.
        let bob_info: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json();
        let bob_orgs = bob_info["orgs"].as_array().expect("orgs bob");
        assert!(
            bob_orgs.len() >= 2,
            "bob has his personal org + Atelier Co: {bob_orgs:?}"
        );
        let bob_org_ids: Vec<i64> = bob_orgs.iter().filter_map(|o| o["id"].as_i64()).collect();
        let mut sorted_ids = bob_org_ids.clone();
        sorted_ids.sort();
        assert_eq!(bob_org_ids, sorted_ids, "orgs sorted by increasing id");
        assert_eq!(
            bob_orgs[0]["role"], "owner",
            "the personal org (non viewer) comes first"
        );

        // Bob (viewer) reads, but can't write.
        let res = server
            .get(&format!("/api/v1/orgs/{org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 200);
        let detail: serde_json::Value = res.json();
        assert_eq!(detail["role"], "viewer");
        assert_eq!(detail["members"].as_array().unwrap().len(), 2);

        let res = server
            .patch(&format!("/api/v1/orgs/{org_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "name": "Hack Co" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer does not rename");
        let res = server
            .post(&format!("/api/v1/orgs/{org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "email": "alice@example.com" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer does not add a member");

        // Alice can't leave her role as the last owner.
        let alice_user_id: i64 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .expect("id alice");
        let res = server
            .delete(&format!("/api/v1/orgs/{org_id}/members/{alice_user_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 409, "dernier owner inamovible");

        // Bob the viewer tries to promote himself: refused (owner required).
        let bob_user_id: i64 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .expect("id bob");
        let res = server
            .patch(&format!("/api/v1/orgs/{org_id}/members/{bob_user_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "role": "owner" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer does not promote themselves");

        // D241: the role change answers the lowercase role (was "Member").
        let res = server
            .patch(&format!("/api/v1/orgs/{org_id}/members/{bob_user_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "role": "member" }))
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.json::<serde_json::Value>()["role"], "member");

        // D239: an empty org name is a `field-invalid` ErrorDetail.
        let res = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "  " }))
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(body["errors"]["fields"]["name"], "required");
    })
    .await;
}

/// SEC-W5: an unknown `sub` claiming the email of an existing account is
/// relinked only when the IdP verified that email.
#[tokio::test]
#[serial]
async fn relink_by_email_requires_a_verified_email() {
    with_app(|server, env| async move {
        let owner = common::valid_token(
            &env.base,
            "00000000-0000-0000-0000-0000000000c1",
            "alice",
            "verified-relink@example.com",
        );
        let first: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&owner))
            .await
            .json();
        let intruder = common::mint_token(&common::TokenSpec {
            sub: "00000000-0000-0000-0000-0000000000c2".into(),
            email: "verified-relink@example.com".into(),
            issuer: format!("{}/auth/v1/", env.base),
            email_verified: false,
            ..Default::default()
        });
        let res = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&intruder))
            .await;
        assert_eq!(
            res.status_code(),
            401,
            "unverified email never takes an account over"
        );
        // The owner keeps their account.
        let again: serde_json::Value = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&owner))
            .await
            .json();
        assert_eq!(again["id"], first["id"]);
    })
    .await;
}
