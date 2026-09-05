//! Superviseur du runtime de flows ETL (D18, mode B) — process enfant
//! `pnex-flow-runtime` isolé, lancé depuis `App::after_routes` (comme
//! `spawn_batcher` / `spawn_reaper` : doit vivre aussi en ServerOnly).
//!
//! Contrat avec l'enfant (cf. `crates/pnex-flow-runtime`) :
//! - artefact projeté : `<state_dir>/flows.json` (écriture atomique) ;
//! - rechargement à chaud : **SIGUSR1** (le runtime relit le fichier et
//!   redéploie via `Engine::redeploy_flows` — aucune surface HTTP) ;
//! - santé : `<state_dir>/runtime.json` écrit par l'enfant (pid, flow_rev,
//!   redeploys) ; stdout JSON-lines rejoué en `tracing`.
//!
//! Secrets : l'enfant ne reçoit que `PATH`, `HOME` + la allowlist
//! d'environnement (`env_allowlist`, ex. `DATABASE_URL`) + les creds
//! OpenObserve **injectées depuis le yaml** (`settings.openobserve` — le
//! serveur ne les a pas en env process) — jamais de secret dans `flows.json`
//! (PRD §8).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, OnceLock};
use std::time::Duration;

use loco_rs::prelude::*;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::oneshot;

use crate::services::flow::FlowSettings;
use pnex_core::{FlowArtifactMeta, FlowRuntimeStatus};

