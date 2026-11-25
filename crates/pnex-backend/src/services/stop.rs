//! Clean stop of the pod (pipelines.md §3.13): on SIGTERM it starts no new
//! run; the sweeps and schedulers of the other pods take over.

use std::sync::atomic::{AtomicBool, Ordering};

static STOPPING: AtomicBool = AtomicBool::new(false);

/// The pod is stopping: no new run.
pub fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

/// Stops starting runs.
pub fn shutdown() {
    STOPPING.store(true, Ordering::Relaxed);
}
