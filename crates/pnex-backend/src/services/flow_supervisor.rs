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
}

/// État partagé : l'émetteur vers la boucle (présent seulement si lancé).
static SUPERVISOR_TX: OnceLock<mpsc::Sender<SupervisorCmd>> = OnceLock::new();
/// Garde anti double-spawn : `after_routes` tourne à chaque boot d'app, y
/// compris dans les tests d'intégration (une app = au plus un superviseur).
static SPAWNED: AtomicBool = AtomicBool::new(false);

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
}
