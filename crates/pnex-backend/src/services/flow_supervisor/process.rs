//! Runtime child process management: spawn, output pumping, reload signal, liveness.

use super::*;

/// Env of the runtime child (and of the pre-flight): allowlist + OpenObserve creds
/// injected from the yaml — identical for both, the check must see what
/// the child will see.
pub(crate) fn apply_runtime_env(cmd: &mut tokio::process::Command, settings: &FlowSettings) {
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env(
            "PNEX_FLOW_LOG",
            std::env::var("PNEX_FLOW_LOG").unwrap_or_else(|_| "info".into()),
        );
    // Same egress policy in the runtime as in the server (R8, http-fetch).
    cmd.env("PNEX_EGRESS", pnex_core::egress::policy().as_str());
    for key in ["PNEX_EGRESS_ALLOW_HOSTS", "PNEX_EGRESS_DENY_CIDRS"] {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    for key in &settings.env_allowlist {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    if let Some(o2) = &settings.o2 {
        cmd.env("OPENOBSERVE_URL", o2.base_url.clone())
            .env("OPENOBSERVE_ROOT_EMAIL", o2.root_email.clone())
            .env("OPENOBSERVE_ROOT_PASSWORD", o2.root_password.clone());
    }
    // Mirror of the O2 dedicated injection: the URL of the Valkey live
    // last-value cache comes from the backend yaml, beyond the allowlist.
    if let Some(url) = &settings.valkey_url {
        cmd.env("VALKEY_URL", url.clone());
    }
    // Fencing identity (D106): stamped by the runtime on its internal calls.
    if let Some(fence) = &settings.worker_fence {
        cmd.env(pnex_core::FLOW_WORKER_ENV, fence.clone());
    }
    if let Some((url, token)) = &settings.notify_deliver {
        cmd.env("PNEX_NOTIFY_DELIVER_URL", url.clone())
            .env("PNEX_NOTIFY_DELIVER_TOKEN", token.clone())
            // Delivery journal of the other channel kinds (D86): same host
            // and token as the websocket deliver endpoint.
            .env(
                "PNEX_NOTIFY_JOURNAL_URL",
                crate::services::settings::journal_url_of(url),
            );
    }
    if let Some((url, token)) = &settings.device_write {
        cmd.env("PNEX_FLOW_WRITE_URL", url.clone())
            .env("PNEX_FLOW_WRITE_TOKEN", token.clone())
            // Recorded video segments (video-record node, D78): same host
            // and service token as the device write endpoint.
            .env(
                "PNEX_FLOW_VIDEO_URL",
                url.replace(
                    "/internal/flow/device-write",
                    "/internal/flow/video-segment",
                ),
            )
            // Detections stored next to the video (video-record fed by
            // vision-detect, D105).
            .env(
                "PNEX_FLOW_ANNOTATION_URL",
                url.replace(
                    "/internal/flow/device-write",
                    "/internal/flow/video-annotations",
                ),
            )
            // JSON events (event-log node, D84).
            .env(
                "PNEX_FLOW_EVENT_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/event"),
            )
            // Time ranges (range-upsert node, D169).
            .env(
                "PNEX_FLOW_TIME_RANGE_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/time-range"),
            )
            // Vault secrets of deployed notify channels (D115): base of
            // `/internal/flow/secret/{id}`, same service token.
            .env(
                "PNEX_FLOW_SECRET_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/secret"),
            )
            // PDFs attached by pnex-notify nodes (D209): base of
            // `/internal/flow/media/{id}`.
            .env(
                "PNEX_FLOW_MEDIA_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/media"),
            )
            // Ontology nodes (D312): base of `/internal/flow/ontology/{query,upsert,link}`.
            .env(
                "PNEX_FLOW_ONTOLOGY_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/ontology"),
            )
            // Replay of stored mails (mail_received nodes, mail.md D464): base
            // of `/internal/flow/mail/{replay,ack}`.
            .env(
                "PNEX_FLOW_MAIL_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/mail"),
            )
            // Website navigation rules (site-recipes.md D327).
            .env(
                "PNEX_FLOW_SITE_RECIPE_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/site-recipe"),
            )
            // Web search candidates (web-presence.md D361).
            .env(
                "PNEX_FLOW_WEB_SEARCH_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/web-search"),
            )
            // One look at a candidate website (web-presence.md D364).
            .env(
                "PNEX_FLOW_WEB_VISIT_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/web-visit"),
            )
            // Server crawls of a site (network-check.md D407).
            .env(
                "PNEX_FLOW_SITE_CRAWL_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/site-crawl"),
            )
            // Company name → SIREN (account-mapping.md D332).
            .env(
                "PNEX_FLOW_ORG_RESOLVE_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/org-resolve"),
            )
            // Pages written by page-write nodes (D209).
            .env(
                "PNEX_FLOW_PAGE_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/page-write"),
            )
            // Org model calls (vision-caption / media-fuse nodes, D195/D196):
            // base of `/internal/flow/ai/{caption,fuse}`.
            .env(
                "PNEX_FLOW_AI_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/ai"),
            )
            // Registry models (vision-detect node, D83): base of
            // `/internal/flow/ml-model/{id}[/content]`.
            .env(
                "PNEX_FLOW_MODEL_URL",
                url.replace("/internal/flow/device-write", "/internal/flow/ml-model"),
            );
    }
}