/// Commandes adressées à la boucle de supervision.
enum SupervisorCmd {
    /// Déployer un artefact (écriture + signal + acquittement).
    Deploy {
        artifact: Value,
        meta: FlowArtifactMeta,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Exécuter une fois le flow déployé (cmd.json + SIGUSR2 + acquittement
    /// stdout corrélé par seq).
    RunOnce {
        flow_id: i64,
        seq: u64,
        reply: oneshot::Sender<Result<pnex_core::RunOnceResult, String>>,
    },
}

/// État partagé : l'émetteur vers la boucle (présent seulement si lancé).
static SUPERVISOR_TX: OnceLock<mpsc::Sender<SupervisorCmd>> = OnceLock::new();
/// Garde anti double-spawn : `after_routes` tourne à chaque boot d'app, y
/// compris dans les tests d'intégration (une app = au plus un superviseur).
static SPAWNED: AtomicBool = AtomicBool::new(false);

// ───────────────────────── Feed debug (panneau) ─────────────────────────

/// Anneau du panneau de debug : dernières sorties `debug` du runtime,
/// attribuées par flow (le runtime émet `flow`/`node_red` — une entrée sans
/// attribution est **jetée** : les orgs partagent un seul `flows.json`,
/// jamais de bucket « inconnu »).
static DEBUG_FEED: OnceLock<std::sync::Mutex<DebugFeed>> = OnceLock::new();

/// Caps bornés : un flow bavard ne peut ni exploser la mémoire ni évincer
/// indéfiniment les autres flows.
const DEBUG_CAP_PER_FLOW: usize = 200;
const DEBUG_MAX_FLOWS: usize = 64;
const DEBUG_TTL_SECS: i64 = 300;

#[derive(Default)]
struct DebugFeed {
    per_flow: std::collections::HashMap<i64, std::collections::VecDeque<(std::time::Instant, pnex_core::FlowDebugEntry)>>,
    next_seq: u64,
    /// Dernier accès par flow (évcition LRU au-delà de `DEBUG_MAX_FLOWS`).
    last_touch: std::collections::HashMap<i64, std::time::Instant>,
}

/// `DebugFeed` n'est lu que sous son mutex — horloge mono-thread efficace.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Timestamp RFC 3339 sans dépendance chrono (le formatter de `time` n'est
/// pas une dep du backend) : secondes depuis l'epoch → ISO-8601 UTC.
fn rfc3339_now() -> String {
    let secs = now_secs();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Algorithme de conversion date civile (Howard Hinnant) — époque 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (rem / 3600, rem % 3600 / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn feed() -> &'static std::sync::Mutex<DebugFeed> {
    DEBUG_FEED.get_or_init(std::sync::Mutex::default)
}

/// Ingère une ligne stdout `{"event":"debug",...}` du runtime : exige
/// l'attribution `flow` (sinon jetée) et estampille à la réception.
pub fn push_debug(raw: &serde_json::Value) {
    if raw.get("event").and_then(|e| e.as_str()) != Some("debug") {
        return;
    }
    let Some(flow_id) = raw.get("flow").and_then(|f| f.as_i64()) else {
        return;
    };
    let entry = pnex_core::FlowDebugEntry {
        seq: 0, // assigné sous lock
        ts: rfc3339_now(),
        flow_id,
        node_id: raw.get("node_red").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        name: raw.get("name").and_then(|v| v.as_str()).map(str::to_string),
        msg: raw.get("msg").cloned().unwrap_or(serde_json::Value::Null),
        source: raw.get("source").and_then(|v| v.as_str()).unwrap_or("debug").to_string(),
        topic: raw.get("topic").and_then(|v| v.as_str()).map(str::to_string),
        msgid: raw.get("msgid").and_then(|v| v.as_str()).map(str::to_string),
    };
    let mut feed = feed().lock().expect("lock debug feed");
    let now = std::time::Instant::now();
    feed.next_seq += 1;
    let mut entry = entry;
    entry.seq = feed.next_seq;
    let q = feed.per_flow.entry(flow_id).or_default();
    q.push_back((now, entry));
    while q.len() > DEBUG_CAP_PER_FLOW {
        q.pop_front();
    }
    feed.last_touch.insert(flow_id, now);
    // Éviction LRU des flows (au-delà du cap, le flow le plus ancien sort).
    while feed.per_flow.len() > DEBUG_MAX_FLOWS {
        let Some(victim) =
            feed.last_touch.iter().min_by_key(|(_, t)| **t).map(|(fid, _)| *fid)
        else {
            break;
        };
        feed.per_flow.remove(&victim);
        feed.last_touch.remove(&victim);
    }
}

/// Snapshot du feed d'un flow (les plus anciennes d'abord), entrées TTL
/// dépassées purgées.
pub fn debug_entries(flow_id: i64, limit: usize) -> Vec<pnex_core::FlowDebugEntry> {
    let mut feed = feed().lock().expect("lock debug feed");
    let now = std::time::Instant::now();
    if let Some(q) = feed.per_flow.get_mut(&flow_id) {
        q.retain(|(t, _)| now.duration_since(*t).as_secs() as i64 <= DEBUG_TTL_SECS);
        q.iter().map(|(_, e)| e.clone()).skip(q.len().saturating_sub(limit)).collect()
    } else {
        Vec::new()
    }
}

/// Purge du feed d'un flow (au succès d'un deploy — les entrées d'une
/// version antérieure ne doivent pas survivre à l'artefact frais).
pub fn clear_debug_feed(flow_id: i64) {
    let mut feed = feed().lock().expect("lock feed");
    feed.per_flow.remove(&flow_id);
}

/// Parsing d'une ligne stdout JSON-lines du runtime. Retourne true si la
/// ligne a été consommée par le feed/acks (reste rejouée en tracing).
fn handle_runtime_line(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    match v.get("event").and_then(|e| e.as_str()) {
        Some("debug") => {
            push_debug(&v);
            true
        }
        Some("run_once_done" | "run_once_failed") => {
            resolve_run_once(&v);
            true
        }
        _ => false,
    }
}

/// Lance le superviseur si `settings.flow.enabled` (sinon no-op — tests,
/// déploiements sans moteur de flow). Idempotent par process.
pub fn spawn_supervisor(ctx: &AppContext) {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.enabled {
        tracing::info!("moteur de flow désactivé (settings.flow.enabled=false)");
        return;
    }
    spawn_supervisor_with(settings);
}

/// Variante testable : réglages explicites.
pub fn spawn_supervisor_with(settings: FlowSettings) {
    if SPAWNED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        tracing::warn!("superviseur de flows déjà lancé dans ce process");
        return;
    }
    let (tx, rx) = mpsc::channel(8);
    if SUPERVISOR_TX.set(tx).is_err() {
        return;
    }
    tokio::spawn(run_supervisor(settings, rx));
}

/// Déploiement : écrit l'artefact puis recharge le runtime. Erreur si le
/// superviseur ne tourne pas (`enabled=false`) ou si l'enfant n'acquitte pas.
pub async fn deploy(artifact: Value, meta: FlowArtifactMeta) -> Result<(), String> {
    let tx = SUPERVISOR_TX
        .get()
        .ok_or_else(|| "runtime de flow indisponible (settings.flow.enabled ?)".to_string())?;
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.clone()
        .send(SupervisorCmd::Deploy { artifact, meta, reply: reply_tx })
        .await
        .map_err(|_| "superviseur de flow arrêté".to_string())?;
    reply_rx.await.map_err(|_| "superviseur de flow injoignable".to_string())?
}

/// État du runtime lu depuis `<state_dir>/runtime.json` (+ test de vie du
/// pid). Best-effort : état absent → `running=false`.
pub fn runtime_status(settings: &FlowSettings) -> FlowRuntimeStatus {
    let path = state_dir_of(settings).join("runtime.json");
    let mut status = FlowRuntimeStatus { running: false, ..Default::default() };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return status;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return status;
    };
    status.pid = v.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32);
    status.restarts = v.get("redeploys").and_then(|r| r.as_u64()).unwrap_or(0);
    status.deployed_flow_id = v.get("flow_id").and_then(|f| f.as_i64());
    status.deployed_version_number = v.get("version_number").and_then(|n| n.as_i64());
    let alive = status.pid.is_some_and(pid_alive);
    status.running = alive && v.get("running").and_then(|r| r.as_bool()).unwrap_or(false);
    status
}

