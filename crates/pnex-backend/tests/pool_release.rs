//! A finished app gives its database connections back. The router in `ctx.shared_store`
//! once kept every pool of a test binary open: a suite of 25 tests held 60 connections,
//! Postgres refused the next one, and later tests failed in 503 `server-busy`.

mod common;

use loco_rs::testing::request::{request_with_config, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, Database};
use serial_test::serial;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Connections on the test database, seen from another one.
fn open_connections() -> i64 {
    runtime().block_on(async {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".into());
        let (server, name) = url.rsplit_once('/').unwrap();
        let db = Database::connect(format!("{server}/postgres"))
            .await
            .unwrap();
        let row = db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT count(*) AS c FROM pg_stat_activity WHERE datname = '{name}'"),
            ))
            .await
            .unwrap()
            .unwrap();
        let count = row.try_get::<i64>("", "c").unwrap();
        db.close().await.unwrap();
        count
    })
}

fn boot_and_query() {
    runtime().block_on(async {
        let base = common::spawn_mock_rauthy().await;
        unsafe { std::env::set_var("RAUTHY_URL", &base) };
        unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
        request_with_config::<App, _, _>(
            RequestConfigBuilder::new().build(),
            |server, ctx| async move {
                let _ = server.get("/api/v1/meta/version").await;
                ctx.db.execute_unprepared("SELECT 1").await.unwrap();
            },
        )
        .await;
    });
}

#[test]
#[serial]
fn a_finished_app_closes_its_connections() {
    let boots = |n| {
        for _ in 0..n {
            boot_and_query();
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
        open_connections()
    };
    // The first app stays referenced (`camera::DB`, set once per process): its pool counts
    // in both measures, and no later app may add to it.
    let before = boots(3);
    let after = boots(6);
    assert!(
        after <= before,
        "{before} connections after three apps, {after} after nine"
    );
}