/// Address-space limit of the runtime child from
/// `PNEX_FLOW_RUNTIME_MAX_MEMORY_MB` (unset, empty or 0 = no limit).
///
/// This is `RLIMIT_AS` (virtual memory): allocators and thread stacks
/// reserve more than they touch, so size it generously (≥ 2× the expected
/// RSS). No CPU-time limit is applied on purpose: `RLIMIT_CPU` counts
/// cumulative seconds and would eventually kill a healthy long-lived
/// runtime. For hard isolation run flow workers in their own pods
/// (`settings.flow.cluster.worker`), with Kubernetes requests/limits.
pub(crate) fn runtime_memory_limit_bytes() -> Option<libc::rlim_t> {
    parse_memory_limit_mb(
        std::env::var("PNEX_FLOW_RUNTIME_MAX_MEMORY_MB")
            .ok()
            .as_deref(),
    )
}

fn parse_memory_limit_mb(raw: Option<&str>) -> Option<libc::rlim_t> {
    let mb: u64 = raw?.trim().parse().ok()?;
    if mb == 0 {
        return None;
    }
    mb.checked_mul(1024 * 1024).map(|b| b as libc::rlim_t)
}

/// Spawns the child + stdout/stderr pumps → tracing. Returns `None` if the
/// launch fails (already logged).
pub(super) fn spawn_child(
    settings: &FlowSettings,
    flows_path: &std::path::Path,
    pending: PendingAcks,
) -> Option<ChildProc> {
    let program = resolve_program(&settings.runtime_cmd);
    tracing::info!(cmd=%program, flows=%flows_path.display(), "starting the flow runtime");
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg(flows_path)
        .arg("--home")
        .arg(state_dir_of(settings))
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    apply_runtime_env(&mut cmd, settings);
    // The runtime dies with its server (D106): a SIGKILLed pnex-server
    // outside a container would otherwise leave an orphan runtime running
    // the flows that the controller hands to another worker.
    //
    // Optional address-space cap (`PNEX_FLOW_RUNTIME_MAX_MEMORY_MB`, unset =
    // unlimited): the runtime shares the pod cgroup with the API, so a
    // runaway flow would otherwise OOM-kill the whole pod. Computed before
    // the fork: the pre_exec closure only performs async-signal-safe calls.
    #[cfg(target_os = "linux")]
    {
        let max_as = runtime_memory_limit_bytes();
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(bytes) = max_as {
                    let lim = libc::rlimit {
                        rlim_cur: bytes,
                        rlim_max: bytes,
                    };
                    if libc::setrlimit(libc::RLIMIT_AS, &lim) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(cmd=%program, error=%e, "cannot launch the flow runtime");
            return None;
        }
    };
    let pid = child.id().expect("pid at spawn");

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    tokio::spawn(pump_lines(stdout, tracing::Level::INFO, Some(pending)));
    tokio::spawn(pump_lines(stderr, tracing::Level::WARN, None));

    // End-of-life watcher: `child.wait()` in a dedicated task to keep the
    // supervision loop free (and reap the process — no zombie).
    let (exit_tx, exit_rx) = std_mpsc::channel();
    tokio::spawn(async move {
        let status = child.wait().await;
        let _ = exit_tx.send(status.map_or_else(|e| e.to_string(), |s| s.to_string()));
    });
    Some(ChildProc { pid, exit_rx })
}