fn state_dir_of(settings: &FlowSettings) -> PathBuf {
    // Chemin relatif résolu contre le cwd (cohérence dev/worker).
    PathBuf::from(&settings.state_dir)
}

// ─────────────────────────── Run once (SIGUSR2) ───────────────────────────

/// Seq monotone des commandes run-once de ce process backend.
static RUN_ONCE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Acks run-once en attente : seq → émetteur du oneshot du handler HTTP.
/// Enregistré **avant** SIGUSR2 (course signal/ack), dépilé par
/// [`resolve_run_once`] depuis `pump_lines`.
static PENDING_RUN_ONCE: OnceLock<
    std::sync::Mutex<std::collections::HashMap<u64, oneshot::Sender<Result<pnex_core::RunOnceResult, String>>>>,
> = OnceLock::new();

fn pending_run_once(
) -> &'static std::sync::Mutex<std::collections::HashMap<u64, oneshot::Sender<Result<pnex_core::RunOnceResult, String>>>> {
    PENDING_RUN_ONCE.get_or_init(std::sync::Mutex::default)
}

/// Exécution manuelle d'un flow déployé : écrit `cmd.json` + SIGUSR2, puis
/// attend l'acquittement stdout corrélé par seq. L'attente a lieu dans une
/// tâche spawnée par la boucle (jamais dans `run_supervisor` — sinon un
/// run-once bloquerait les deploys jusqu'au timeout).
pub async fn run_once(flow_id: i64) -> Result<pnex_core::RunOnceResult, String> {
    use std::sync::atomic::Ordering;
    let tx = SUPERVISOR_TX
        .get()
        .ok_or_else(|| "runtime de flow indisponible (settings.flow.enabled ?)".to_string())?;
    let seq = RUN_ONCE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.clone()
        .send(SupervisorCmd::RunOnce { flow_id, seq, reply: reply_tx })
        .await
        .map_err(|_| "superviseur de flow arrêté".to_string())?;
    let result = reply_rx
        .await
        .map_err(|_| "superviseur de flow injoignable".to_string())??;
    Ok(result)
}

/// Dépile un ack stdout `run_once_done`/`run_once_failed` corrélé par seq.
fn resolve_run_once(v: &serde_json::Value) {
    let Some(seq) = v.get("seq").and_then(|s| s.as_u64()) else { return };
    let mut pending = pending_run_once().lock().expect("lock pending run_once");
    if let Some(tx) = pending.remove(&seq) {
        match v.get("event").and_then(|e| e.as_str()) {
            Some("run_once_done") => {
                let _ = tx.send(Ok(pnex_core::RunOnceResult {
                    injected: v.get("injected").and_then(|i| i.as_u64()).unwrap_or(0) as u32,
                    nodes: v.get("nodes").and_then(|n| n.as_u64()).map(|n| n as u32),
                }));
            }
            _ => {
                let err = v
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or("run_once_failed")
                    .to_string();
                let _ = tx.send(Err(err));
            }
        }
    }
}

// ───────────────────────────── Boucle interne ─────────────────────────────

