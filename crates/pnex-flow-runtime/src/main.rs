//! `pnex-flow-runtime` — binaire headless du moteur de flow ETL PNEX (D18).
//!
//! Notre propre `main` au-dessus de `edgelink-core` (jamais le `edgelinkd`
//! upstream, dont l'API admin web ne doit jamais être exposée) :
//! - **stdout = événements JSON-lines machine** (`started`, `debug`,
//!   `redeployed`, `stopped`…) consommés par le superviseur Loco ;
//! - **stderr = logs** du moteur en JSON-lines ;
//! - **SIGUSR1 = rechargement à chaud** : relecture du `flows.json` puis
//!   `Engine::redeploy_flows` (aucune coupure d'ingestion, aucune surface
//!   HTTP). Échec de redéploiement → sortie code 1 → le superviseur relance ;
//! - **ctrl_c/SIGTERM via ctrl_c** = arrêt propre (`Engine::stop`).
//!
//! Les nœuds custom PNEX (`pnex-node-sql` et suivants) s'enregistrent tout
//! seuls à l'édition de liens (`inventory`) — la référence explicite
//! [`pnex_node_sql::registered`] est un garde-fou anti-élagage.
//!
//! Usage : `pnex-flow-runtime <flows.json> [--home <dir>]`

mod logger;
mod state;

use std::path::PathBuf;
use std::process::ExitCode;

use edgelink_core::runtime::engine::Engine;
use edgelink_core::runtime::registry::RegistryBuilder;

fn main() -> ExitCode {
    let logger = logger::JsonLogger::from_env();
    log::set_boxed_logger(Box::new(logger)).expect("logger unique");
    log::set_max_level(logger::max_level_from_env());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime tokio");
    runtime.block_on(run())
}

fn usage() -> ExitCode {
    eprintln!("usage: pnex-flow-runtime <flows.json> [--home <dir>]");
    ExitCode::from(2)
}

async fn run() -> ExitCode {
    let mut flows_path: Option<String> = None;
    let mut home = PathBuf::from("./flow-state");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--home" => match args.next() {
                Some(h) => home = PathBuf::from(h),
                None => return usage(),
            },
            other if other.starts_with('-') => {
                eprintln!("option inconnue : {other}");
                return usage();
            }
            other => flows_path = Some(other.to_string()),
        }
    }
    let Some(flows_path) = flows_path else { return usage() };

    // Garde-fou d'édition de liens : les nœuds PNEX s'enregistrent via
    // inventory ; cette référence garantit leur inclusion dans le binaire.
    pnex_node_sql::registered();
    // Nœuds Phase 6 (device/calc/metric) — même garde-fou anti-élagage.
    pnex_node_device::registered();

    let reg = match RegistryBuilder::default().build() {
        Ok(r) => r,
        Err(e) => {
            log::error!("Registre de nœuds indisponible : {e}");
            return ExitCode::FAILURE;
        }
    };

    let engine = match Engine::with_flows_file(&reg, &flows_path, None).await {
        Ok(e) => e,
        Err(e) => {
            log::error!("flows.json illisible : {flows_path} : {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = engine.start().await {
        log::error!("Démarrage du moteur impossible : {e}");
        return ExitCode::FAILURE;
    }

    // Métadonnées de version lues depuis le tab du flows.json (auto-descriptif).
    let (mut flow_id, mut version_number) = meta_of_file(&flows_path).await;
    let mut redeploys: u64 = 0;
    let mut st = state::RuntimeState {
        pid: std::process::id(),
        running: true,
        started_at: logger::epoch_secs(),
        flow_rev: Some(engine.flows_rev().await),
        redeploys,
        flow_id,
        version_number,
    };
    state::write(&home, &st);
    emit("started", serde_json::json!({
        "pid": st.pid,
        "flow_rev": st.flow_rev,
        "flow_id": flow_id,
        "version": version_number,
    }));

    // Pump debug + événements moteur → stdout (contrat superviseur).
    let debug_rx = engine.debug_channel().subscribe();
    let events_rx = engine.subscribe_events();
    tokio::spawn(pump_debug(debug_rx));
    tokio::spawn(pump_events(events_rx));

    // SIGUSR1 = redeploy (unix). En l'absence (non-unix), seul ctrl_c arrête.
    #[cfg(unix)]
    let mut sigusr1 = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Installation de SIGUSR1 impossible : {e}");
            return ExitCode::FAILURE;
        }
    };
    #[cfg(not(unix))]
    let mut sigusr1 = ();

    loop {
        match next_action(&mut sigusr1).await {
            Action::Stop => break,
            Action::Reload => match reload(&engine, &reg, &flows_path).await {
                Ok(rev) => {
                    redeploys += 1;
                    (flow_id, version_number) = meta_of_file(&flows_path).await;
                    st.redeploys = redeploys;
                    st.flow_rev = Some(rev.clone());
                    st.flow_id = flow_id;
                    st.version_number = version_number;
                    state::write(&home, &st);
                    emit("redeployed", serde_json::json!({
                        "flow_rev": rev,
                        "flow_id": flow_id,
                        "version": version_number,
                    }));
                }
                Err(e) => {
                    // On refuse de tourner sur un graphe incohérent :
                    // exit(1) → le superviseur relance avec le fichier courant.
                    emit("redeploy_failed", serde_json::json!({ "error": e.to_string() }));
                    let _ = engine.stop().await;
                    return ExitCode::FAILURE;
                }
            },
        }
    }

    st.running = false;
    state::write(&home, &st);
    let _ = engine.stop().await;
    emit("stopped", serde_json::json!({ "redeploys": redeploys }));
    ExitCode::SUCCESS
}

