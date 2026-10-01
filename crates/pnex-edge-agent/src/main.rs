//! `pnex-agent` — PNeX edge agent (D95, docs/architecture/edge-agent.md).
//!
//! Local scripts push free-form values to a password-less HTTP API
//! (loopback by default); the agent stores them in a durable disk queue and
//! forwards them over the encrypted `/ws/device` tunnel, buffering through
//! slow, saturated or broken links.

use pnex_edge_agent::{config, install, run, service};

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pnex-agent", version, about = "PNeX edge agent")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enroll this machine with a code from the PNeX UI and install the service.
    Install {
        /// PNeX server URL, e.g. https://pnex.local
        #[arg(long)]
        server: String,
        /// Single-use enrollment code (XXXX-XXXX-XXXX).
        #[arg(long)]
        enroll: String,
        /// SHA-256 fingerprint of the server's local CA (printed by the UI).
        #[arg(long)]
        ca_sha256: Option<String>,
        /// Local API bind address (default 127.0.0.1:7070).
        #[arg(long)]
        listen: Option<String>,
        /// Client networks allowed on a LAN bind (CIDR, repeatable).
        #[arg(long)]
        allow: Vec<String>,
        /// Only write the configuration (run it yourself with `pnex-agent run`).
        #[arg(long)]
        no_service: bool,
        /// Per-user installation (no root): user directory + systemd --user.
        #[arg(long)]
        user: bool,
        /// Configuration/queue directory (default: platform location).
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Run the agent in the foreground (what the service executes).
    Run {
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Windows: started by the Service Control Manager.
        #[arg(long, hide = true)]
        service: bool,
    },
    /// Show the running agent status (local API).
    Status {
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Push one value through the running agent.
    Send {
        key: String,
        value: String,
        #[arg(long)]
        unit: Option<String>,
        /// Also record the value in PNeX history (OpenObserve).
        #[arg(long)]
        record: bool,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Stop and remove the service (`--purge` also deletes config + queue).
    Uninstall {
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        user: bool,
        #[arg(long)]
        dir: Option<PathBuf>,
    },
}

fn init_logs(dir: Option<&std::path::Path>) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let filter = tracing_subscriber::EnvFilter::try_from_env("PNEX_AGENT_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    match dir {
        Some(d) if std::fs::create_dir_all(d.join("logs")).is_ok() => {
            let appender = tracing_appender::rolling::daily(d.join("logs"), "pnex-agent.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .with(file)
                .init();
            Some(guard)
        }
        _ => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .init();
            None
        }
    }
}

/// Local API base URL from the installed configuration.
fn local_base(dir: &std::path::Path) -> Result<String> {
    let (cfg, _, _) = config::load(dir)?;
    Ok(format!(
        "http://{}",
        cfg.listen.replace("0.0.0.0", "127.0.0.1")
    ))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { dir, service } => {
            let dir = dir.unwrap_or_else(|| config::default_dir(false));
            let _guard = init_logs(Some(&dir));
            if service {
                #[cfg(windows)]
                return service::win::run(dir);
                #[cfg(not(windows))]
                anyhow::bail!("--service is only meaningful on Windows");
            }
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run::run(&dir, run::os_shutdown_signal()))
        }
        Cmd::Install {
            server,
            enroll,
            ca_sha256,
            listen,
            allow,
            no_service,
            user,
            dir,
        } => {
            let _guard = init_logs(None);
            let dir = dir.unwrap_or_else(|| config::default_dir(user));
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(install::install(
                &dir,
                install::InstallArgs {
                    server,
                    enroll,
                    ca_sha256,
                    listen,
                    allow,
                    no_service,
                    user_mode: user,
                },
            ))
        }
        Cmd::Status { dir } => {
            let dir = dir.unwrap_or_else(|| config::default_dir(false));
            let base = local_base(&dir)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let v: serde_json::Value = reqwest::get(format!("{base}/v1/status"))
                    .await
                    .with_context(|| {
                        format!("agent not reachable on {base} — is the service running?")
                    })?
                    .json()
                    .await?;
                println!("{}", serde_json::to_string_pretty(&v)?);
                Ok(())
            })
        }
        Cmd::Send {
            key,
            value,
            unit,
            record,
            dir,
        } => {
            let dir = dir.unwrap_or_else(|| config::default_dir(false));
            let base = local_base(&dir)?;
            let value = serde_json::from_str::<serde_json::Value>(&value)
                .unwrap_or(serde_json::Value::String(value));
            let body =
                serde_json::json!({"key": key, "value": value, "unit": unit, "record": record});
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let res = reqwest::Client::new()
                    .post(format!("{base}/v1/points"))
                    .json(&body)
                    .send()
                    .await
                    .with_context(|| format!("agent not reachable on {base}"))?;
                let status = res.status();
                let text = res.text().await.unwrap_or_default();
                if status.is_success() {
                    println!("{text}");
                    Ok(())
                } else {
                    anyhow::bail!("{status}: {text}")
                }
            })
        }
        Cmd::Uninstall { purge, user, dir } => {
            let dir = dir.unwrap_or_else(|| config::default_dir(user));
            service::uninstall(user)?;
            if purge {
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("cannot remove {}", dir.display()))?;
                println!("Removed {}", dir.display());
            }
            println!("pnex-agent uninstalled.");
            Ok(())
        }
    }
}
