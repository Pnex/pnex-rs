//! `/api/v1/system/*` (D72): platform-admin gating, retention precedence
//! and validation, O2 cleanup guards (role, typed confirmation).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::{
    organization_members, sea_orm_active_enums::OrgMemberRole, users,
};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use serial_test::serial;

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "sys-alice-000000000000000",
        "alice",
        "sys-alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "sys-bob-00000000000000000",
        "bob",
        "sys-bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            f(server, ctx, alice, bob).await;
        },
    )
    .await;
}

/// Provisions the user and returns (user id, personal org id, org name).
async fn provision(server: &axum_test::TestServer, token: &str) -> (i64, i64, String) {
    let res = server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await;
    assert_eq!(res.status_code(), 200);
    let body: serde_json::Value = res.json();
    assert_eq!(body["platform_admin"], false);
    assert_eq!(body["deployment_mode"], "self_hosted");
    let org = &body["orgs"][0];
    (
        body["id"].as_i64().unwrap(),
        org["id"].as_i64().unwrap(),
        org["name"].as_str().unwrap().to_string(),
    )
}

async fn make_platform_admin(db: &sea_orm::DatabaseConnection, user_id: i64) {
    let user = users::Entity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: users::ActiveModel = user.into();
    active.platform_admin = Set(true);
    active.update(db).await.unwrap();
}

