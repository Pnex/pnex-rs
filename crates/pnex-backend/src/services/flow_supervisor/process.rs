//! Runtime child process management: spawn, output pumping, reload signal, liveness.

use super::*;

/// Env de l'enfant runtime (et du pré-flight) : allowlist + creds OpenObserve
/// injectées depuis le yaml — identique pour les deux, le check doit voir ce
/// que l'enfant verra.
pub(crate) fn apply_runtime_env(cmd: &mut tokio::process::Command, settings: &FlowSettings) {
    cmd.env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env(
            "PNEX_FLOW_LOG",
            std::env::var("PNEX_FLOW_LOG").unwrap_or_else(|_| "info".into()),
        );
    // Same egress policy in the runtime as in the server (R8, http-fetch).
    cmd.env("PNEX_EGRESS", pnex_core::egress::policy().as_str());
    if let Ok(hosts) = std::env::var("PNEX_EGRESS_ALLOW_HOSTS") {
        cmd.env("PNEX_EGRESS_ALLOW_HOSTS", hosts);
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

/// Spawn de l'enfant + pumps stdout/stderr → tracing. Retourne `None` si le
/// lancement échoue (déjà logué).
pub(super) fn spawn_child(
    settings: &FlowSettings,
    flows_path: &std::path::Path,
    pending: PendingAcks,
) -> Option<ChildProc> {
    let program = resolve_program(&settings.runtime_cmd);
    tracing::info!(cmd=%program, flows=%flows_path.display(), "démarrage du runtime de flow");
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
            tracing::error!(cmd=%program, error=%e, "lancement du runtime de flow impossible");
            return None;
        }
    };
    let pid = child.id().expect("pid au spawn");

    let stdout = child.stdout.take().expect("stdout pipé");
    let stderr = child.stderr.take().expect("stderr pipé");
    tokio::spawn(pump_lines(stdout, tracing::Level::INFO, Some(pending)));
    tokio::spawn(pump_lines(stderr, tracing::Level::WARN, None));

    // Watcher de fin de vie : `child.wait()` en tâche dédiée pour garder la
    // boucle de supervision libre (et récolter le process — pas de zombie).
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
        // Seul le stdout (INFO) est parsé : les événements consommés par le
        // feed/acks descendent en `debug` pour éviter le bruit INFO.
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

/// Re-emits a runtime log line. The runtime already filters on
/// `PNEX_FLOW_LOG`, so info/debug/trace all surface as `info!` (the server
/// itself runs at info: a `debug!` here would be dropped silently); the
/// original level stays visible in the `level` field.
fn emit_runtime_log(log: &RuntimeLog) {
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
        // SIGUSR1 : réservé à l'application — notre contrat de rechargement.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err("rechargement par signal non supporté sur cette plateforme".into())
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

/// Le pid existe-t-il (signal 0) ? Tolérant aux plateformes sans support.
pub(super) fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // `kill(pid, 0)` : aucun signal envoyé, seulement la vérification.
        // -1/ESRCH = mort ; -1/EPERM = vivant (autre utilisateur).
        let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Un token-programme contenant un `/` est un chemin : résolu contre le cwd
/// puis la racine du monorepo (copie de la règle `resolve_program` du
/// firmware-builder — les chemins Taskfile/.env restent valides quel que soit
/// le cwd du worker).
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
