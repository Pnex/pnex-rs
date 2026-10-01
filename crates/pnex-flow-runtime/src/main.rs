//! `pnex-flow-runtime` — binaire headless du moteur de flow ETL PNEX (D18).
//!
//! Notre propre `main` au-dessus de `edgelink-core` (jamais le `edgelinkd`
//! upstream, dont l'API admin web ne doit jamais être exposée) :
//! - **ferme d'engines** : un `Engine` edgelinkd **par flow** (1 tab = 1
//!   engine, cf. `engines.rs`) — un tab invalide ne fait échouer que son
//!   engine, les autres continuent (plus d'exit(1) global pour un flow
//!   fautif) ;
//! - **stdout = événements JSON-lines machine** (`started`, `flow_started`,
//!   `flow_error`, `debug`, `redeployed`, `stopped`…) consommés par le
//!   superviseur Loco ;
//! - **stderr = logs** du moteur en JSON-lines (+ paniques via hook) ;
//! - **SIGUSR1 = rechargement à chaud** : relecture du `flows.json`, diff
//!   par tab — seuls les engines des tabs modifiés sont swappés (aucune
//!   pause d'ingestion pour les autres). Échec **fichier** (lecture/JSON) →
//!   `reload_failed` et les engines courants sont conservés ;
//! - **ctrl_c/SIGTERM via ctrl_c** = arrêt propre (stop de tous les engines).
//! - **`--check <flows.json>`** = pré-flight du deploy : construit chaque
//!   engine sans `start()`, rapport par tab sur stdout, exit 0/1.
//! - **`--check-function <request.json>`** = validation compile-only d'une
//!   fonction du registre (barre d'erreurs de l'éditeur) — une ligne JSON
//!   `FunctionValidateResponse`, exit 0/1.
//!
//! PNEX custom nodes register themselves at link time (`inventory`) — the
//! explicit `pnex_node_*::registered()` references are anti-stripping guards.
//!
//! Usage : `pnex-flow-runtime <flows.json> [--home <dir>]`
//!         `pnex-flow-runtime --check <flows.json>`
//!         `pnex-flow-runtime --test-function <request.json>`
//!         `pnex-flow-runtime --check-function <request.json>`

mod attrib;
mod check_fn;
mod engines;
mod logger;
mod state;
mod test_fn;

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use edgelink_core::runtime::registry::RegistryBuilder;

fn main() -> ExitCode {
    let logger = logger::JsonLogger::from_env();
    log::set_boxed_logger(Box::new(logger)).expect("logger unique");
    log::set_max_level(logger::max_level_from_env());

    // Paniques → stderr JSON (canal logs du superviseur). Une panic dans une
    // tâche nœud ne tue pas le process (frontière tokio) mais meurt
    // silencieusement côté vendor — le hook la rend au moins visible.
    std::panic::set_hook(Box::new(|info| {
        let line = serde_json::json!({
            "ts": logger::epoch_secs(),
            "level": "ERROR",
            "target": "panic",
            "message": format!("panique dans une tâche : {info}"),
        });
        let _ = writeln!(std::io::stderr(), "{line}");
    }));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime tokio");
    runtime.block_on(run())
}

fn usage() -> ExitCode {
    eprintln!("usage: pnex-flow-runtime <flows.json> [--home <dir>]");
    eprintln!("       pnex-flow-runtime --check <flows.json>");
    eprintln!("       pnex-flow-runtime --test-function <request.json>");
    eprintln!("       pnex-flow-runtime --check-function <request.json>");
    ExitCode::from(2)
}