struct ChildProc {
    pid: u32,
    /// SIGUSR1 envoyés à CET enfant (acquittés par le compteur `redeploys`
    /// de runtime.json — ou par la version projetée, pour le vrai runtime).
    signals_sent: u64,
    /// Récepteur de fin de vie (description de l'exit — le watcher récolte
    /// le process dans sa propre tâche).
    exit_rx: std_mpsc::Receiver<String>,
}

async fn run_supervisor(settings: FlowSettings, mut rx: mpsc::Receiver<SupervisorCmd>) {
    let dir = state_dir_of(&settings);
    let flows_path = dir.join("flows.json");
    let runtime_json = dir.join("runtime.json");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::error!(dir=%dir.display(), error=%e, "répertoire d'état des flows inaccessible");
        return;
    }

    // Reprise au boot : un artefact existant (restart backend) → l'enfant
    // reprend l'ingestion en cours.
    let mut child: Option<ChildProc> = None;
    let mut runtime_wanted = flows_path.exists();
    let mut started_at: Option<tokio::time::Instant> = None;
    let mut backoff = settings.restart_backoff_secs;
    let mut respawn_at = tokio::time::Instant::now();
    loop {
        // 1) Un enfant est-il mort ? → reprise avec backoff exponentiel borné
        // (reset si l'enfant a tenu assez longtemps pour être jugé stable).
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
            match spawn_child(&settings, &flows_path) {
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
            Ok(SupervisorCmd::RunOnce { flow_id, seq, reply }) => {
                // Ne bloque jamais la boucle : l'attente d'ack est spawnée.
                handle_run_once(&settings, &child, flow_id, seq, reply);
            }
            Ok(SupervisorCmd::Deploy { artifact, meta, reply }) => {
                let outcome =
                    handle_deploy(&settings, &flows_path, &runtime_json, &mut child, &artifact, meta).await;
                if outcome.is_ok() {
                    runtime_wanted = true;
                    started_at = Some(tokio::time::Instant::now());
                    backoff = settings.restart_backoff_secs;
                }
                let _ = reply.send(outcome.map(|_| ()));
            }
            Err(TryRecvError::Disconnected) => {
                tracing::info!("superviseur de flows : canal fermé, arrêt");
                return;
            }
            Err(TryRecvError::Empty) => {}
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Exécution d'une commande run-once : vivant ? → responder enregistré
/// **avant** le signal (course signal/ack) → `cmd.json` atomique → SIGUSR2 →
/// attente d'ack dans une tâche spawnée (timeout `reload_ack_secs`).
fn handle_run_once(
    settings: &FlowSettings,
    child: &Option<ChildProc>,
    flow_id: i64,
    seq: u64,
    reply: oneshot::Sender<Result<pnex_core::RunOnceResult, String>>,
) {
    let Some(c) = child.as_ref().filter(|c| pid_alive(c.pid)) else {
        let _ = reply.send(Err("runtime de flow arrêté".into()));
        return;
    };
    let (ack_tx, ack_rx) = oneshot::channel();
    pending_run_once().lock().expect("lock pending run_once").insert(seq, ack_tx);

    let dir = state_dir_of(settings);
    let cmd_path = dir.join("cmd.json");
    let tmp = dir.join("cmd.json.tmp");
    let cmd = serde_json::json!({ "seq": seq, "flow": format!("pnexflow{flow_id}") });
    let written = std::fs::write(&tmp, cmd.to_string()).and_then(|_| std::fs::rename(&tmp, &cmd_path));
    if let Err(e) = written {
        pending_run_once().lock().expect("lock pending run_once").remove(&seq);
        let _ = reply.send(Err(format!("écriture de {} : {e}", cmd_path.display())));
        return;
    }
    if let Err(e) = signal_usr2(c.pid) {
        pending_run_once().lock().expect("lock pending run_once").remove(&seq);
        let _ = reply.send(Err(format!("SIGUSR2 vers le pid {} : {e}", c.pid)));
        return;
    }
    let ack_secs = settings.reload_ack_secs;
    tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_secs(ack_secs), ack_rx).await {
            Ok(Ok(outcome)) => {
                let _ = reply.send(outcome);
            }
            Ok(Err(_)) => {
                let _ = reply.send(Err("superviseur de flow injoignable".into()));
            }
            Err(_) => {
                pending_run_once().lock().expect("lock pending run_once").remove(&seq);
                let _ = reply.send(Err(format!("aucun acquittement de run-once après {ack_secs} s")));
            }
        }
    });
}