/// Métadonnées de version (`pnex_flow_id`/`pnex_version` du tab) depuis le
/// fichier projeté — best-effort : un flows.json inconnu ne bloque rien.
async fn meta_of_file(flows_path: &str) -> (Option<i64>, Option<i64>) {
    match tokio::fs::read_to_string(flows_path).await {
        Ok(s) => meta_of_json(&s),
        Err(_) => (None, None),
    }
}

fn meta_of_json(json: &str) -> (Option<i64>, Option<i64>) {
    let Ok(serde_json::Value::Array(entries)) = serde_json::from_str::<serde_json::Value>(json) else {
        return (None, None);
    };
    let tab = entries.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("tab"));
    (
        tab.and_then(|t| t.get("pnex_flow_id")).and_then(|v| v.as_i64()),
        tab.and_then(|t| t.get("pnex_version")).and_then(|v| v.as_i64()),
    )
}

/// Prochain événement d'ordonnancement du runtime. `select!` de tokio
/// n'acceptant pas de `#[cfg]` sur ses branches, la gestion de SIGUSR1
/// (unix) est isolée ici.
enum Action {
    Stop,
    Reload,
}

#[cfg(unix)]
async fn next_action(sigusr1: &mut tokio::signal::unix::Signal) -> Action {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => Action::Stop,
        _ = sigusr1.recv() => Action::Reload,
    }
}

#[cfg(not(unix))]
async fn next_action((): ()) -> Action {
    let _ = tokio::signal::ctrl_c().await;
    Action::Stop
}

async fn reload(
    engine: &Engine,
    reg: &edgelink_core::runtime::registry::RegistryHandle,
    flows_path: &str,
) -> edgelink_core::Result<String> {
    let json = tokio::fs::read_to_string(flows_path).await?;
    let value: serde_json::Value = serde_json::from_str(&json)?;
    engine.redeploy_flows(value, reg, None).await?;
    let rev = engine.flows_rev().await;
    Ok(rev)
}

fn emit(event: &str, mut fields: serde_json::Value) {
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("event".into(), serde_json::json!(event));
        obj.insert("ts".into(), serde_json::json!(logger::epoch_secs()));
    }
    println!("{fields}");
}

async fn pump_debug(mut rx: tokio::sync::broadcast::Receiver<edgelink_core::runtime::debug_channel::DebugMessage>) {
    loop {
        match rx.recv().await {
            Ok(m) => emit(
                "debug",
                serde_json::json!({
                    "node": m.id,
                    "name": m.name,
                    "msg": m.msg,
                    "msgid": m.msgid,
                }),
            ),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("canal debug saturé : {n} messages perdus");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn pump_events(mut rx: tokio::sync::broadcast::Receiver<edgelink_core::runtime::engine_events::EngineEvent>) {
    use edgelink_core::runtime::engine_events::EngineEvent as E;
    loop {
        let ev = match rx.recv().await {
            Ok(ev) => ev,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("canal d'événements saturé : {n} perdus");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        let name = match ev {
            E::EngineStarted => "engine_started",
            E::EngineStopped => "engine_stopped",
            E::EngineRestartStarted => "engine_restart_started",
            E::EngineRestartCompleted => "engine_restart_completed",
            E::DebugChannelReinitialized => "debug_channel_reinitialized",
            E::FlowDeploymentStarted => "flow_deployment_started",
            E::FlowDeploymentCompleted => "flow_deployment_completed",
            E::Custom { event_type, .. } => {
                emit("engine_custom", serde_json::json!({ "type": event_type }));
                continue;
            }
        };
        emit("engine", serde_json::json!({ "name": name }));
    }
}
