//! Clean stop of import runs (data-pipelines.md §7 bis, point 1). On
//! SIGTERM the pod starts no run; those it holds get [`grace`] to finish,
//! then are failed `import-run-interrupted` instead of left running until
//! the sweep finds them without a heartbeat.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use pnex_core::err_codes;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use tokio::sync::watch;
use uuid::Uuid;

/// What a run may take to finish once the pod stops:
/// `PNEX_SHUTDOWN_GRACE_SECS` (270 s by default), kept under the pod's
/// `terminationGracePeriodSeconds` (300 in the chart).
pub fn grace() -> Duration {
    let secs = std::env::var("PNEX_SHUTDOWN_GRACE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(270);
    Duration::from_secs(secs)
}

static STOPPING: AtomicBool = AtomicBool::new(false);
static RUNNING: LazyLock<Mutex<HashSet<Uuid>>> = LazyLock::new(Default::default);
/// Bumped whenever a run ends, so [`shutdown`] wakes up.
static ENDED: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);

/// The pod is stopping: no new run.
pub fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

/// A run held by this pod, for as long as the guard lives.
pub struct Held(Uuid);

pub fn hold(run: Uuid) -> Held {
    RUNNING.lock().expect("import runs").insert(run);
    Held(run)
}

impl Drop for Held {
    fn drop(&mut self) {
        RUNNING.lock().expect("import runs").remove(&self.0);
        ENDED.send_modify(|n| *n += 1);
    }
}

fn held() -> Vec<Uuid> {
    RUNNING
        .lock()
        .expect("import runs")
        .iter()
        .copied()
        .collect()
}

/// Waits at most `grace` until no run is held; returns those still held.
async fn wait_held(grace: Duration) -> Vec<Uuid> {
    let mut ended = ENDED.subscribe();
    let wait = async {
        while !held().is_empty() {
            if ended.changed().await.is_err() {
                return;
            }
        }
    };
    let _ = tokio::time::timeout(grace, wait).await;
    held()
}

/// Stops starting runs, waits at most `grace` for the held ones, then
/// fails those still running.
pub async fn shutdown(db: &DatabaseConnection, grace: Duration) {
    STOPPING.store(true, Ordering::Relaxed);
    let left = wait_held(grace).await;
    if left.is_empty() {
        return;
    }
    tracing::warn!(runs = left.len(), "import runs cut by the pod stop");
    let r = db
        .execute_raw(super::stmt(
            "UPDATE object_import_runs SET state = 'failed', state_code = $2, finished_at = now() \
             WHERE id = ANY($1) AND state = 'running'",
            vec![left.into(), err_codes::IMPORT_RUN_INTERRUPTED.into()],
        ))
        .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "interrupted import runs not recorded");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_run_ending_in_its_grace_is_waited_for_not_one_past_it() {
        let g = hold(Uuid::new_v4());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            drop(g);
        });
        assert!(wait_held(Duration::from_secs(5)).await.is_empty());
        let late = Uuid::new_v4();
        let g = hold(late);
        assert_eq!(wait_held(Duration::from_millis(50)).await, [late]);
        drop(g);
        assert!(held().is_empty());
    }
}