#[tokio::test]
#[serial]
async fn status_names_who_holds_each_lease_and_flags_another_version() {
    use sea_orm::ConnectionTrait;
    with_app(|server, ctx, alice, _bob| async move {
        let (alice_id, _, _) = provision(&server, &alice).await;
        make_platform_admin(&ctx.db, alice_id).await;
        let topology = || async {
            let body: serde_json::Value = server
                .get("/api/v1/system/status")
                .add_header("Authorization", bearer(&alice))
                .await
                .json();
            body["components"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["key"] == "topology")
                .unwrap()
                .clone()
        };
        let lease = |t: &serde_json::Value, name: &str| {
            t["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["key"] == "cluster_lease" && m["label"] == name)
                .and_then(|m| m["text"].as_str().map(str::to_string))
        };
        let db = &ctx.db;
        assert!(pnex_backend::services::flow_cluster::store::try_lease(
            db,
            "task:mine",
            "pod-a:1:1",
            60_000
        )
        .await
        .unwrap());
        let t = topology().await;
        let text = lease(&t, "task:mine").expect("lease listed");
        assert!(text.starts_with("pod-a · "), "{text}");
        assert!(text.contains(&pnex_backend::app::app_version()), "{text}");
        assert!(
            !t["detail"]
                .as_str()
                .unwrap_or("")
                .contains("another version"),
            "{t}"
        );

        // A pod left behind by a partial rollout.
        db.execute_unprepared(
            "INSERT INTO flow_leases (name, holder, expires_at, holder_version) \
             VALUES ('task:old', 'pod-b:1:1', now() + interval '1 minute', '0.0.1 (old)')",
        )
        .await
        .unwrap();
        // An expired lease holds nothing.
        db.execute_unprepared(
            "INSERT INTO flow_leases (name, holder, expires_at, holder_version) \
             VALUES ('task:gone', 'pod-c:1:1', now() - interval '1 minute', 'x')",
        )
        .await
        .unwrap();
        let t = topology().await;
        assert!(lease(&t, "task:old").unwrap().contains("0.0.1 (old)"));
        assert_eq!(lease(&t, "task:gone"), None);
    })
    .await;
}

/// O2 answering the stream lists and recording each change of settings.
async fn fake_o2(
    logs: serde_json::Value,
) -> (
    pnex_backend::services::openobserve::Client,
    std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
) {
    use axum::extract::{Path, Query, State};
    use axum::routing::{get, put};
    use std::collections::HashMap;
    type Puts = std::sync::Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>;
    let puts: Puts = Default::default();
    let router = axum::Router::new()
        .route(
            "/api/o/streams",
            get(move |Query(q): Query<HashMap<String, String>>| {
                let logs = logs.clone();
                async move {
                    axum::Json(if q.get("type").map(String::as_str) == Some("logs") {
                        logs
                    } else {
                        serde_json::json!({ "list": [] })
                    })
                }
            }),
        )
        .route(
            "/api/o/streams/{name}/settings",
            put(
                |State(puts): State<Puts>,
                 Path(name): Path<String>,
                 Query(q): Query<HashMap<String, String>>,
                 axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(q.get("type").map(String::as_str), Some("logs"));
                    puts.lock().unwrap().push((name, body));
                    axum::Json(serde_json::json!({}))
                },
            ),
        )
        .with_state(puts.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = pnex_backend::services::openobserve::Client::new(
        &pnex_backend::services::openobserve::OpenobserveSettings {
            base_url: format!("http://{addr}"),
            root_email: "root@example.com".into(),
            root_password: "x".into(),
        },
    );
    (client, puts)
}

#[tokio::test]
#[serial]
async fn reconcile_sets_the_history_retention_only_when_it_differs() {
    use pnex_backend::models::_entities::organizations;
    use pnex_backend::services::retention::reconcile_org;
    with_app(|server, ctx, alice, _bob| async move {
        let (_, org_id, _) = provision(&server, &alice).await;
        let org = organizations::Entity::find_by_id(org_id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        let stream = |days: i64| {
            serde_json::json!({ "list": [
                { "name": "other_logs", "settings": { "data_retention": 1 } },
                { "name": "object_changes", "settings": { "data_retention": days } },
            ]})
        };

        // The instance default on the journal: the history retention replaces it.
        let (client, puts) = fake_o2(stream(30)).await;
        assert_eq!(reconcile_org(&ctx.db, &client, &org, "o").await, Ok(1));
        assert_eq!(
            *puts.lock().unwrap(),
            [(
                "object_changes".to_string(),
                serde_json::json!({ "data_retention": 3650 })
            )]
        );

        // Already right: nothing written.
        let (client, puts) = fake_o2(stream(3650)).await;
        assert_eq!(reconcile_org(&ctx.db, &client, &org, "o").await, Ok(0));
        assert!(puts.lock().unwrap().is_empty());

        // Created by the first write of an object: nothing to set before that.
        let (client, puts) = fake_o2(serde_json::json!({ "list": [] })).await;
        assert_eq!(reconcile_org(&ctx.db, &client, &org, "o").await, Ok(0));
        assert!(puts.lock().unwrap().is_empty());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn status_is_reserved_to_platform_admins() {
    with_app(|server, ctx, alice, _bob| async move {
        let (alice_id, _, _) = provision(&server, &alice).await;

        let res = server
            .get("/api/v1/system/status")
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 403);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "platform-admin-required");

        make_platform_admin(&ctx.db, alice_id).await;
        let res = server
            .get("/api/v1/system/status")
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        let keys: Vec<&str> = body["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "database",
                "rauthy",
                "openobserve",
                "valkey",
                "object_storage",
                "host",
                "topology",
                "ai",
                "secrets"
            ]
        );
        assert_eq!(body["components"][0]["status"], "ok");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn retention_precedence_and_admin_edits() {
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, org_id, _) = provision(&server, &alice).await;
        let get = || {
            server
                .get("/api/v1/system/retention")
                .add_header("Authorization", bearer(&alice))
                .add_header("X-Org-Id", org_id.to_string())
        };

        // Clean slate: no global default left by another test.
        pnex_backend::services::retention::set_global_default(&ctx.db, None, alice_id)
            .await
            .unwrap();

        let body: serde_json::Value = get().await.json();
        assert_eq!(body["source"], "env");
        assert_eq!(body["days"], 30);
        assert_eq!(body["editable"], false);

        // Non-admin cannot edit.
        let res = server
            .put(&format!("/api/v1/system/retention/orgs/{org_id}"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "days": 90 }))
            .await;
        assert_eq!(res.status_code(), 403);

        make_platform_admin(&ctx.db, alice_id).await;

        let res = server
            .put(&format!("/api/v1/system/retention/orgs/{org_id}"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "days": 0 }))
            .await;
        assert_eq!(res.status_code(), 422);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "retention-out-of-range");

        // History has its own value (Q-P1), long by default, whatever the others say.
        pnex_backend::services::retention::set_history_days(&ctx.db, None, alice_id)
            .await
            .unwrap();
        let body: serde_json::Value = get().await.json();
        assert_eq!(body["history_days"], 3650);
        let put_history = |token: String, days: serde_json::Value| {
            server
                .put("/api/v1/system/retention/history")
                .add_header("Authorization", bearer(&token))
                .json(&serde_json::json!({ "days": days }))
        };
        let res = put_history(bob.clone(), 7.into()).await;
        assert_eq!(res.status_code(), 403);
        let res = put_history(alice.clone(), 0.into()).await;
        assert_eq!(res.status_code(), 422);
        let res = put_history(alice.clone(), 3651.into()).await;
        assert_eq!(res.status_code(), 422);
        let res = put_history(alice.clone(), 400.into()).await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = get().await.json();
        assert_eq!(body["history_days"], 400, "{body}");
        assert_ne!(body["days"], 400, "telemetry retention does not follow");
        let res = put_history(alice.clone(), serde_json::Value::Null).await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = get().await.json();
        assert_eq!(body["history_days"], 3650);

        let res = server
            .put("/api/v1/system/retention/default")
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "days": 14 }))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = get().await.json();
        assert_eq!(
            (body["source"].as_str(), body["days"].as_u64()),
            (Some("global"), Some(14))
        );
        assert_eq!(body["editable"], true);

        let res = server
            .put(&format!("/api/v1/system/retention/orgs/{org_id}"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "days": 90 }))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(
            (body["source"].as_str(), body["days"].as_u64()),
            (Some("override"), Some(90))
        );

        // Clearing the override falls back to the global default.
        let res = server
            .put(&format!("/api/v1/system/retention/orgs/{org_id}"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "days": null }))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["source"], "global");

        pnex_backend::services::retention::set_global_default(&ctx.db, None, alice_id)
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[serial]
async fn o2_cleanup_requires_write_role_and_typed_confirmation() {
    with_app(|server, ctx, alice, bob| async move {
        let (_, org_id, org_name) = provision(&server, &alice).await;
        let (bob_id, _, _) = provision(&server, &bob).await;

        // Owner, wrong confirmation → 422 before any O2 call.
        let res = server
            .post("/api/v1/system/o2/purge")
            .add_header("Authorization", bearer(&alice))
            .add_header("X-Org-Id", org_id.to_string())
            .json(&serde_json::json!({ "confirm": "not the name" }))
            .await;
        assert_eq!(res.status_code(), 422);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "confirmation-mismatch");

        // Bob joins Alice's org as viewer: every deletion is refused.
        if organization_members::Entity::find()
            .filter(organization_members::Column::OrgId.eq(org_id))
            .filter(organization_members::Column::UserId.eq(bob_id))
            .one(&ctx.db)
            .await
            .unwrap()
            .is_none()
        {
            organization_members::ActiveModel {
                org_id: Set(org_id),
                user_id: Set(bob_id),
                role: Set(OrgMemberRole::Viewer),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .unwrap();
        }
        let res = server
            .post("/api/v1/system/o2/purge")
            .add_header("Authorization", bearer(&bob))
            .add_header("X-Org-Id", org_id.to_string())
            .json(&serde_json::json!({ "confirm": org_name }))
            .await;
        assert_eq!(res.status_code(), 403);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "access-denied");

        let res = server
            .delete("/api/v1/system/o2/streams/soil_moisture")
            .add_header("Authorization", bearer(&bob))
            .add_header("X-Org-Id", org_id.to_string())
            .await;
        assert_eq!(res.status_code(), 403);

        // The viewer may still read the listing (O2 absent in tests).
        let res = server
            .get("/api/v1/system/o2/streams")
            .add_header("Authorization", bearer(&bob))
            .add_header("X-Org-Id", org_id.to_string())
            .await;
        assert_eq!(res.status_code(), 200);

        // Range validation happens before any O2 call.
        let res = server
            .post("/api/v1/system/o2/streams/soil_moisture/delete-range")
            .add_header("Authorization", bearer(&alice))
            .add_header("X-Org-Id", org_id.to_string())
            .json(&serde_json::json!({ "start": "2026-09-28T12:00:00Z", "end": "2026-09-28T10:00:00Z" }))
            .await;
        assert_eq!(res.status_code(), 422);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "o2-time-range-invalid");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn orgs_overview_is_admin_only_and_lists_every_org() {
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, alice_org, _) = provision(&server, &alice).await;
        let (_, bob_org, _) = provision(&server, &bob).await;

        let res = server
            .get("/api/v1/system/orgs?limit=100")
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 403);

        make_platform_admin(&ctx.db, alice_id).await;
        let res = server
            .get("/api/v1/system/orgs?limit=100")
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert!(body["count"].as_i64().unwrap() >= 2, "paginated envelope");
        let rows = body["results"].as_array().cloned().unwrap();
        let ids: Vec<i64> = rows.iter().map(|r| r["org_id"].as_i64().unwrap()).collect();
        // The admin sees organizations they are not a member of.
        assert!(ids.contains(&alice_org) && ids.contains(&bob_org));
        let bob_row = rows.iter().find(|r| r["org_id"] == bob_org).unwrap();
        assert!(bob_row["retention_days"].as_u64().unwrap() >= 1);
        // Self-hosted: no subscription quota.
        assert!(bob_row["quota_bytes"].is_null());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_tier_is_changed_by_platform_admins_only() {
    use pnex_backend::models::_entities::{organizations, subscription_tiers};
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, _, _) = provision(&server, &alice).await;
        let (_, bob_org, _) = provision(&server, &bob).await;
        let now = chrono::Utc::now().into();
        let tier = subscription_tiers::ActiveModel {
            created_at: Set(now),
            updated_at: Set(now),
            name: Set("o37-test-tier".into()),
            max_sensor_devices: Set(1),
            max_actuator_devices: Set(1),
            max_mixed_devices: Set(1),
            min_build_interval_secs: Set(0),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();

        // Owner of their own org, but not platform admin: refused (R2).
        let res = server
            .put(&format!("/api/v1/system/orgs/{bob_org}/tier"))
            .add_header("Authorization", bearer(&bob))
            .json(&serde_json::json!({ "tier_id": tier.id }))
            .await;
        assert_eq!(res.status_code(), 403);
        let res = server
            .get("/api/v1/system/tiers?limit=100")
            .add_header("Authorization", bearer(&bob))
            .await;
        assert_eq!(res.status_code(), 403);

        make_platform_admin(&ctx.db, alice_id).await;
        let res = server
            .get("/api/v1/system/tiers?limit=100")
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let tiers = res.json::<serde_json::Value>()["results"]
            .as_array()
            .cloned()
            .unwrap();
        assert!(tiers.iter().any(|t| t["id"] == tier.id));

        let res = server
            .put(&format!("/api/v1/system/orgs/{bob_org}/tier"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "tier_id": tier.id + 100_000 }))
            .await;
        assert_eq!(res.status_code(), 422);
        assert_eq!(res.json::<serde_json::Value>()["error"], "tier-unknown");

        let res = server
            .put(&format!("/api/v1/system/orgs/{bob_org}/tier"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "tier_id": tier.id }))
            .await;
        assert_eq!(res.status_code(), 200);
        let org = organizations::Entity::find_by_id(bob_org)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(org.subscription_tier_id, Some(tier.id));
        let res = server
            .get("/api/v1/system/orgs?limit=100")
            .add_header("Authorization", bearer(&alice))
            .await;
        let rows = res.json::<serde_json::Value>()["results"]
            .as_array()
            .cloned()
            .unwrap();
        let row = rows.iter().find(|r| r["org_id"] == bob_org).unwrap();
        assert_eq!(row["tier_id"], tier.id);
        assert_eq!(row["tier_name"], "o37-test-tier");

        // `null` removes the tier.
        let res = server
            .put(&format!("/api/v1/system/orgs/{bob_org}/tier"))
            .add_header("Authorization", bearer(&alice))
            .json(&serde_json::json!({ "tier_id": null }))
            .await;
        assert_eq!(res.status_code(), 200);
        let org = organizations::Entity::find_by_id(bob_org)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(org.subscription_tier_id, None);
    })
    .await;
}

