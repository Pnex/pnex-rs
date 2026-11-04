//! Worker process of data pipelines (data-pipelines.md D573): takes runs from
//! the queue, one at a time, until SIGTERM gives its run back to the queue.

use std::time::Duration;

use pnex_pipeline_worker::{Config, Worker};
use tokio::signal::unix::{signal, SignalKind};

const IDLE: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let worker = Worker::connect(Config::from_env()?).await?;
    let mut stop = signal(SignalKind::terminate())?;
    tracing::info!("pipeline worker started");
    loop {
        tokio::select! {
            _ = stop.recv() => {
                worker.release().await;
                tracing::info!("pipeline worker stopped");
                return Ok(());
            }
            ran = worker.run_next() => match ran {
                Ok(true) => {}
                Ok(false) => tokio::time::sleep(IDLE).await,
                Err(e) => {
                    tracing::warn!("pipeline queue unavailable: {e}");
                    tokio::time::sleep(IDLE).await;
                }
            },
        }
    }
}
