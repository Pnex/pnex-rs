//! Database consistency under concurrency (horizontal scaling, workstream D):
//! per-org advisory locks, compare-and-set OTA transitions, the "one active
//! OTA assignment per device" index, and optimistic version saves that
//! answer a conflict (never a 500, never a silently overwritten restore).
//!
//! Requires PostgreSQL (TEST_DATABASE_URL): the races are only arbitrated
//! there (advisory locks and row locks are no-ops on sqlite).

mod common;

use std::time::Duration;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::models::_entities::ota_assignments;
use pnex_backend::services::db_lock::{self, TenantLock};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    // Engine OFF: a deploy answers 503 once past the gates (no runtime).
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-scale-consistency-{}", std::process::id()),
        )
    };
    unsafe { std::env::set_var("PNEX_FLOW_RELOAD_ACK_SECS", "2") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx).await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn create_device(server: &axum_test::TestServer, auth: &str, org: i64, slug: &str) -> i64 {
    let res = server
        .post("/api/v1/devices")
        .add_header("Authorization", format!("Bearer {auth}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": slug,
            "predefined_device_name": "generic_esp8266",
        }))
        .await;
    res.assert_status(axum_test::http::StatusCode::CREATED);
    res.json::<serde_json::Value>()["id"].as_i64().expect("id")
}

fn ota_row(org: i64, device: i64, state: &str) -> ota_assignments::ActiveModel {
    ota_assignments::ActiveModel {
        org_id: Set(org),
        device_registry_id: Set(device),
        target_version: Set("1.0.0".into()),
        artifact_key: Set("k".into()),
        sha256: Set(String::new()),
        size_bytes: Set(None),
        state: Set(state.into()),
        progress: Set(None),
        error: Set(None),
        cmd_id: Set(Some("c1".into())),
        ..Default::default()
    }
}

fn graph_json() -> serde_json::Value {
    serde_json::json!({ "nodes": [{ "id": "n1", "kind": "debug", "config": {} }] })
}

fn is_postgres() -> bool {
    std::env::var("TEST_DATABASE_URL").is_ok_and(|u| u.starts_with("postgres"))
}

