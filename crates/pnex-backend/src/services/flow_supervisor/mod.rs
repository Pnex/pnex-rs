//! Supervisor of the ETL flow runtime (D18, mode B) — an isolated
//! `pnex-flow-runtime` child process. Since D106 a supervisor is an **instance**
//! owned by a flow cluster worker (`services::flow_cluster`): one worker =
//! one supervisor = one runtime child running the flows of the orgs placed
//! on that worker.
//!
//! Contrat avec l'enfant (cf. `crates/pnex-flow-runtime`) :
//! - artefact projeté : `<state_dir>/flows.json` (écriture atomique) ;
//! - rechargement à chaud : **SIGUSR1** (le runtime relit le fichier et
//!   redéploie via `Engine::redeploy_flows` — aucune surface HTTP) ;
//! - santé : `<state_dir>/runtime.json` écrit par l'enfant (pid, flow_rev,
//!   redeploys) ; stdout JSON-lines rejoué en `tracing`.
//!
//! Secrets : l'enfant ne reçoit que `PATH`, `HOME` + la allowlist
//! d'environnement (`env_allowlist`, e.g. `VALKEY_URL`) + les creds
//! OpenObserve **injectées depuis le yaml** (`settings.openobserve` — le
//! serveur ne les a pas en env process) — jamais de secret dans `flows.json`
//! (PRD §8).

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use loco_rs::prelude::*;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;

use crate::services::flow::FlowSettings;
use pnex_core::{FlowArtifactMeta, FlowRuntimeStatus};

/// Erreur de deploy structurée : `Check` = pré-flight rejeté par le runtime
/// (→ 400 avec les erreurs moteur réelles par flow), `Runtime` = panne
/// d'infra (503 `flow_runtime`).
#[derive(Debug)]
pub enum DeployError {
    /// Tabs rejetés par le pré-flight **et bloquants** (filtrés par
    /// `enforce_flow`) — (flow_id, erreur moteur réelle).
    Check(Vec<FlowCheckError>),
    Runtime(String),
}

/// Un tab rejeté par le pré-flight (`--check` du runtime).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowCheckError {
    pub flow_id: i64,
    pub message: String,
}

/// One deploy request handed to a supervisor.
pub struct DeployRequest {
    /// Complete artifact the runtime must run after this deploy.
    pub artifact: Value,
    /// Flow whose runtime event acknowledges the deploy (`flow_started` /
    /// `flow_error` / `flow_stopped`). `None` = fire-and-forget reconcile:
    /// the artifact is written and signalled, per-flow health still flows
    /// in through the stdout events.
    pub ack: Option<FlowArtifactMeta>,
    /// Flow whose pre-flight failure is **blocking** (deploy of one flow);
    /// `None` = removals/reprojections (failures of other tabs are logged).
    pub enforce_flow: Option<i64>,
    /// Entries to pre-flight instead of the whole artifact (the fragment of
    /// the org being changed: a worker must not re-check thousands of
    /// untouched tabs at every deploy). `None` = check the whole artifact.
    pub preflight_scope: Option<Value>,
}

/// Commandes adressées à la boucle de supervision.
enum SupervisorCmd {
    /// Déployer un artefact (pré-flight + écriture + signal + acquittement).
    Deploy {
        req: DeployRequest,
        reply: oneshot::Sender<Result<(), DeployError>>,
    },
    /// Kill the runtime child now and keep it down until the next deploy
    /// (self-fencing of a worker that lost its lease, drain end).
    Halt { reply: oneshot::Sender<()> },
}

/// Pending deploy acknowledgements of one supervisor: flow_id → sender of
/// the oneshot awaited by `handle_deploy`. Registered **before** spawn /
/// SIGUSR1 (signal/ack race), resolved by the stdout pump. Deploys are
/// serialized by the supervision loop → at most one pending per flow.
pub(crate) type PendingAcks =
    Arc<std::sync::Mutex<std::collections::HashMap<i64, oneshot::Sender<Result<(), String>>>>>;

/// Handle of one supervised runtime (cheap to clone).
#[derive(Clone)]
pub struct Supervisor {
    tx: mpsc::Sender<SupervisorCmd>,
    settings: Arc<FlowSettings>,
}

mod debug;
mod health;
mod process;

pub use debug::*;
pub use health::*;
// `process` has no `pub` items (crate-internal helpers only): a `pub` glob
// here would warn about a re-export nothing can actually reach.
pub(crate) use process::*;