async fn pump_lines(
    stream: impl tokio::io::AsyncRead + Unpin,
    level: tracing::Level,
    pending: Option<PendingAcks>,
) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Only stdout (INFO) is parsed: events consumed by the
        // feed/acks drop to `debug` to avoid INFO noise.
        if let Some(pending) = pending.as_ref().filter(|_| level == tracing::Level::INFO) {
            if handle_runtime_line(&line, pending) {
                tracing::debug!(runtime=%line, "flow runtime");
                continue;
            }
        }
        if level == tracing::Level::INFO {
            tracing::info!(runtime=%line, "flow runtime");
            continue;
        }
        // stderr carries the runtime's JSON log lines: re-emit each one at
        // its own level instead of flagging everything as a warning.
        match parse_runtime_log(&line) {
            Some(log) => emit_runtime_log(&log),
            None => tracing::warn!(runtime=%line, "flow runtime"),
        }
    }
}

/// One JSON log line of the runtime logger (`pnex-flow-runtime` `logger.rs`:
/// `{"ts","level","target","message"}`).
#[derive(Debug, PartialEq)]
pub(super) struct RuntimeLog {
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
}

/// Parses a runtime stderr line; `None` when it is not a JSON log line
/// (panic output, raw prints…) — the caller falls back to `warn`.
pub(super) fn parse_runtime_log(line: &str) -> Option<RuntimeLog> {
    let value: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let level = match value.get("level")?.as_str()?.to_ascii_uppercase().as_str() {
        "ERROR" => tracing::Level::ERROR,
        "WARN" | "WARNING" => tracing::Level::WARN,
        "INFO" => tracing::Level::INFO,
        "DEBUG" => tracing::Level::DEBUG,
        "TRACE" => tracing::Level::TRACE,
        _ => return None,
    };
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Some(RuntimeLog {
        level,
        target: text("target"),
        message: text("message"),
    })
}

/// Warnings the engine prints on every (re)load of a valid flow: canvas ids
/// that are not hex (hashed, as intended) and terminal nodes without output
/// wires. One line per node per deploy buried the real warnings.
fn is_benign(log: &RuntimeLog) -> bool {
    log.level == tracing::Level::WARN
        && (log.message.starts_with("Failed to parse ID ")
            || log.message.starts_with("No output wires in this node"))
}

/// Re-emits a runtime log line. The runtime already filters on
/// `PNEX_FLOW_LOG`, so info/debug/trace all surface as `info!` (the server
/// itself runs at info: a `debug!` here would be dropped silently); the
/// original level stays visible in the `level` field.
fn emit_runtime_log(log: &RuntimeLog) {
    if is_benign(log) {
        return;
    }
    let (level, target, message) = (log.level.as_str(), &log.target, &log.message);
    match log.level {
        tracing::Level::ERROR => {
            tracing::error!(runtime_level = level, runtime_target=%target, "flow runtime: {message}")
        }
        tracing::Level::WARN => {
            tracing::warn!(runtime_level = level, runtime_target=%target, "flow runtime: {message}")
        }
        _ => {
            tracing::info!(runtime_level = level, runtime_target=%target, "flow runtime: {message}")
        }
    }
}

