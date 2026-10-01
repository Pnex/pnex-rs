//! Per-pod concurrency caps for CPU/memory heavy work.
//!
//! Every pool is a process-wide semaphore sized from an environment
//! variable (read once, lazily). A caller waits at most `wait` for a slot;
//! past that the request fails fast with `503 server-busy` instead of piling
//! up blocking threads, memory or child processes on one pod. The limits are
//! **per pod**: N replicas allow N × the configured value cluster-wide.
//!
//! | Pool | Env (slots) | Default | Env (wait ms) | Default |
//! |---|---|---|---|---|
//! | CoolProp (thermo diagrams, mixtures) | `PNEX_COOLPROP_MAX_CONCURRENT` | 2 | `PNEX_COOLPROP_WAIT_MS` | 10000 |
//! | Vision inference (tract) | `PNEX_VISION_MAX_CONCURRENT` | CPUs/2 (≥1) | `PNEX_VISION_WAIT_MS` | 15000 |
//! | Runtime checks (function test/check, deploy pre-flight) | `PNEX_RUNTIME_CHECK_MAX_CONCURRENT` | 4 | `PNEX_RUNTIME_CHECK_WAIT_MS` | 5000 |
//! | Large uploads (media, versions, stitch frames, video segments) | `PNEX_UPLOAD_MAX_CONCURRENT` | 4 | `PNEX_UPLOAD_WAIT_MS` | 30000 |
//!
//! CoolProp is serialized by a global FFI mutex anyway: its pool only
//! bounds the queue so a burst of diagram requests does not park every
//! blocking-pool thread behind that mutex.

use std::sync::OnceLock;
use std::time::Duration;

use axum::http::StatusCode;
use loco_rs::prelude::*;
use pnex_core::err_codes;
use tokio::sync::{Semaphore, SemaphorePermit};

/// One named, bounded pool.
pub struct Pool {
    name: &'static str,
    slots: usize,
    wait: Duration,
    sem: Semaphore,
}

/// The pool stayed saturated for the whole wait budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Saturated {
    pub pool: &'static str,
}

impl std::fmt::Display for Saturated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} pool saturated on this pod, retry later", self.pool)
    }
}

impl std::error::Error for Saturated {}

impl From<Saturated> for Error {
    fn from(s: Saturated) -> Self {
        busy_error(s)
    }
}

/// `503 server-busy` with the canonical English description.
pub fn busy_error(s: Saturated) -> Error {
    Error::CustomError(
        StatusCode::SERVICE_UNAVAILABLE,
        loco_rs::controller::ErrorDetail::new(err_codes::SERVER_BUSY, s.to_string()),
    )
}

impl Pool {
    /// Builds a pool (exposed for tests; production code uses the
    /// `coolprop()`/`vision()`/... accessors).
    pub fn new(name: &'static str, slots: usize, wait: Duration) -> Self {
        let slots = slots.max(1);
        Self {
            name,
            slots,
            wait,
            sem: Semaphore::new(slots),
        }
    }

    /// Waits up to the pool's budget for a slot.
    pub async fn acquire(&self) -> std::result::Result<SemaphorePermit<'_>, Saturated> {
        match tokio::time::timeout(self.wait, self.sem.acquire()).await {
            Ok(Ok(permit)) => Ok(permit),
            _ => {
                tracing::warn!(
                    pool = self.name,
                    slots = self.slots,
                    "compute pool saturated, rejecting request"
                );
                Err(Saturated { pool: self.name })
            }
        }
    }

    /// Non-blocking attempt (no wait at all).
    pub fn try_acquire(&self) -> std::result::Result<SemaphorePermit<'_>, Saturated> {
        self.sem
            .try_acquire()
            .map_err(|_| Saturated { pool: self.name })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn slots(&self) -> usize {
        self.slots
    }

    pub fn available(&self) -> usize {
        self.sem.available_permits()
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

fn env_wait(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(default_ms),
    )
}

fn cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
}

/// CoolProp jobs (thermo diagram/cycle, mixture validation).
pub fn coolprop() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| {
        Pool::new(
            "coolprop",
            env_usize("PNEX_COOLPROP_MAX_CONCURRENT", 2),
            env_wait("PNEX_COOLPROP_WAIT_MS", 10_000),
        )
    })
}

/// Vision model inference / load.
pub fn vision() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| {
        Pool::new(
            "vision",
            env_usize("PNEX_VISION_MAX_CONCURRENT", (cpus() / 2).max(1)),
            env_wait("PNEX_VISION_WAIT_MS", 15_000),
        )
    })
}

/// Short-lived runtime child processes: function test/check and the deploy
/// pre-flight `--check` share this cap.
pub fn runtime_check() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| {
        Pool::new(
            "runtime-check",
            env_usize("PNEX_RUNTIME_CHECK_MAX_CONCURRENT", 4),
            env_wait("PNEX_RUNTIME_CHECK_WAIT_MS", 5_000),
        )
    })
}

/// Large buffered uploads (each holds up to the body limit in memory).
pub fn upload() -> &'static Pool {
    static P: OnceLock<Pool> = OnceLock::new();
    P.get_or_init(|| {
        Pool::new(
            "upload",
            env_usize("PNEX_UPLOAD_MAX_CONCURRENT", 4),
            env_wait("PNEX_UPLOAD_WAIT_MS", 30_000),
        )
    })
}

/// Route middleware bounding concurrent large uploads per pod. It wraps
/// the handler **including** its body extraction, so at most
/// `PNEX_UPLOAD_MAX_CONCURRENT` bodies are buffered at once. Safe methods
/// (GET/HEAD/OPTIONS) sharing the route pass through untouched.
pub async fn upload_gate(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::Method;
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return next.run(req).await;
    }
    match upload().acquire().await {
        Ok(_permit) => next.run(req).await,
        Err(s) => busy_error(s).into_response(),
    }
}

/// Runs a CoolProp closure on the blocking pool under the CoolProp cap.
pub async fn run_coolprop<T, F>(f: F) -> std::result::Result<T, Error>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let _permit = coolprop().acquire().await?;
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::string(&format!("coolprop task failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saturated_pool_rejects_after_wait() {
        let pool = Pool::new("test", 1, Duration::from_millis(20));
        let held = pool.acquire().await.expect("first slot");
        let err = pool.acquire().await.expect_err("pool is full");
        assert_eq!(err.pool, "test");
        drop(held);
        assert!(pool.acquire().await.is_ok());
    }

    #[test]
    fn zero_slots_is_clamped_to_one() {
        let pool = Pool::new("test", 0, Duration::from_millis(1));
        assert_eq!(pool.slots(), 1);
        assert!(pool.try_acquire().is_ok());
    }

    #[test]
    fn busy_error_is_503_server_busy() {
        match busy_error(Saturated { pool: "x" }) {
            Error::CustomError(status, detail) => {
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(detail.error.as_deref(), Some(err_codes::SERVER_BUSY));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