impl Supervisor {
    /// Starts the supervision loop of one runtime. The child is spawned
    /// lazily by the first non-empty deploy: a stale `flows.json` left by a
    /// previous process is **never** replayed (the cluster worker projects
    /// its placements from the database at boot).
    ///
    /// The state dir is claimed exclusively (see [`claim_state_dir`]): two
    /// supervisors (two pods on a shared volume, or two workers of one
    /// process) never share a `flows.json` nor delete each other's files.
    pub fn spawn(settings: FlowSettings) -> Self {
        let (tx, rx) = mpsc::channel(8);
        let (settings, lock) = claim_state_dir(settings);
        let settings = Arc::new(settings);
        tokio::spawn(run_supervisor(settings.clone(), rx, lock));
        Self { tx, settings }
    }

    pub fn settings(&self) -> &FlowSettings {
        &self.settings
    }

    /// Deploy: pre-flight (`--check`), artifact write, reload, per-flow
    /// acknowledgement (see [`DeployRequest`]).
    pub async fn deploy(&self, req: DeployRequest) -> Result<(), DeployError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(SupervisorCmd::Deploy {
                req,
                reply: reply_tx,
            })
            .await
            .map_err(|_| DeployError::Runtime("flow runtime supervisor stopped".into()))?;
        reply_rx
            .await
            .map_err(|_| DeployError::Runtime("flow runtime supervisor unreachable".into()))?
    }

    /// Kills the runtime child (fencing): returns once it is down.
    pub async fn halt(&self) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .tx
            .send(SupervisorCmd::Halt { reply: reply_tx })
            .await
            .is_ok()
        {
            let _ = reply_rx.await;
        }
    }

    /// Runtime state of **one flow** on this supervisor.
    pub fn runtime_status(&self, flow_id: i64) -> FlowRuntimeStatus {
        runtime_status(&self.settings, flow_id)
    }
}

/// État du runtime pour **un flow** : santé process (`runtime.json` : pid,
/// vivant, rechargements) croisée avec la santé **par flow** (événements
/// `flow_*` du stdout). Best-effort : état absent → `running=false`.
pub fn runtime_status(settings: &FlowSettings, flow_id: i64) -> FlowRuntimeStatus {
    let path = state_dir_of(settings).join("runtime.json");
    let mut status = FlowRuntimeStatus {
        running: false,
        ..Default::default()
    };
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            status.pid = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
            status.restarts = v.get("redeploys").and_then(|r| r.as_u64()).unwrap_or(0);
            // Santé process uniquement — PAS de version/flow ici : l'artefact
            // est multi-flows et la version déployée d'un flow vit en DB
            // (`flows.deployed_version_id`, remplie au deploy acquitté). Le
            // handler `GET /flows/{id}/runtime` croise santé moteur × DB.
            let alive = status.pid.is_some_and(pid_alive);
            status.running = alive && v.get("running").and_then(|r| r.as_bool()).unwrap_or(false);
        }
    }
    // Santé de l'engine de CE flow (None = inconnue — backend redémarré,
    // pas encore d'annonce du runtime).
    if let Some((engine_status, last_error)) = flow_engine_health(flow_id) {
        status.engine_status = Some(engine_status.as_str().to_string());
        status.last_error = last_error;
    }
    status
}

/// Boot advice (from `App::after_routes`): in a multi-pod deployment, a
/// pod that both serves the API and runs a flow runtime child shares one
/// cgroup between the two — a runaway flow can OOM-kill the API pod.
pub fn warn_colocated_runtime(config: &loco_rs::config::Config) {
    let flow = FlowSettings::from_config(config);
    if !flow.enabled || !crate::services::media::multi_pod_configured(config) {
        return;
    }
    let cluster = crate::services::flow_cluster::ClusterSettings::from_config(config);
    if !cluster.worker {
        return;
    }
    tracing::warn!(
        worker = %cluster.worker_id,
        memory_limit_mb = ?process::runtime_memory_limit_bytes().map(|b| b / (1024 * 1024)),
        "this pod serves the API AND runs a flow runtime (PNEX_FLOW_RUN_WORKER=true) in the same cgroup — in production run dedicated worker pods and set PNEX_FLOW_RUN_WORKER=false on API pods; PNEX_FLOW_RUNTIME_MAX_MEMORY_MB caps the runtime child"
    );
}