#[tokio::test]
#[serial]
async fn tenant_lock_serializes_same_key_only() {
    if !is_postgres() {
        return;
    }
    with_app(|_server, _auth, ctx| async move {
        let held = TenantLock::acquire(
            &ctx.db,
            db_lock::ns::FLOW_DEPLOY,
            42,
            Duration::from_secs(5),
        )
        .await
        .expect("first acquire");
        // Same key: times out while held.
        let err = TenantLock::acquire(
            &ctx.db,
            db_lock::ns::FLOW_DEPLOY,
            42,
            Duration::from_millis(200),
        )
        .await
        .err()
        .expect("second acquire must time out");
        assert!(db_lock::is_lock_timeout(&err), "unexpected error: {err}");
        // Other org / other namespace: granted immediately.
        TenantLock::acquire(
            &ctx.db,
            db_lock::ns::FLOW_DEPLOY,
            43,
            Duration::from_millis(200),
        )
        .await
        .expect("other org")
        .release()
        .await;
        TenantLock::acquire(
            &ctx.db,
            db_lock::ns::REGULATOR,
            42,
            Duration::from_millis(200),
        )
        .await
        .expect("other namespace")
        .release()
        .await;
        held.release().await;
        TenantLock::acquire(
            &ctx.db,
            db_lock::ns::FLOW_DEPLOY,
            42,
            Duration::from_millis(200),
        )
        .await
        .expect("released")
        .release()
        .await;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn flow_deploy_waits_for_the_org_lock() {
    if !is_postgres() {
        return;
    }
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let res = server
            .post("/api/v1/flows")
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({ "name": "locked", "graph": graph_json() }))
            .await;
        res.assert_status(axum_test::http::StatusCode::CREATED);
        let flow_id = res.json::<serde_json::Value>()["id"].as_i64().expect("id");

        // Another pod holds the org's deploy lock: the deploy must wait.
        let held = TenantLock::acquire(
            &ctx.db,
            db_lock::ns::FLOW_DEPLOY,
            org,
            Duration::from_secs(5),
        )
        .await
        .expect("lock");
        let blocked = tokio::time::timeout(
            Duration::from_millis(800),
            server
                .post(&format!("/api/v1/flows/{flow_id}/deploy"))
                .add_header("Authorization", format!("Bearer {auth}"))
                .add_header("X-Org-Id", org.to_string())
                .json(&serde_json::json!({})),
        )
        .await;
        if let Ok(res) = &blocked {
            panic!(
                "deploy went through while the org lock was held: {} {}",
                res.status_code(),
                res.text()
            );
        }
        held.release().await;

        // Released: the deploy proceeds (503 = engine off, past the gates).
        let res = server
            .post(&format!("/api/v1/flows/{flow_id}/deploy"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .json(&serde_json::json!({}))
            .await;
        assert_ne!(
            res.status_code(),
            axum_test::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ota_one_active_assignment_per_device() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_device(&server, &auth, org, "ota-uniq").await;
        ota_row(org, dev, "pending")
            .insert(&ctx.db)
            .await
            .expect("first active row");
        let err = ota_row(org, dev, "downloading")
            .insert(&ctx.db)
            .await
            .err()
            .expect("second active row must be rejected");
        assert!(
            db_lock::is_unique_violation(&err),
            "unexpected error: {err}"
        );
        // Terminal rows are not constrained.
        ota_row(org, dev, "failed")
            .insert(&ctx.db)
            .await
            .expect("terminal row");
        ota_row(org, dev, "succeeded")
            .insert(&ctx.db)
            .await
            .expect("terminal row");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ota_transition_applies_once() {
    with_app(|server, auth, ctx| async move {
        use pnex_backend::services::ota;
        let org = personal_org(&server, &auth).await;
        let dev = create_device(&server, &auth, org, "ota-cas").await;
        let row = ota_row(org, dev, ota::ST_DOWNLOADING)
            .insert(&ctx.db)
            .await
            .expect("row");
        // Two concurrent writers both read `downloading` (watchdog vs device
        // failure report): only the first transition applies.
        let first = ota::try_transition(
            &ctx.db,
            row.clone(),
            "ota-cas",
            ota::ST_FAILED,
            None,
            Some("timeout"),
        )
        .await
        .expect("first");
        assert!(first.is_some(), "first transition must apply");
        let second = ota::try_transition(
            &ctx.db,
            row.clone(),
            "ota-cas",
            ota::ST_FAILED,
            None,
            Some("device error"),
        )
        .await
        .expect("second");
        assert!(second.is_none(), "stale transition must not apply");
        // `transition` keeps its contract: returns the CURRENT row.
        let current = ota::transition(&ctx.db, row, "ota-cas", ota::ST_SUCCEEDED, Some(100), None)
            .await
            .expect("transition");
        assert_eq!(current.state, ota::ST_FAILED);
        assert_eq!(current.error.as_deref(), Some("timeout"));
        let stored = ota_assignments::Entity::find_by_id(current.id)
            .one(&ctx.db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ota::ST_FAILED);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ota_deploy_conflict_is_409() {
    with_app(|server, auth, ctx| async move {
        let org = personal_org(&server, &auth).await;
        let dev = create_device(&server, &auth, org, "ota-409").await;
        ota_row(org, dev, "pending")
            .insert(&ctx.db)
            .await
            .expect("active row");
        let res = server
            .post(&format!("/api/v1/devices/{dev}/ota"))
            .add_header("Authorization", format!("Bearer {auth}"))
            .add_header("X-Org-Id", org.to_string())
            .await;
        res.assert_status(axum_test::http::StatusCode::CONFLICT);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn dashboard_save_never_overwrites_a_concurrent_restore() {
    with_app(|server, auth, ctx| async move {
        use pnex_backend::services::dashboards::{self, DashboardWriteError};
        let org = personal_org(&server, &auth).await;
        let (d1, _) = dashboards::create_dashboard(&ctx.db, org, "race", None, None, None)
            .await
            .expect("create");
        let layout = pnex_core::DashboardLayout {
            canvas: pnex_core::CanvasSpec {
                width: 1600,
                height: 900,
                background: None,
            },
            widgets: vec![],
            wires: vec![],
            ..Default::default()
        };
        let (d2, v2) = dashboards::append_version(&ctx.db, &d1, 1, &layout, None, None)
            .await
            .expect("v2");
        assert_eq!(v2, 2);
        // Another pod restores v1 while this handler still holds `d2`
        // (pointer 2 in memory): the stale save must be a conflict.
        dashboards::restore_version(&ctx.db, &d2, 1)
            .await
            .expect("restore");
        let res = dashboards::append_version(&ctx.db, &d2, 2, &layout, None, None).await;
        assert!(
            matches!(
                res,
                Err(DashboardWriteError::Conflict {
                    expected: 2,
                    current: 1
                })
            ),
            "stale save must conflict"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn concurrent_flow_saves_conflict_instead_of_500() {
    if !is_postgres() {
        return;
    }
    with_app(|server, auth, ctx| async move {
        use pnex_backend::services::flow::{self, FlowWriteError};
        let org = personal_org(&server, &auth).await;
        let graph: pnex_core::FlowGraph = serde_json::from_value(graph_json()).unwrap();
        let by = pnex_backend::services::secrets::flow::GraphWriter {
            ring: None,
            user_id: None,
            can_write_secrets: false,
        };
        let (f, _, _) = flow::create_flow(&ctx.db, org, "race", &graph, None, None, None, &by)
            .await
            .expect("create");
        let mut wins = 0;
        let mut conflicts = 0;
        let results = futures_util::future::join_all((0..6).map(|_| {
            let db = ctx.db.clone();
            let f = f.clone();
            let graph = graph.clone();
            async move {
                let by = pnex_backend::services::secrets::flow::GraphWriter {
                    ring: None,
                    user_id: None,
                    can_write_secrets: false,
                };
                flow::append_version(&db, &f, 1, &graph, None, None, None, &by).await
            }
        }))
        .await;
        for r in results {
            match r {
                Ok(_) => wins += 1,
                Err(FlowWriteError::Conflict { .. }) => conflicts += 1,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        assert_eq!(wins, 1, "exactly one save of v1 wins");
        assert_eq!(conflicts, 5);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn boot_migrations_serialize_under_the_lock() {
    if !is_postgres() {
        return;
    }
    with_app(|_server, _auth, _ctx| async move {
        let uri = std::env::var("TEST_DATABASE_URL").unwrap();
        // Two pods booting together: both wait for the lock in turn and
        // find nothing left to apply (the app boot already migrated).
        let (a, b) = tokio::join!(
            db_lock::migrate_under_lock::<pnex_migration::Migrator>(&uri),
            db_lock::migrate_under_lock::<pnex_migration::Migrator>(&uri),
        );
        assert!(a.expect("pod a"));
        assert!(b.expect("pod b"));
        // Off Postgres the framework path is kept.
        assert!(
            !db_lock::migrate_under_lock::<pnex_migration::Migrator>("sqlite://x.db?mode=rwc")
                .await
                .unwrap()
        );
    })
    .await;
}