/// R9 (D755): the global limits of hosted agents are edited by a platform admin only,
/// checked on write, and read at use time by the next build and upload.
/// The upload part needs the test S3 (`docker compose up -d rustfs rustfs-init`).
#[tokio::test]
#[serial]
async fn agent_limits_are_edited_by_platform_admins_and_apply_at_once() {
    with_app(|server, ctx, alice, bob| async move {
        let (alice_id, org_id, _) = provision(&server, &alice).await;
        provision(&server, &bob).await;
        make_platform_admin(&ctx.db, alice_id).await;
        let path = "/api/v1/system/agent-limits";
        // Other suites rely on the configured limits: none stored before or after.
        let forget = || async {
            use sea_orm::ConnectionTrait;
            ctx.db
                .execute_unprepared("DELETE FROM system_settings WHERE key = 'agent_limits'")
                .await
                .unwrap();
        };
        forget().await;

        let res = server
            .get(path)
            .add_header("Authorization", bearer(&bob))
            .await;
        assert_eq!(res.status_code(), 403);
        let res = server
            .get(path)
            .add_header("Authorization", bearer(&alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let info: serde_json::Value = res.json();
        // The test configuration sets the artifact sizes; the rest are code defaults.
        assert_eq!(info["defaults"]["artifact_max_bytes"], 65536, "{info}");
        assert_eq!(info["defaults"]["build_memory"], "8Gi");
        let mut limits = info["defaults"].clone();

        limits["build_memory"] = "4Gi".into();
        let res = server
            .put(path)
            .add_header("Authorization", bearer(&bob))
            .json(&limits)
            .await;
        assert_eq!(res.status_code(), 403);

        let mut bad = limits.clone();
        bad["build_ephemeral_storage"] = "lots".into();
        bad["job_logs_days"] = 0.into();
        let res = server
            .put(path)
            .add_header("Authorization", bearer(&alice))
            .json(&bad)
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(
            body["errors"]["fields"]["build_ephemeral_storage"], "invalid",
            "{body}"
        );
        assert_eq!(body["errors"]["fields"]["job_logs_days"], "range:1..3650");

        limits["artifact_max_bytes"] = (1u64 << 20).into();
        limits["artifact_run_max_bytes"] = (1u64 << 20).into();
        let res = server
            .put(path)
            .add_header("Authorization", bearer(&alice))
            .json(&limits)
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let info: serde_json::Value = res.json();
        assert_eq!(info["overridden"], true);
        assert_eq!(info["limits"]["build_memory"], "4Gi");

        // The next build pod and the next upload use the stored values.
        let build = pnex_backend::services::fabric_settings::build(&ctx).await;
        assert_eq!(build.memory, "4Gi");
        let upload = |size: usize| {
            server
                .post("/api/v1/agent-artifacts?name=big")
                .add_header("Authorization", bearer(&alice))
                .add_header("X-Org-Id", org_id.to_string())
                .bytes(vec![7u8; size].into())
        };
        let res = upload(70_000).await;
        assert!(res.status_code().is_success(), "{}", res.text());

        limits["artifact_max_bytes"] = 65536.into();
        let res = server
            .put(path)
            .add_header("Authorization", bearer(&alice))
            .json(&limits)
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(upload(70_000).await.status_code().as_u16(), 413);
        forget().await;
    })
    .await;
}