/// Name of the exclusive lock file of a state dir.
const STATE_DIR_LOCK: &str = ".supervisor.lock";

/// Takes a non-blocking exclusive `flock` on `<dir>/.supervisor.lock`.
/// `None` = held by another supervisor (or the dir is unusable).
fn try_lock_dir(dir: &std::path::Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(STATE_DIR_LOCK))
        .ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: plain flock(2) on a descriptor we own.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return None;
        }
    }
    Some(file)
}

/// Stable-ish instance tag of this process: worker id env, else host name
/// (the pod name under Kubernetes), plus the pid.
fn instance_tag() -> String {
    let host = std::env::var("PNEX_FLOW_WORKER_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "local".into());
    let safe: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}-{}", std::process::id())
}

/// Claims the configured state dir for this supervisor. The common case
/// (one supervisor per dir) keeps the configured path unchanged. When the
/// dir is already held — several pods mounting one shared volume, or two
/// workers of one process configured alike — this supervisor moves to a
/// private `<state_dir>/<host>-<pid>[-n]` sub-directory instead of
/// clobbering the other one's `flows.json` / `runtime.json`.
///
/// The lock is an advisory `flock`: it holds across pods on volumes that
/// implement it (NFSv4, CephFS, local disks); prefer one volume per pod.
fn claim_state_dir(mut settings: FlowSettings) -> (FlowSettings, Option<std::fs::File>) {
    let base = state_dir_of(&settings);
    if let Some(lock) = try_lock_dir(&base) {
        return (settings, Some(lock));
    }
    let tag = instance_tag();
    for n in 0..16u32 {
        let name = if n == 0 {
            tag.clone()
        } else {
            format!("{tag}-{n}")
        };
        let dir = base.join(name);
        if let Some(lock) = try_lock_dir(&dir) {
            tracing::warn!(
                configured = %base.display(),
                effective = %dir.display(),
                "flow state dir already used by another supervisor (shared volume?) — using a private sub-directory"
            );
            settings.state_dir = dir.to_string_lossy().into_owned();
            return (settings, Some(lock));
        }
    }
    tracing::error!(
        dir = %base.display(),
        "flow state dir could not be claimed exclusively — continuing unlocked"
    );
    (settings, None)
}

fn state_dir_of(settings: &FlowSettings) -> PathBuf {
    // Chemin relatif résolu contre le cwd (cohérence dev/worker).
    PathBuf::from(&settings.state_dir)
}

/// Flow ids of the tabs of an artifact (`pnex_flow_id` of `tab` entries).
fn artifact_flow_ids(artifact: &Value) -> std::collections::HashSet<i64> {
    artifact
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n.get("type").and_then(|t| t.as_str()) == Some("tab"))
        .filter_map(|n| n.get("pnex_flow_id").and_then(|v| v.as_i64()))
        .collect()
}

fn artifact_is_empty(artifact: &Value) -> bool {
    artifact.as_array().is_none_or(|a| a.is_empty())
}

// ───────────────────────────── Boucle interne ─────────────────────────────

struct ChildProc {
    pid: u32,
    /// Récepteur de fin de vie (description de l'exit — le watcher récolte
    /// le process dans sa propre tâche).
    exit_rx: std_mpsc::Receiver<String>,
}

