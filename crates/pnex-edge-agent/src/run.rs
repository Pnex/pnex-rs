//! Agent runtime: queue + group-commit writer + uplink + local API, until
//! the shutdown future resolves (Ctrl-C, SIGTERM, service stop).

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::config::{self, QUEUE_FILE};
use crate::queue::Queue;
use crate::state::Shared;

/// Pending durable-write requests before the API answers 503.
const WRITER_BACKLOG: usize = 1024;
/// Queue bounds enforcement period.
const TRIM_EVERY: Duration = Duration::from_secs(600);

pub async fn run(dir: &Path, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
    let (cfg, secrets, ca) = config::load(dir)?;
    let queue = Queue::open(&dir.join(QUEUE_FILE))?;
    let (tx, rx) = mpsc::channel(WRITER_BACKLOG);
    let shared = Shared::new(queue.clone(), tx);
    let writer = crate::api::spawn_writer(queue.clone(), rx, std::sync::Arc::downgrade(&shared));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        device = %secrets.device_id,
        server = %cfg.server,
        backlog = queue.len().unwrap_or(0),
        "pnex-agent starting"
    );

    // Uplink (reconnects forever).
    let uplink = tokio::spawn(crate::uplink::run(shared.clone(), cfg.clone(), secrets, ca));

    // Queue bounds (oldest first) — never blocks ingestion.
    let trimmer = {
        let shared = shared.clone();
        let (max_points, max_age) = (cfg.max_queue_points, cfg.max_age_secs);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TRIM_EVERY);
            loop {
                tick.tick().await;
                let q = shared.queue.clone();
                let min_ts = chrono::Utc::now().timestamp_millis()
                    - i64::try_from(max_age.saturating_mul(1000)).unwrap_or(i64::MAX);
                match tokio::task::spawn_blocking(move || q.trim(max_points, min_ts)).await {
                    Ok(Ok(n)) if n > 0 => {
                        shared.dropped.fetch_add(n, Ordering::Relaxed);
                        tracing::warn!(dropped = n, "queue bounds reached — oldest points dropped");
                    }
                    Ok(Err(e)) => tracing::warn!("queue trim failed: {e}"),
                    _ => {}
                }
            }
        })
    };

    let addr = cfg.listen_addr()?;
    let allow = cfg.allow_nets()?;
    if !addr.ip().is_loopback() {
        tracing::warn!(
            %addr,
            allow = ?cfg.allow,
            "local API exposed beyond loopback without password (LAN opt-in)"
        );
    }
    let app = crate::api::router(shared.clone(), allow);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot bind the local API on {addr}"))?;
    tracing::info!("local API listening on http://{addr}");
    let sd = shared.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;
        sd.shutting_down.store(true, Ordering::Relaxed);
        tracing::info!("pnex-agent stopping");
    })
    .await
    .context("local API server failed")?;
    uplink.abort();
    trimmer.abort();
    let _ = uplink.await;
    let _ = trimmer.await;
    // Last strong handle: dropping it closes the writer channel, the writer
    // thread drains and exits, and the queue file is released.
    drop(shared);
    drop(queue);
    let _ = tokio::task::spawn_blocking(move || writer.join()).await;
    Ok(())
}

/// Ctrl-C or SIGTERM.
pub async fn os_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
