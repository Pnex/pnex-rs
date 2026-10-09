//! Cross-pod serialization helpers on the database (horizontal scaling,
//! docs/architecture/horizontal-scaling.md).
//!
//! Several API pods share one Postgres: in-process mutexes no longer
//! serialize anything. Read-check-write sequences that must be atomic per
//! tenant (deploy gates, quotas, materialized projections) take a
//! transaction-scoped advisory lock instead: `pg_advisory_xact_lock(ns, key)`
//! is released on commit/rollback and when the connection dies, so a
//! crashed pod never leaves a lock behind.
//!
//! On sqlite (tests, single node) every helper is a no-op: there is one
//! process, and the test pools may hold a single connection — opening a
//! second transaction there would deadlock the caller.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, SqlErr,
    Statement, TransactionTrait,
};

/// Lock namespaces (first int4 of the two-key advisory lock form). The
/// second key is the tenant id (org) or 0 for a global lock.
pub mod ns {
    /// Flow deploy/stop/delete of one org (pin exclusivity gate + apply +
    /// mark deployed).
    pub const FLOW_DEPLOY: i32 = 0x504e_0001;
    /// Regulator projection rewrite of one org.
    pub const REGULATOR: i32 = 0x504e_0002;
    /// Device quota (count + insert) of one org.
    pub const DEVICE_QUOTA: i32 = 0x504e_0003;
    /// Firmware build quota (count + min interval + insert) of one org.
    pub const BUILD_QUOTA: i32 = 0x504e_0004;
    // 0x504e_0005 was the pre-release secrets takeover: never reuse it.
    /// Master key rotation of the vault (global, key 0).
    pub const SECRETS_REKEY: i32 = 0x504e_0006;
    /// First-use creation of the OTA signing key (global, key 0).
    pub const OTA_SIGNING_KEY: i32 = 0x504e_0007;
    /// First-use creation of the device CA of one org (key = org id).
    pub const DEVICE_CA: i32 = 0x504e_0008;
}

/// Advisory lock key used by the boot migration (single int8 form).
pub const MIGRATION_LOCK_KEY: i64 = 0x504e_4558_4d49_4752; // "PNEXMIGR"

/// Folds an i64 tenant id into the int4 second key (collisions only between
/// ids 2^31 apart: harmless, they would merely serialize together).
fn key32(key: i64) -> i32 {
    (key.rem_euclid(i32::MAX as i64)) as i32
}

/// Takes `pg_advisory_xact_lock(ns, key)` on `conn`, which must be a
/// transaction (the lock lives until it ends). No-op off Postgres.
pub async fn xact_lock<C: ConnectionTrait>(conn: &C, ns: i32, key: i64) -> Result<(), DbErr> {
    if conn.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(());
    }
    conn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_xact_lock($1, $2)",
        [ns.into(), key32(key).into()],
    ))
    .await?;
    Ok(())
}

/// A held tenant lock: a dedicated transaction whose only purpose is to
/// carry the advisory lock while the caller works on its usual
/// connections (long operations such as a runtime acknowledgement can run
/// under it without pinning the business rows). `None` off Postgres.
pub struct TenantLock {
    txn: Option<DatabaseTransaction>,
}

impl TenantLock {
    /// Blocks until the lock `(ns, key)` is granted, or fails after
    /// `timeout` (`lock_timeout`) so a stuck holder cannot pile up
    /// requests forever.
    pub async fn acquire(
        db: &DatabaseConnection,
        ns: i32,
        key: i64,
        timeout: std::time::Duration,
    ) -> Result<Self, DbErr> {
        if db.get_database_backend() != DatabaseBackend::Postgres {
            return Ok(Self { txn: None });
        }
        let txn = db.begin().await?;
        // SET LOCAL does not take bind parameters.
        txn.execute_unprepared(&format!(
            "SET LOCAL lock_timeout = '{}ms'",
            timeout.as_millis().max(1)
        ))
        .await?;
        xact_lock(&txn, ns, key).await?;
        Ok(Self { txn: Some(txn) })
    }

    /// Releases the lock (commits the empty carrier transaction). Dropping
    /// the guard also releases it (rollback), this is just explicit.
    pub async fn release(self) {
        if let Some(txn) = self.txn {
            if let Err(e) = txn.commit().await {
                tracing::warn!("tenant lock release failed (rolled back by the pool): {e}");
            }
        }
    }
}

/// True when `e` is a unique constraint violation (Postgres or sqlite).
pub fn is_unique_violation(e: &DbErr) -> bool {
    matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_)))
}

/// True when `e` is a Postgres `lock_not_available` (55P03, lock_timeout).
pub fn is_lock_timeout(e: &DbErr) -> bool {
    e.to_string().contains("55P03") || e.to_string().contains("lock timeout")
}

/// Runs the boot migration (`M::up`) under a SESSION advisory lock held on
/// a dedicated single-connection pool, so N pods booting together migrate
/// one at a time: the first applies the pending migrations, the others
/// wait, then find nothing left to do. Returns `Ok(false)` off Postgres
/// (the caller keeps the framework's own migration path).
///
/// Loco runs `auto_migrate` inside `create_app`, before any hook: the app
/// `boot` hook calls this first, then disables `auto_migrate` for loco.
pub async fn migrate_under_lock<M: pnex_migration::MigratorTrait>(
    uri: &str,
) -> Result<bool, DbErr> {
    if !(uri.starts_with("postgres://") || uri.starts_with("postgresql://")) {
        return Ok(false);
    }
    let mut opts = sea_orm::ConnectOptions::new(uri.to_string());
    // One connection: every statement (lock, migrations, unlock) runs in
    // the same session, which owns the lock.
    opts.max_connections(1)
        .min_connections(1)
        .connect_timeout(std::time::Duration::from_secs(10))
        .sqlx_logging(false);
    let db = sea_orm::Database::connect(opts).await?;
    tracing::info!("boot migration: waiting for the migration lock");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT pg_advisory_lock($1)",
        [MIGRATION_LOCK_KEY.into()],
    ))
    .await?;
    tracing::info!("boot migration: lock acquired, applying pending migrations");
    let res = M::up(&db, None).await;
    // Released explicitly; closing the connection would release it too.
    let _ = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_unlock($1)",
            [MIGRATION_LOCK_KEY.into()],
        ))
        .await;
    let _ = db.close().await;
    res.map(|()| true)
}
