//! `pnex-runner`: enrols once with a join token, then polls for jobs until stopped.
//! A workload it leaves behind is destroyed at its next start; the server gives its
//! jobs to another runner when the lease runs out.

use std::path::PathBuf;
use std::time::Duration;

use pnex_runner::api::{self, HttpApi};
use pnex_runner::k8s::K8s;
use pnex_runner::podman::Podman;
use pnex_runner::{Host, Runner};
use tokio::signal::unix::{signal, SignalKind};

const TICK: Duration = Duration::from_secs(3);
const MAX_JOBS_DEFAULT: usize = 4;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn server_url() -> Result<String, String> {
    let url = env("PNEX_RUNNER_SERVER_URL")
        .ok_or_else(|| "PNEX_RUNNER_SERVER_URL is required".to_string())?;
    let url = url.trim_end_matches('/').to_string();
    let allow_http = env("PNEX_RUNNER_ALLOW_HTTP").as_deref() == Some("1");
    if url.starts_with("https://") || (allow_http && url.starts_with("http://")) {
        Ok(url)
    } else {
        Err(
            "PNEX_RUNNER_SERVER_URL must be https (PNEX_RUNNER_ALLOW_HTTP=1 for a dev server)"
                .into(),
        )
    }
}

/// The stored credentials, or a registration with the join token.
async fn credentials<H: Host>(host: &H, url: &str) -> Result<(String, String), String> {
    if let Some(found) = host.load_credentials().await? {
        return Ok(found);
    }
    let join = env("PNEX_RUNNER_JOIN_TOKEN")
        .ok_or_else(|| "not enrolled and PNEX_RUNNER_JOIN_TOKEN is not set".to_string())?;
    let registered = api::register(url, join.trim(), host.capabilities()).await?;
    host.save_credentials(&registered.runner_id, &registered.token)
        .await?;
    tracing::info!(runner = registered.runner_id, "runner enrolled");
    Ok((registered.runner_id, registered.token))
}

async fn serve<H: Host>(mut host: H, url: &str) -> Result<(), String> {
    let max_jobs = env("PNEX_RUNNER_MAX_JOBS")
        .and_then(|n| n.parse().ok())
        .unwrap_or(MAX_JOBS_DEFAULT);
    let (runner_id, token) = credentials(&host, url).await?;
    host.set_runner_id(runner_id.clone());
    let mut runner = Runner::new(HttpApi::new(url, token)?, host, max_jobs);
    let mut stop = signal(SignalKind::terminate()).map_err(|e| e.to_string())?;
    tracing::info!(runner = runner_id, max_jobs, "runner started");
    loop {
        runner.tick().await;
        tokio::select! {
            _ = stop.recv() => {
                tracing::info!("runner stopped");
                return Ok(());
            }
            _ = tokio::time::sleep(TICK) => {}
        }
    }
}

fn credentials_path() -> Result<PathBuf, String> {
    if let Some(path) = env("PNEX_RUNNER_CREDENTIALS") {
        return Ok(path.into());
    }
    let home =
        env("HOME").ok_or_else(|| "PNEX_RUNNER_CREDENTIALS or HOME is required".to_string())?;
    Ok(PathBuf::from(home).join(".config/pnex-runner/credentials.json"))
}

async fn run() -> Result<(), String> {
    let url = server_url()?;
    let runtime = env("PNEX_RUNNER_RUNTIME");
    let in_cluster = env("KUBERNETES_SERVICE_HOST").is_some();
    match env("PNEX_RUNNER_EXECUTOR").as_deref() {
        Some("kubernetes") => serve(k8s(runtime)?, &url).await,
        Some("podman") => serve(podman(runtime)?, &url).await,
        Some(other) => Err(format!("unknown PNEX_RUNNER_EXECUTOR: {other}")),
        None if in_cluster => serve(k8s(runtime)?, &url).await,
        None => serve(podman(runtime)?, &url).await,
    }
}

fn k8s(runtime: Option<String>) -> Result<K8s, String> {
    K8s::in_cluster(
        runtime,
        env("PNEX_RUNNER_BUILDS").as_deref() == Some("true"),
        env("PNEX_RUNNER_PULL_SECRET"),
        env("PNEX_RUNNER_JOB_EGRESS")
            .map(|v| {
                v.split(',')
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn podman(runtime: Option<String>) -> Result<Podman, String> {
    let cli = env("PNEX_RUNNER_CONTAINER_CLI").unwrap_or_else(|| "podman".to_string());
    Ok(Podman::new(cli, runtime, credentials_path()?))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    if let Err(e) = run().await {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}