async fn run_supervisor(
    settings: Arc<FlowSettings>,
    mut rx: mpsc::Receiver<SupervisorCmd>,
    // Held for the whole loop: released when the supervisor stops.
    _state_lock: Option<std::fs::File>,
) {
    let dir = state_dir_of(&settings);
    let flows_path = dir.join("flows.json");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!(dir=%dir.display(), error=%e, "répertoire d'état des flows inaccessible");
        return;
    }
    // A previous process may have left an artifact behind: it is stale by
    // definition (the placements may have moved meanwhile) — never replayed.
    let _ = std::fs::remove_file(&flows_path);
    let _ = std::fs::remove_file(dir.join("runtime.json"));

    let pending: PendingAcks = Arc::default();
    let mut child: Option<ChildProc> = None;
    let mut runtime_wanted = false;
    let mut started_at: Option<tokio::time::Instant> = None;
    let mut backoff = settings.restart_backoff_secs;
    let mut respawn_at = tokio::time::Instant::now();
    loop {
        // 1) Did a child die? → restart with a bounded exponential backoff
        // (reset when the child lived long enough to be deemed stable).
        if let Some(c) = &child {
            if let Ok(status) = c.exit_rx.try_recv() {
                tracing::warn!(pid=c.pid, status=%status, "runtime de flow arrêté");
                let uptime = started_at.map(|t| t.elapsed()).unwrap_or_default();
                if uptime >= Duration::from_secs(settings.restart_backoff_secs * 2) {
                    backoff = settings.restart_backoff_secs;
                }
                respawn_at = tokio::time::Instant::now() + Duration::from_secs(backoff);
                backoff = (backoff * 2).min(settings.restart_backoff_max_secs);
                child = None;
                started_at = None;
            }
        }

        // 2) Relance planifiée (crash, ou premier spawn d'un deploy).
        if runtime_wanted && child.is_none() && tokio::time::Instant::now() >= respawn_at {
            match spawn_child(&settings, &flows_path, pending.clone()) {
                Some(c) => {
                    started_at = Some(tokio::time::Instant::now());
                    backoff = settings.restart_backoff_secs;
                    child = Some(c);
                }
                None => {
                    respawn_at = tokio::time::Instant::now() + Duration::from_secs(backoff);
                    backoff = (backoff * 2).min(settings.restart_backoff_max_secs);
                }
            }
        }

        // 3) Commandes (non bloquant : on boucle à intervalle court).
        match rx.try_recv() {
            Ok(SupervisorCmd::Deploy { req, reply }) => {
                let empty = artifact_is_empty(&req.artifact);
                let outcome =
                    handle_deploy(&settings, &flows_path, &mut child, &pending, req).await;
                if outcome.is_ok() {
                    // An empty artifact keeps no child alive (a worker
                    // without placement costs no process).
                    runtime_wanted = !empty;
                    started_at = Some(tokio::time::Instant::now());
                    backoff = settings.restart_backoff_secs;
                }
                let _ = reply.send(outcome.map(|_| ()));
            }
            Ok(SupervisorCmd::Halt { reply }) => {
                runtime_wanted = false;
                if let Some(c) = child.take() {
                    kill_child(c.pid);
                    // Reap within the terminate budget (the watcher task
                    // reports the exit).
                    let deadline = Duration::from_secs(settings.terminate_secs.max(1));
                    let _ =
                        tokio::task::spawn_blocking(move || c.exit_rx.recv_timeout(deadline)).await;
                }
                let _ = std::fs::remove_file(&flows_path);
                let _ = reply.send(());
            }
            Err(TryRecvError::Disconnected) => {
                tracing::info!("superviseur de flows : canal fermé, arrêt");
                if let Some(c) = child.take() {
                    kill_child(c.pid);
                }
                return;
            }
            Err(TryRecvError::Empty) => {}
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Atomic write of `value` at `path` (tmp + rename).
fn write_atomic(path: &std::path::Path, value: &Value) -> Result<(), DeployError> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_string_pretty(value).map_err(|e| DeployError::Runtime(e.to_string()))?,
    )
    .map_err(|e| DeployError::Runtime(format!("cannot write {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| DeployError::Runtime(format!("cannot rename {}: {e}", path.display())))
}

/// Deploys an artifact: **pre-flight first** (the candidate — or the
/// requested scope — is loaded by the runtime without ever touching
/// `flows.json`), then write, reload and per-flow acknowledgement.
/// Returns `Ok(true)` when this deploy (re)started a child.
async fn handle_deploy(
    settings: &FlowSettings,
    flows_path: &std::path::Path,
    child: &mut Option<ChildProc>,
    pending: &PendingAcks,
    req: DeployRequest,
) -> Result<bool, DeployError> {
    let DeployRequest {
        artifact,
        ack,
        enforce_flow,
        preflight_scope,
    } = req;

    // 1) Pre-flight on the scope (default: the whole candidate), written
    //    next to the live artifact — never over it.
    let scope = preflight_scope.as_ref().unwrap_or(&artifact);
    if !artifact_is_empty(scope) {
        let candidate = flows_path.with_extension("candidate.json");
        write_atomic(&candidate, scope)?;
        let violations = run_preflight(settings, &candidate).await;
        let _ = std::fs::remove_file(&candidate);
        let violations = violations.map_err(DeployError::Runtime)?;
        if !violations.is_empty() {
            // Only the deployed flow is blocking; other invalid tabs are
            // tolerated by the farm (isolated flow_error) — warn only.
            let (blocking, others): (Vec<_>, Vec<_>) = match enforce_flow {
                Some(fid) => violations.into_iter().partition(|v| v.flow_id == fid),
                None => (Vec::new(), violations),
            };
            for v in &others {
                tracing::warn!(
                    flow_id = v.flow_id,
                    error = %v.message,
                    "pre-flight: another flow is invalid (not blocking)"
                );
            }
            if !blocking.is_empty() {
                return Err(DeployError::Check(blocking));
            }
        }
    }

    let alive = child.as_ref().is_some_and(|c| pid_alive(c.pid));
    let empty = artifact_is_empty(&artifact);
    // An acked flow absent from the artifact cannot run on a runtime that
    // is down or about to boot on it: the removal is applied already.
    let ack = ack.filter(|m| alive || artifact_flow_ids(&artifact).contains(&m.flow_id));

    // 2) Acknowledgement registered **before** anything that can produce
    //    events (signal/ack race).
    let ack_rx = ack.as_ref().map(|m| {
        let (ack_tx, ack_rx) = oneshot::channel();
        pending
            .lock()
            .expect("lock pending deploy")
            .insert(m.flow_id, ack_tx);
        (m.clone(), ack_rx)
    });
    let drop_pending = |m: &Option<(FlowArtifactMeta, oneshot::Receiver<Result<(), String>>)>| {
        if let Some((m, _)) = m {
            pending
                .lock()
                .expect("lock pending deploy")
                .remove(&m.flow_id);
        }
    };

    // 3) Atomic write of the validated artifact.
    if let Err(e) = write_atomic(flows_path, &artifact) {
        drop_pending(&ack_rx);
        return Err(e);
    }

    // Debug feed purged BEFORE the signal: the entries of the previous
    // version die with the artifact they reflect.
    if let Some((m, _)) = &ack_rx {
        clear_debug_feed(m.flow_id);
    }

    // 4) Child alive? Otherwise spawn (first deploy or restart after a
    //    crash) — except for an empty artifact: nothing to run.
    if !alive {
        if let Some(c) = child.take() {
            kill_child(c.pid);
        }
        if empty {
            drop_pending(&ack_rx);
            return Ok(false);
        }
        match spawn_child(settings, flows_path, pending.clone()) {
            Some(c) => *child = Some(c),
            None => {
                drop_pending(&ack_rx);
                return Err(DeployError::Runtime("cannot start the flow runtime".into()));
            }
        }
    }
    let pid = child.as_ref().expect("enfant vivant").pid;

    // 5) SIGUSR1 — except on a fresh spawn, which already reads the new
    //    file and announces all its tabs at boot (flow_started per flow).
    if alive {
        if let Err(e) = signal_reload(pid) {
            drop_pending(&ack_rx);
            return Err(DeployError::Runtime(format!("SIGUSR1 to pid {pid}: {e}")));
        }
    }

    // 6) Wait for the **per-flow** acknowledgement (when requested).
    let Some((meta, ack_rx)) = ack_rx else {
        return Ok(!alive);
    };
    let ack_secs = settings.reload_ack_secs;
    match tokio::time::timeout(Duration::from_secs(ack_secs), ack_rx).await {
        Ok(Ok(Ok(()))) => {
            tracing::info!(
                flow_id = meta.flow_id,
                version = meta.version_number,
                pid,
                "flow déployé"
            );
            Ok(!alive)
        }
        Ok(Ok(Err(e))) => Err(DeployError::Runtime(format!(
            "flow rejected by the runtime: {e}"
        ))),
        Ok(Err(_)) => Err(DeployError::Runtime(
            "flow runtime supervisor unreachable".into(),
        )),
        Err(_) => {
            pending
                .lock()
                .expect("lock pending deploy")
                .remove(&meta.flow_id);
            Err(DeployError::Runtime(format!(
                "reload acknowledgement missing after {ack_secs} s (pid {pid})"
            )))
        }
    }
}

/// Pré-flight : `--check` du runtime sur le candidat (env identique à
/// l'enfant — le check doit voir les mêmes nœuds/secrets). Retourne les
/// tabs rejetés (vide = vert) ; `Err` = le pré-flight lui-même est en panne
/// (runtime absent, timeout, pas de `check_done`).
async fn run_preflight(
    settings: &FlowSettings,
    candidate: &std::path::Path,
) -> Result<Vec<FlowCheckError>, String> {
    // Shares the per-pod runtime-check cap with function test/validate so
    // a burst of deploys cannot fork unbounded `--check` children.
    let _permit = crate::services::compute_limits::runtime_check()
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let program = resolve_program(&settings.runtime_cmd);
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg("--check")
        .arg(candidate)
        .env_clear()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    apply_runtime_env(&mut cmd, settings);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("lancement du pré-flight : {e}"))?;
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().expect("stdout pipé"));
    let wait = async move {
        let mut violations = Vec::new();
        let mut lines = String::new();
        use tokio::io::AsyncBufReadExt;
        loop {
            lines.clear();
            match stdout.read_line(&mut lines).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let Ok(v) = serde_json::from_str::<Value>(lines.trim()) else {
                        continue;
                    };
                    if v.get("event").and_then(|e| e.as_str()) == Some("check_flow")
                        && v.get("ok").and_then(|o| o.as_bool()) == Some(false)
                    {
                        violations.push(FlowCheckError {
                            flow_id: v.get("flow").and_then(|f| f.as_i64()).unwrap_or_default(),
                            message: v
                                .get("error")
                                .and_then(|e| e.as_str())
                                .unwrap_or("tab rejeté par le runtime")
                                .to_string(),
                        });
                    }
                }
            }
        }
        let status = child.wait().await;
        violations.shrink_to_fit();
        (status, violations)
    };

    let secs = settings.check_timeout_secs;
    match tokio::time::timeout(Duration::from_secs(secs), wait).await {
        Ok((Ok(_), violations)) => Ok(violations),
        Ok((Err(e), _)) => Err(format!("pré-flight : attente du runtime : {e}")),
        Err(_) => Err(format!("pré-flight sans acquittement après {secs} s")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_program_chemins() {
        // Token sans `/` : recherche PATH, laissé tel quel.
        assert_eq!(resolve_program("pnex-flow-runtime"), "pnex-flow-runtime");
        // Chemin inexistant : tel quel (erreur claire au spawn).
        assert_eq!(resolve_program("./nulle-part/xyz"), "./nulle-part/xyz");
        // Chemin du repo : résolu contre la racine monorepo.
        let abs = resolve_program("crates/pnex-core/Cargo.toml");
        assert!(abs.contains("Cargo.toml"), "{abs}");
        assert!(std::path::Path::new(&abs).exists(), "{abs}");
    }

    #[test]
    fn second_supervisor_on_a_shared_state_dir_gets_a_private_subdir() {
        let dir = std::env::temp_dir().join(format!("pnex-flow-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = FlowSettings {
            state_dir: dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let (first, lock_a) = claim_state_dir(base.clone());
        assert!(lock_a.is_some());
        assert_eq!(
            first.state_dir, base.state_dir,
            "first claim keeps the configured dir"
        );
        let (second, lock_b) = claim_state_dir(base.clone());
        assert!(lock_b.is_some());
        assert_ne!(second.state_dir, base.state_dir);
        assert!(second.state_dir.starts_with(&base.state_dir));
        // Releasing the first lock frees the configured dir again.
        drop(lock_a);
        let (third, lock_c) = claim_state_dir(base.clone());
        assert!(lock_c.is_some());
        assert_eq!(third.state_dir, base.state_dir);
        drop((lock_b, lock_c));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_status_sans_etat() {
        let s = FlowSettings {
            state_dir: "/tmp/pnex-flow-status-absente".into(),
            ..Default::default()
        };
        let st = runtime_status(&s, 999_800);
        assert!(!st.running);
        assert_eq!(st.pid, None);
        assert_eq!(st.engine_status, None, "santé flow inconnue sans événement");
    }

    #[test]
    fn runtime_status_ne_porte_pas_de_version() {
        // Le runtime.json ne porte AUCUNE version/flow (artefact multi-flows
        // — la version déployée d'un flow vit en DB). Un ancien fichier avec
        // ces champs est lu sans erreur mais ignoré.
        let dir = std::env::temp_dir().join(format!("pnex-flow-legacy-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("runtime.json"),
            format!(
                r#"{{"pid":{},"running":true,"started_at":1,"redeploys":2,"flow_id":4,"version_number":26}}"#,
                std::process::id() // vivant : le test de vie du pid passe
            ),
        )
        .unwrap();
        let s = FlowSettings {
            state_dir: dir.to_string_lossy().to_string(),
            ..Default::default()
        };
        let st = runtime_status(&s, 999_810);
        assert!(st.running, "pid vivant → moteur considéré démarré");
        assert_eq!(st.restarts, 2);
        assert_eq!(
            st.deployed_flow_id, None,
            "version jamais lue du runtime.json"
        );
        assert_eq!(st.deployed_version_number, None);
        assert_eq!(st.engine_status, None, "santé flow = événements uniquement");
        assert_eq!(st.last_error, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sante_par_flow_et_acquittement_deploy() {
        // Sanité : la carte de santé est un global de test — ids dédiés.
        reset_flow_health_for_tests();
        let fid = 999_820;
        let pending: PendingAcks = Arc::default();

        // flow_started : santé Running + acquitte un deploy en attente (Ok).
        let (tx, mut rx) = oneshot::channel();
        pending.lock().expect("lock").insert(fid, tx);
        handle_runtime_line(
            &format!(
                r#"{{"event":"flow_started","flow":{fid},"tab":"pnexflow{fid}","rev":"abc"}}"#
            ),
            &pending,
        );
        assert_eq!(
            rx.try_recv().expect("ack résolu"),
            Ok(()),
            "flow_started doit acquitter le deploy"
        );
        let (status, last_error) = flow_engine_health(fid).expect("entrée de santé");
        assert_eq!(status, FlowEngineHealth::Running);
        assert_eq!(last_error, None);

        // flow_error : santé Error + last_error, acquitte en Err (erreur
        // moteur réelle remontée au contrôleur).
        let (tx, mut rx) = oneshot::channel();
        pending.lock().expect("lock").insert(fid, tx);
        handle_runtime_line(
            &format!(
                r#"{{"event":"flow_error","flow":{fid},"tab":"pnexflow{fid}","error":"wire fantôme"}}"#
            ),
            &pending,
        );
        assert_eq!(
            rx.try_recv().expect("ack résolu"),
            Err("wire fantôme".to_string()),
            "flow_error doit rejeter le deploy avec l'erreur réelle"
        );
        let (status, last_error) = flow_engine_health(fid).expect("entrée de santé");
        assert_eq!(status, FlowEngineHealth::Error);
        assert_eq!(last_error.as_deref(), Some("wire fantôme"));

        // flow_stopped (retrait du tab — dé-déploiement) : ack Ok (le
        // changement a été appliqué) + santé Stopped.
        let (tx, mut rx) = oneshot::channel();
        pending.lock().expect("lock").insert(fid, tx);
        handle_runtime_line(
            &format!(r#"{{"event":"flow_stopped","flow":{fid},"tab":"pnexflow{fid}"}}"#),
            &pending,
        );
        assert_eq!(
            rx.try_recv().expect("ack résolu"),
            Ok(()),
            "retrait = acquittement"
        );
        let (status, _) = flow_engine_health(fid).expect("entrée de santé");
        assert_eq!(status, FlowEngineHealth::Stopped);
    }

    fn debug_line(flow: i64, msg: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "debug", "node": "deadbeef", "node_red": "n2",
            "flow": flow, "name": "n2", "msg": msg, "msgid": "m1"
        })
    }

    #[test]
    fn node_status_keeps_the_latest_per_node_and_clears_on_deploy() {
        let status = |node: &str, code: &str| {
            serde_json::json!({
                "event": "debug", "flow": 999_950, "node": "h", "node_red": node,
                "source": "pnex-status", "msg": {"level": "ok", "code": code, "stats": {}},
            })
        };
        push_debug(&status("n3", "vision-model-loading"));
        push_debug(&status("n3", "vision-running"));
        push_debug(&status("n2", "camera-streaming"));
        let got = node_statuses(999_950);
        assert_eq!(got.len(), 2, "one status per node");
        assert_eq!(got[1].node_id, "n3");
        assert_eq!(got[1].msg["code"], "vision-running", "latest wins");
        clear_debug_feed(999_950);
        assert!(node_statuses(999_950).is_empty());
    }

    #[test]
    fn node_last_value_per_debug_node_capped_and_cleared_on_deploy() {
        push_debug(&debug_line(999_951, "first"));
        push_debug(&debug_line(999_951, "second"));
        let big = "x".repeat(20_000);
        push_debug(&serde_json::json!({
            "event": "debug", "node": "h", "node_red": "n9", "flow": 999_951, "msg": big
        }));
        // Statuses never land in the last values.
        push_debug(&serde_json::json!({
            "event": "debug", "flow": 999_951, "node": "h", "node_red": "n3",
            "source": "pnex-status", "msg": {"level": "ok", "code": "vision-running", "stats": {}},
        }));
        let got = node_last_values(999_951);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].node_id, "n2");
        assert_eq!(got[0].msg, "second", "latest wins");
        assert_eq!(got[1].msg["truncated"], true);
        assert!(got[1].msg["preview"].as_str().unwrap().len() <= 8 * 1024);
        clear_debug_feed(999_951);
        assert!(node_last_values(999_951).is_empty());
    }

    #[test]
    fn node_last_value_downsamples_long_arrays_before_truncating() {
        // A forecast at horizon 1000: one point per step, far above 8 KB.
        let points: Vec<_> = (0..1000)
            .map(|i| serde_json::json!({"ts": 1_759_000_000_000_i64 + i * 1000, "mean": 63.123456 + i as f64, "lower": 60.5, "upper": 66.5}))
            .collect();
        let capped = debug::cap_message(serde_json::json!({
            "payload": {"value": 63.0, "points": points},
            "topic": "bearing",
        }));
        assert!(capped.get("preview").is_none(), "{capped}");
        assert_eq!(capped["downsampled"], true);
        assert_eq!(capped["topic"], "bearing");
        let got = capped["payload"]["points"].as_array().unwrap();
        assert!(got.len() <= 100 && got.len() >= 10, "{}", got.len());
        assert_eq!(got[0]["ts"], 1_759_000_000_000_i64);
        assert_eq!(
            got.last().unwrap()["ts"],
            1_759_000_000_000_i64 + 999 * 1000
        );
        assert!(capped.to_string().len() <= 8 * 1024);
    }

    #[test]
    fn feed_cap_ttls_et_sans_attribution() {
        // Sans attribution flow : l'entrée est JETÉE (multi-org — pas de
        // bucket « inconnu »).
        push_debug(&serde_json::json!({
            "event": "debug", "node": "x", "node_red": "n1", "msg": "orpheline"
        }));
        assert!(debug_entries(999_901, 100).is_empty());

        // Cap par flow : au-delà de DEBUG_CAP_PER_FLOW, les plus anciennes
        // sortent.
        for i in 0..(DEBUG_CAP_PER_FLOW + 10) {
            push_debug(&debug_line(999_902, &format!("m{i}")));
        }
        let entries = debug_entries(999_902, 100);
        assert_eq!(entries.len(), 100);
        assert_eq!(entries.last().unwrap().msg, serde_json::json!("m209"));
        assert!(entries.len() < DEBUG_CAP_PER_FLOW);
        // Le feed complet garde le cap exact.
        let all = debug_entries(999_902, DEBUG_CAP_PER_FLOW + 50);
        assert_eq!(all.len(), DEBUG_CAP_PER_FLOW);

        // Sources étanches : `pnex-display` passe tel quel, topic/msgid lus.
        push_debug(&serde_json::json!({
            "event": "debug", "node": "abab", "node_red": "n7", "flow": 999_903,
            "name": "sonde", "msg": {"k": 1}, "source": "pnex-display",
            "topic": "t", "msgid": "m9"
        }));
        let e = &debug_entries(999_903, 10)[0];
        assert_eq!(e.source, "pnex-display");
        assert_eq!(e.msg, serde_json::json!({"k": 1}));
        assert_eq!(e.node_id, "n7");
        assert_eq!(e.topic.as_deref(), Some("t"));
        assert_eq!(e.msgid.as_deref(), Some("m9"));
        // ts RFC 3339 bien formé.
        assert!(e.ts.len() == 20 && e.ts.ends_with('Z'), "{}", e.ts);
    }

    #[test]
    fn rfc3339_forme() {
        let ts = rfc3339_now();
        assert_eq!(ts.len(), 20, "{ts}");
        assert!(ts.ends_with('Z'), "{ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }

    #[test]
    fn settings_debug_tools_defaut_faux() {
        let s = FlowSettings::default();
        assert!(!s.debug_tools, "mode run par défaut");
    }
}