async fn run() -> ExitCode {
    let mut flows_path: Option<String> = None;
    let mut home = PathBuf::from("./flow-state");
    let mut check = false;
    let mut test_fn_path: Option<String> = None;
    let mut check_fn_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--test-function" => match args.next() {
                Some(p) => test_fn_path = Some(p),
                None => return usage(),
            },
            "--check-function" => match args.next() {
                Some(p) => check_fn_path = Some(p),
                None => return usage(),
            },
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
    // Mode « test en live » : court-circuite le registre/moteur — la
    // fonction est exécutée sandboxée et une ligne JSON est imprimée.
    if let Some(path) = test_fn_path {
        return test_fn::run(&path).await;
    }
    // Mode « validation compile-only » : même court-circuit, sans exécution
    // du code — diagnostics structurés en une ligne JSON.
    if let Some(path) = check_fn_path {
        return check_fn::run(&path).await;
    }
    let Some(flows_path) = flows_path else {
        return usage();
    };

    // Link-time guard: PNEX nodes register through inventory; these
    // references guarantee their inclusion in the binary.
    // Nœuds Phase 6 (device/calc/metric) — même garde-fou anti-élagage.
    pnex_node_device::registered();
    // Sonde (panneau debug + badge éditeur) — même garde-fou.
    pnex_node_display::registered();
    // Fixed or random payload (pnex-value) — same anti-stripping guard.
    pnex_node_value::registered();
    // Cartes de régulation mixtes (TT heat/cool, PID) — sans ce crate lié,
    // le type inconnu tomberait dans le fallback `unknown` (no-op silencieux).
    pnex_node_control::registered();
    // Propriétés thermophysiques in-process (pnex-coolprop) — même garde-fou.
    pnex_node_coolprop::registered();
    // Notifications multi-canaux (D49–D54) — même garde-fou.
    pnex_node_notify::registered();
    // Requête HTTP client (C1a, amendement edge-model 2026-09-15) — même garde-fou.
    pnex_node_http_fetch::registered();
    // Camera frame source + video recorder (camera-video.md D78).
    pnex_node_camera::registered();
    // JSON event log → OpenObserve logs (D84).
    pnex_node_events::registered();
    // Object detection on camera frames (D83).
    pnex_node_vision::registered();
    // Org shared memory (Valkey): memory-write / memory-read.
    pnex_node_memory::registered();
    // Anomaly scoring + forecasting on telemetry series (augurs).
    pnex_node_predict::registered();
    // Fonctions Starlark du registre « Fonctions » — même garde-fou.
    pnex_node_starlark::registered();

    let reg = match RegistryBuilder::default().build() {
        Ok(r) => r,
        Err(e) => {
            log::error!("Registre de nœuds indisponible : {e}");
            return ExitCode::FAILURE;
        }
    };

    if check {
        return run_check(&reg, &flows_path).await;
    }
    run_serve(reg, flows_path, home).await
}