/// SIGUSR2 : commande run-once (même contrat que SIGUSR1/rechargement).
fn signal_usr2(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR2) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err("run-once par signal non supporté sur cette plateforme".into())
    }
}

/// Déploiement d'un artefact. Retourne `Ok(true)` si un enfant a été
/// (re)lancé par ce deploy, `Ok(false)` s'il tournait déjà.
async fn handle_deploy(
    settings: &FlowSettings,
    flows_path: &std::path::Path,
    runtime_json: &std::path::Path,
    child: &mut Option<ChildProc>,
    artifact: &Value,
    meta: FlowArtifactMeta,
) -> Result<bool, String> {
    // 1) Écriture atomique de l'artefact (tmp + rename).
    let tmp = flows_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(artifact).map_err(|e| e.to_string())?)
        .map_err(|e| format!("écriture de {} : {e}", tmp.display()))?;
    std::fs::rename(&tmp, flows_path).map_err(|e| format!("renommage de {} : {e}", flows_path.display()))?;

    // Feed debug purgé AVANT le signal : les entrées de la version
    // antérieure meurent avec l'artefact qu'elles reflètent, et les entrées
    // fraîches (émises pendant le rechargement) survivent.
    clear_debug_feed(meta.flow_id);

    // 2) Enfant vivant ? Sinon spawn (premier deploy ou reprise après crash).
    let alive = match child {
        Some(c) => pid_alive(c.pid),
        None => false,
    };
    if !alive {
        match spawn_child(settings, flows_path) {
            Some(c) => {
                *child = Some(c);
            }
            None => return Err("impossible de démarrer le runtime de flow".into()),
        }
    }
    let pid = child.as_ref().expect("enfant vivant").pid;

    // 3) SIGUSR1 — sauf au tout premier spawn, qui lit déjà le fichier frais.
    let mut signals_sent = 0;
    if alive {
        if let Err(e) = signal_reload(pid) {
            return Err(format!("SIGUSR1 vers le pid {pid} : {e}"));
        }
        signals_sent = 1;
    }
    if let Some(c) = child.as_mut() {
        c.signals_sent += signals_sent;
    }

    // 4) Acquittement : le runtime met à jour runtime.json — soit avec la
    //    version projetée (vrai runtime), soit en incrémentant son compteur
    //    de rechargements (contrat générique, faux runtime de test). On
    //    attend l'un ou l'autre.
    let expected_signals = child.as_ref().map(|c| c.signals_sent).unwrap_or(0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(settings.reload_ack_secs);
    while tokio::time::Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(runtime_json) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                let version_ok = v.get("version_number").and_then(|n| n.as_i64()) == Some(meta.version_number)
                    && v.get("flow_id").and_then(|n| n.as_i64()) == Some(meta.flow_id);
                let redeploys = v.get("redeploys").and_then(|r| r.as_u64()).unwrap_or(0);
                if version_ok || redeploys >= expected_signals {
                    tracing::info!(
                        flow_id = meta.flow_id,
                        version = meta.version_number,
                        pid,
                        "flow déployé"
                    );
                    return Ok(!alive);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!(
        "acquittement de rechargement absent après {} s (pid {pid})",
        settings.reload_ack_secs
    ))
}

