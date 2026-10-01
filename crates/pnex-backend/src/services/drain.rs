//! Graceful drain of in-process background writers at shutdown.
//!
//! Buffered writers (O2 telemetry batcher, notify journal writer) keep
//! their sender in a process-wide static, so their channel never closes
//! and their "final flush on close" never ran: a rolling update lost the
//! last batch of every pod. Each writer registers here; the app
//! `on_shutdown` hook calls [`shutdown`], which signals every registered
//! writer and waits (bounded) until they all flushed and exited.
//!
//! Generation based: a stop request only concerns writers registered
//! before it, so a test binary booting several apps in one process is not
//! affected by a previous app's shutdown.

use std::sync::LazyLock;
use std::time::Duration;

use tokio::sync::watch;

/// Stop generation: bumped by every [`shutdown`].
static STOP: LazyLock<watch::Sender<u64>> = LazyLock::new(|| watch::channel(0).0);
/// Number of registered writers still running.
static ACTIVE: LazyLock<watch::Sender<usize>> = LazyLock::new(|| watch::channel(0).0);

/// A registered writer: poll [`Registration::stopped`] in its loop, flush,
/// then drop the registration (that is what [`shutdown`] waits for).
pub struct Registration {
    generation: u64,
    stop: watch::Receiver<u64>,
}

impl Registration {
    /// Resolves once a shutdown was requested after this registration.
    pub async fn stopped(&mut self) {
        let generation = self.generation;
        let _ = self.stop.wait_for(|g| *g > generation).await;
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        ACTIVE.send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// Registers one background writer.
pub fn register() -> Registration {
    ACTIVE.send_modify(|n| *n += 1);
    let stop = STOP.subscribe();
    let generation = *stop.borrow();
    Registration { generation, stop }
}

/// Signals every registered writer and waits until they all exited, at
/// most `timeout` (pending data is lost past it — the pod is going away).
pub async fn shutdown(timeout: Duration) {
    STOP.send_modify(|g| *g += 1);
    let mut active = ACTIVE.subscribe();
    if tokio::time::timeout(timeout, active.wait_for(|n| *n == 0))
        .await
        .is_err()
    {
        tracing::warn!(
            still_running = *active.borrow(),
            "shutdown drain timed out: some buffered writes are lost"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_waits_for_registered_writers_to_flush() {
        let flushed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = flushed.clone();
        let mut reg = register();
        tokio::spawn(async move {
            reg.stopped().await;
            // Simulated final flush.
            tokio::time::sleep(Duration::from_millis(50)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            drop(reg);
        });
        shutdown(Duration::from_secs(5)).await;
        assert!(flushed.load(std::sync::atomic::Ordering::SeqCst));
        // A writer registered after that shutdown is not stopped by it.
        let mut late = register();
        let stopped = tokio::time::timeout(Duration::from_millis(50), late.stopped()).await;
        assert!(
            stopped.is_err(),
            "a later registration must not see an old stop"
        );
    }
}