/// Pré-flight du deploy : construit chaque engine **sans `start()`** — ni
/// tâche nœud, ni effet de bord ; `--home` accepté mais inutile. Rapport
/// par tab sur stdout, exit 0/1.
async fn run_check(
    reg: &edgelink_core::runtime::registry::RegistryHandle,
    flows_path: &str,
) -> ExitCode {
    let (tabs, _rev) = match engines::read_tabs(flows_path).await {
        Ok(t) => t,
        Err(e) => {
            // Fichier illisible / JSON invalide / structure non groupable.
            emit(
                "check_done",
                serde_json::json!({ "ok": false, "total": 0, "errors": 0, "error": e }),
            );
            return ExitCode::FAILURE;
        }
    };
    let mut errors = 0u32;
    for tab in &tabs {
        match engines::build_engine(reg, tab) {
            Ok(_) => emit(
                "check_flow",
                serde_json::json!({ "flow": tab.flow_id, "tab": tab.tab_id, "ok": true }),
            ),
            Err(e) => {
                errors += 1;
                emit(
                    "check_flow",
                    serde_json::json!({ "flow": tab.flow_id, "tab": tab.tab_id, "ok": false, "error": e }),
                );
            }
        }
    }
    emit(
        "check_done",
        serde_json::json!({ "ok": errors == 0, "total": tabs.len(), "errors": errors }),
    );
    if errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

async fn run_serve(
    reg: edgelink_core::runtime::registry::RegistryHandle,
    flows_path: String,
    home: PathBuf,
) -> ExitCode {
    // Échec **fichier** au boot : exit(1) — le superviseur relance avec
    // backoff (même comportement qu'avant la ferme).
    let (tabs, rev) = match engines::read_tabs(&flows_path).await {
        Ok(t) => t,
        Err(e) => {
            log::error!("flows.json illisible : {flows_path} : {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut farm = engines::EngineFarm::new(reg);
    // Issues émises à la volée (stdout → acquittement deploy) : un tab lent
    // au boot ne retarde pas l'annonce des suivants.
    farm.boot(tabs, &mut emit_outcome_streamed).await;

    // Pas de méta de version ici : l'artefact est multi-flows et le
    // runtime.json ne porte que la santé du process — la version déployée
    // d'un flow vit en DB, tenue par le deploy acquitté.
    let mut redeploys: u64 = 0;
    let mut st = state::RuntimeState {
        pid: std::process::id(),
        running: true,
        started_at: logger::epoch_secs(),
        flow_rev: Some(rev),
        redeploys,
    };
    state::write(&home, &st);
    emit(
        "started",
        serde_json::json!({
            "pid": st.pid,
            "flow_rev": st.flow_rev,
        }),
    );

    // Pompe debug par engine (chaque engine a son propre canal broadcast) —
    // (re)lancée après chaque cycle boot/reload pour les engines nouveaux ou
    // swappés (l'ancienne pompe meurt d'elle-même : canal fermé au drop).
    let mut pumps: HashMap<i64, tokio::task::JoinHandle<()>> = HashMap::new();
    reconcile_pumps(&farm, &mut pumps, &[]);

    // SIGUSR1 = reload diff (unix). En l'absence (non-unix), seul ctrl_c
    // arrête.
    #[cfg(unix)]
    let mut sigusr1 =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
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
            Action::Reload => match engines::read_tabs(&flows_path).await {
                Err(e) => {
                    // Fichier illisible/JSON invalide : on garde les engines
                    // courants (jamais d'exit pour un artefact fautif — le
                    // superviseur re-signalisera au prochain deploy).
                    emit("reload_failed", serde_json::json!({ "error": e }));
                }
                Ok((tabs, rev)) => {
                    let report = farm.reload(tabs, &mut emit_outcome_streamed).await;
                    // Compteur incrémenté par cycle appliqué (artefact lisible),
                    // même sans changement — compat acquittement supervisor.
                    redeploys += 1;
                    st.redeploys = redeploys;
                    st.flow_rev = Some(rev);
                    state::write(&home, &st);
                    emit(
                        "redeployed",
                        serde_json::json!({
                            "changed": report.changed,
                            "flow_rev": st.flow_rev,
                        }),
                    );
                    reconcile_pumps(&farm, &mut pumps, &report.removed);
                }
            },
        }
    }

    st.running = false;
    state::write(&home, &st);
    for handle in pumps.values() {
        handle.abort();
    }
    farm.stop_all().await;
    emit("stopped", serde_json::json!({ "redeploys": redeploys }));
    ExitCode::SUCCESS
}

/// (Re)lance les pompes debug : nouvelles engines après boot/reload, retrait
/// des pompes des flows supprimés. Une pompe swappée meurt d'elle-même
/// (canal de l'ancien engine fermé au drop) — `is_finished` couvre ce cas.
fn reconcile_pumps(
    farm: &engines::EngineFarm,
    pumps: &mut HashMap<i64, tokio::task::JoinHandle<()>>,
    removed: &[i64],
) {
    for fid in removed {
        if let Some(handle) = pumps.remove(fid) {
            handle.abort();
        }
    }
    for fe in farm.engines() {
        let stale = match pumps.get(&fe.flow_id) {
            Some(handle) => handle.is_finished(),
            None => true,
        };
        if stale {
            let rx = fe.engine.debug_channel().subscribe();
            pumps.insert(
                fe.flow_id,
                tokio::spawn(pump_debug(
                    rx,
                    fe.flow_id,
                    std::sync::Arc::new(fe.node_by_hex.clone()),
                )),
            );
        }
    }
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

fn emit(event: &str, mut fields: serde_json::Value) {
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("event".into(), serde_json::json!(event));
        obj.insert("ts".into(), serde_json::json!(logger::epoch_secs()));
    }
    println!("{fields}");
}

fn emit_outcome(oc: &engines::FlowOutcome) {
    match oc {
        engines::FlowOutcome::Started {
            flow_id,
            tab_id,
            rev,
        } => {
            emit(
                "flow_started",
                serde_json::json!({ "flow": flow_id, "tab": tab_id, "rev": rev }),
            );
        }
        engines::FlowOutcome::Failed {
            flow_id,
            tab_id,
            error,
        } => {
            emit(
                "flow_error",
                serde_json::json!({ "flow": flow_id, "tab": tab_id, "error": error }),
            );
        }
        engines::FlowOutcome::Stopped { flow_id, tab_id } => {
            emit(
                "flow_stopped",
                serde_json::json!({ "flow": flow_id, "tab": tab_id }),
            );
        }
    }
}

/// Adaptateur pour les callbacks de `boot`/`reload` (`FnMut` par valeur) —
/// émission immédiate sur stdout de chaque issue de cycle.
fn emit_outcome_streamed(oc: engines::FlowOutcome) {
    emit_outcome(&oc);
}

async fn pump_debug(
    mut rx: tokio::sync::broadcast::Receiver<edgelink_core::runtime::debug_channel::DebugMessage>,
    flow_id: i64,
    node_by_hex: std::sync::Arc<HashMap<String, String>>,
) {
    loop {
        match rx.recv().await {
            Ok(m) => {
                // Id éditeur quand connu (le fallback = m.id brut couvre les
                // nœuds pnex-display, qui s'identifient eux-mêmes).
                let node_red = node_by_hex
                    .get(&m.id)
                    .cloned()
                    .unwrap_or_else(|| m.id.clone());
                let fields = serde_json::json!({
                    "flow": flow_id,
                    "node": m.id,
                    "node_red": node_red,
                    "name": m.name,
                    "msg": m.msg,
                    "msgid": m.msgid,
                    // Marqueur de source : la sonde pnex-display s'identifie
                    // via `format` ; les nœuds debug builtin portent un format
                    // d'affichage (« Number », « Object »…) → tout le reste
                    // reste « debug » (badge + tag cyan du drawer).
                    "source": match m.format.as_deref() {
                        Some("pnex-display") => "pnex-display",
                        // Camera/vision node status (D103).
                        Some(pnex_core::vision::NODE_STATUS_FORMAT) => "pnex-status",
                        _ => "debug",
                    },
                });
                emit("debug", fields);
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                log::warn!("flow {flow_id} : canal debug saturé : {n} messages perdus");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