/// Spawn de l'enfant + pumps stdout/stderr → tracing. Retourne `None` si le
/// lancement échoue (déjà logué).
fn spawn_child(settings: &FlowSettings, flows_path: &std::path::Path) -> Option<ChildProc> {
    // Purge d'une cmd.json périmée : un enfant relancé ne doit jamais rejouer
    // une commande run-once d'avant-crash.
    let _ = std::fs::remove_file(state_dir_of(settings).join("cmd.json"));
    let program = resolve_program(&settings.runtime_cmd);
    tracing::info!(cmd=%program, flows=%flows_path.display(), "démarrage du runtime de flow");
    let mut cmd = tokio::process::Command::new(&program);
    cmd.arg(flows_path)
        .arg("--home")
        .arg(state_dir_of(settings))
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default())
        .env("PNEX_FLOW_LOG", std::env::var("PNEX_FLOW_LOG").unwrap_or_else(|_| "info".into()))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Allowlist : seules ces variables franchissent la frontière process.
    for key in &settings.env_allowlist {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    // Credentials OpenObserve : le serveur les tient du yaml
    // (`settings.openobserve`), pas de son env process — l'allowlist seule ne
    // transmettrait jamais rien (retour e2e 2026-09-04 : nœud device en
    // échec au build, moteur en crash-loop sans acquittement). Même domaine
    // de confiance que DATABASE_URL : le runtime lit/écrit la télémétrie
    // (nœuds device/metric) pour le compte du serveur.
    if let Some(o2) = &settings.o2 {
        cmd.env("OPENOBSERVE_URL", o2.base_url.clone())
            .env("OPENOBSERVE_ROOT_EMAIL", o2.root_email.clone())
            .env("OPENOBSERVE_ROOT_PASSWORD", o2.root_password.clone());
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
    tokio::spawn(pump_lines(stdout, tracing::Level::INFO));
    tokio::spawn(pump_lines(stderr, tracing::Level::WARN));

    // Watcher de fin de vie : `child.wait()` en tâche dédiée pour garder la
    // boucle de supervision libre (et récolter le process — pas de zombie).
    let (exit_tx, exit_rx) = std_mpsc::channel();
    tokio::spawn(async move {
        let status = child.wait().await;
        let _ = exit_tx.send(status.map_or_else(|e| e.to_string(), |s| s.to_string()));
    });
    Some(ChildProc { pid, signals_sent: 0, exit_rx })
}

async fn pump_lines(stream: impl tokio::io::AsyncRead + Unpin, level: tracing::Level) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Seul le stdout (INFO) est parsé : les événements consommés par le
        // feed/acks descendent en `debug` pour éviter le bruit INFO.
        if level == tracing::Level::INFO && handle_runtime_line(&line) {
            tracing::debug!(runtime=%line, "flow runtime");
            continue;
        }
        match level {
            tracing::Level::INFO => tracing::info!(runtime=%line, "flow runtime"),
            _ => tracing::warn!(runtime=%line, "flow runtime"),
        }
    }
}

fn signal_reload(pid: u32) -> Result<(), String> {
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

/// Le pid existe-t-il (signal 0) ? Tolérant aux plateformes sans support.
fn pid_alive(pid: u32) -> bool {
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
fn resolve_program(token: &str) -> String {
    use std::path::Path;
    const REPO_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    if !token.contains('/') {
        return token.to_string();
    }
    let try_anchor = |anchor: &Path| -> Option<String> {
        anchor.join(token).canonicalize().ok().map(|p| p.display().to_string())
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
mod tests {
    use super::*;

    #[test]
    fn resolve_program_chemins() {
        // Token sans `/` : recherche PATH, laissé tel quel.
        assert_eq!(resolve_program("pnex-flow-runtime"), "pnex-flow-runtime");
        // Chemin inexistant : tel quel (erreur claire au spawn).
        assert_eq!(resolve_program("./nulle-part/xyz"), "./nulle-part/xyz");
        // Chemin du repo : résolu contre la racine monorepo.
        let abs = resolve_program("crates/pnex-node-sql/Cargo.toml");
        assert!(abs.contains("Cargo.toml"), "{abs}");
        assert!(std::path::Path::new(&abs).exists(), "{abs}");
    }

    #[test]
    fn runtime_status_sans_etat() {
        let s = FlowSettings { state_dir: "/tmp/pnex-flow-status-absente".into(), ..Default::default() };
        let st = runtime_status(&s);
        assert!(!st.running);
        assert_eq!(st.pid, None);
    }

    fn debug_line(flow: i64, msg: &str) -> serde_json::Value {
        serde_json::json!({
            "event": "debug", "node": "deadbeef", "node_red": "n2",
            "flow": flow, "name": "n2", "msg": msg, "msgid": "m1"
        })
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
    fn resolve_run_once_seq_inconnue_ignorer() {
        // Aucun responder enregistré pour cette seq : ne doit pas paniquer.
        resolve_run_once(&serde_json::json!({
            "event": "run_once_done", "seq": 987_654_321, "injected": 3, "nodes": 1
        }));
        resolve_run_once(&serde_json::json!({
            "event": "run_once_failed", "seq": 987_654_322, "error": "flow_absent"
        }));
        // Sans seq : ignoré.
        resolve_run_once(&serde_json::json!({"event": "run_once_done", "injected": 1}));
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