#[cfg(test)]
mod runtime_log_tests {
    use super::*;

    #[test]
    fn json_log_line_keeps_its_level() {
        let line = r#"{"ts":1,"level":"INFO","target":"edgelink_core","message":"flow started"}"#;
        assert_eq!(
            parse_runtime_log(line),
            Some(RuntimeLog {
                level: tracing::Level::INFO,
                target: "edgelink_core".into(),
                message: "flow started".into(),
            })
        );
        for (raw, level) in [
            ("ERROR", tracing::Level::ERROR),
            ("WARN", tracing::Level::WARN),
            ("DEBUG", tracing::Level::DEBUG),
            ("TRACE", tracing::Level::TRACE),
        ] {
            let line = format!(r#"{{"ts":1,"level":"{raw}","target":"t","message":"m"}}"#);
            assert_eq!(parse_runtime_log(&line).map(|l| l.level), Some(level));
        }
    }

    #[test]
    fn load_noise_is_benign_real_warnings_are_not() {
        let warn = |message: &str| RuntimeLog {
            level: tracing::Level::WARN,
            target: "edgelink_core".into(),
            message: message.into(),
        };
        assert!(is_benign(&warn(
            "Failed to parse ID 'pnexflow1_n2' as hex, converting to hash"
        )));
        assert!(is_benign(&warn(
            "No output wires in this node: Node(id='42fd', name='')"
        )));
        assert!(!is_benign(&warn("pnex-device-write [] : write failed")));
    }

    #[test]
    fn non_json_or_unknown_level_falls_back() {
        assert_eq!(
            parse_runtime_log("thread 'main' panicked at src/main.rs"),
            None
        );
        assert_eq!(parse_runtime_log(r#"{"message":"no level"}"#), None);
        assert_eq!(parse_runtime_log(r#"{"level":"LOUD","message":"x"}"#), None);
    }
}

pub(super) fn signal_reload(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        // SIGUSR1: reserved for the application — our reload contract.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err("signal reload not supported on this platform".into())
    }
}

/// SIGKILL of a runtime child (fencing / halt): no grace period — a worker
/// that lost its placement must stop acting at once.
pub(super) fn kill_child(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Does the pid exist (signal 0)? Tolerant of platforms without support.
pub(super) fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // `kill(pid, 0)`: no signal sent, only the check.
        // -1/ESRCH = dead; -1/EPERM = alive (another user).
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// A program token containing a `/` is a path: resolved against the cwd
/// then the monorepo root (copy of the firmware-builder `resolve_program`
/// rule — Taskfile/.env paths stay valid whatever the worker's
/// cwd).
pub(crate) fn resolve_program(token: &str) -> String {
    use std::path::Path;
    const REPO_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    if !token.contains('/') {
        return token.to_string();
    }
    let try_anchor = |anchor: &Path| -> Option<String> {
        anchor
            .join(token)
            .canonicalize()
            .ok()
            .map(|p| p.display().to_string())
    };
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(abs) = try_anchor(&cwd) {
            return abs;
        }
    }
    if let Some(abs) = try_anchor(Path::new(REPO_ROOT)) {
        return abs;
    }
    token.to_string()
}

#[cfg(test)]
mod limit_tests {
    use super::parse_memory_limit_mb;

    #[test]
    fn memory_limit_is_opt_in() {
        assert_eq!(parse_memory_limit_mb(None), None);
        assert_eq!(parse_memory_limit_mb(Some("")), None);
        assert_eq!(parse_memory_limit_mb(Some("0")), None);
        assert_eq!(parse_memory_limit_mb(Some("abc")), None);
        assert_eq!(
            parse_memory_limit_mb(Some(" 512 ")),
            Some(512 * 1024 * 1024)
        );
    }
}
