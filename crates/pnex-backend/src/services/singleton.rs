//! Cluster-wide singleton background sweeps (D106): with several pods,
//! each periodic sweep (liveness reaper, OTA watchdog, video pruner, O2
//! retention reconcile) runs on ONE pod — the holder of its row lease
//! `task:<name>`, renewed at every tick, taken over by another pod once it
//! expires. Single node: always the holder.

use std::time::Duration;

use sea_orm::DatabaseConnection;

/// Is it this pod's turn to run the sweep `name` (period `every`)? The
/// lease lasts three periods. A database error skips the round (the sweep
/// needs the database anyway).
pub async fn my_turn(db: &DatabaseConnection, name: &str, every: Duration) -> bool {
    let ttl_ms = (every.as_millis() as u64).saturating_mul(3).max(3_000);
    match crate::services::flow_cluster::store::try_lease(
        db,
        &format!("task:{name}"),
        crate::services::device_bus::pod_id(),
        ttl_ms,
    )
    .await
    {
        Ok(mine) => mine,
        Err(e) => {
            tracing::warn!(task = name, error = %e, "singleton lease check failed — round skipped");
            false
        }
    }
}

/// Gives back every singleton lease of this pod (app `on_shutdown`): the
/// sweeps move to another pod at its next tick, not after three periods.
pub async fn release_all(db: &DatabaseConnection) {
    match crate::services::flow_cluster::store::release_task_leases(
        db,
        crate::services::device_bus::pod_id(),
    )
    .await
    {
        Ok(n) if n > 0 => tracing::info!(released = n, "singleton leases released"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "singleton lease release failed (they will expire)"),
    }
}
