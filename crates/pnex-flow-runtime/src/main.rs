//! `pnex-flow-runtime` — headless binary of the PNEX ETL flow engine (D18).
//!
//! Our own `main` on top of `edgelink-core` (never the upstream `edgelinkd`,
//! whose web admin API must never be exposed):
//! - **engine farm**: one edgelinkd `Engine` **per flow** (1 tab = 1
//!   engine, see `engines.rs`) — an invalid tab only fails its own
//!   engine, the others keep running (no more global exit(1) for a faulty
//!   flow);
//! - **stdout = machine JSON-lines events** (`started`, `flow_started`,
//!   `flow_error`, `debug`, `redeployed`, `stopped`…) consumed by the
//!   Loco supervisor;
//! - **stderr = engine logs** as JSON-lines (+ panics via hook);
//! - **SIGUSR1 = hot reload**: re-reads `flows.json`, diff
//!   per tab — only the engines of modified tabs are swapped (no
//!   ingestion pause for the others). **File** failure (read/JSON) →
//!   `reload_failed` and the current engines are kept;
//! - **ctrl_c/SIGTERM via ctrl_c** = clean shutdown (stop of all engines).
//! - **`--check <flows.json>`** = deploy pre-flight: builds each
//!   engine without `start()`, per-tab report on stdout, exit 0/1.
//! - **`--check-function <request.json>`** = compile-only validation of a
//!   registry function (editor error bar) — one JSON line
//!   `FunctionValidateResponse`, exit 0/1.
//!
//! PNEX custom nodes register themselves at link time (`inventory`) — the
//! explicit `pnex_node_*::registered()` references are anti-stripping guards.
//!
//! Usage: `pnex-flow-runtime <flows.json> [--home <dir>]`
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

    // Panics → JSON stderr (supervisor log channel). A panic in a
    // node task does not kill the process (tokio boundary) but dies
    // silently on the vendor side — the hook at least makes it visible.
    std::panic::set_hook(Box::new(|info| {
        let line = serde_json::json!({
            "ts": logger::epoch_secs(),
            "level": "ERROR",
            "target": "panic",
            "message": format!("panic in a task: {info}"),
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
                eprintln!("unknown option: {other}");
                return usage();
            }
            other => flows_path = Some(other.to_string()),
        }
    }
    // "Live test" mode: bypasses the registry/engine — the
    // function runs sandboxed and one JSON line is printed.
    if let Some(path) = test_fn_path {
        return test_fn::run(&path).await;
    }
    // "Compile-only validation" mode: same bypass, without running
    // the code — structured diagnostics in one JSON line.
    if let Some(path) = check_fn_path {
        return check_fn::run(&path).await;
    }
    let Some(flows_path) = flows_path else {
        return usage();
    };

    // Link-time guard: PNEX nodes register through inventory; these
    // references guarantee their inclusion in the binary.
    // Phase 6 nodes (device/calc/metric) — same anti-pruning guard.
    pnex_node_device::registered();
    // Probe (debug panel + editor badge) — same guard.
    pnex_node_display::registered();
    // Fixed or random payload (pnex-value) — same anti-stripping guard.
    pnex_node_value::registered();
    // Mixed control cards (TT heat/cool, PID) — without this crate linked,
    // the unknown type would fall into the `unknown` fallback (silent no-op).
    pnex_node_control::registered();
    // In-process thermophysical properties (pnex-coolprop) — same guard.
    pnex_node_coolprop::registered();
    // Multi-channel notifications (D49–D54) — same guard.
    pnex_node_notify::registered();
    // Client HTTP request (C1a, edge-model amendment 2026-09-15) — same guard.
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
    // Org controls written from the surfaces (D127): control-source.
    pnex_node_ui_control::registered();
    pnex_node_weather::registered();
    // Starlark functions of the "Functions" registry — same guard.
    pnex_node_starlark::registered();

    let reg = match allowlisted_registry() {
        Ok(r) => r,
        Err(e) => {
            log::error!("Node registry unavailable: {e}");
            return ExitCode::FAILURE;
        }
    };

    if check {
        return run_check(&reg, &flows_path).await;
    }
    run_serve(reg, flows_path, home).await
}

/// Node registry restricted to `pnex_core::runtime_type_allowed` (SEC-1,
/// docs/architecture/security.md R5): every compiled-in builtin with a host
/// effect (`exec`, `template`, …) is dropped, so an artifact naming one fails
/// to load instead of running it.
fn allowlisted_registry() -> edgelink_core::Result<edgelink_core::runtime::registry::RegistryHandle>
{
    let all = RegistryBuilder::default().build()?;
    let mut builder = RegistryBuilder::new();
    for (type_name, meta) in all.all() {
        if pnex_core::runtime_type_allowed(type_name) {
            builder = builder.register(meta);
        } else {
            log::debug!("[REGISTRY] builtin '{type_name}' not allowlisted, dropped");
        }
    }
    builder.build()
}

/// Deploy pre-flight: builds each engine **without `start()`** — no
/// node task, no side effect; `--home` accepted but useless. Report
/// per tab on stdout, exit 0/1.
async fn run_check(
    reg: &edgelink_core::runtime::registry::RegistryHandle,
    flows_path: &str,
) -> ExitCode {
    let (tabs, _rev) = match engines::read_tabs(flows_path).await {
        Ok(t) => t,
        Err(e) => {
            // Unreadable file / invalid JSON / structure that can't be grouped.
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
    // **File** failure at boot: exit(1) — the supervisor restarts with
    // backoff (same behaviour as before the farm).
    let (tabs, rev) = match engines::read_tabs(&flows_path).await {
        Ok(t) => t,
        Err(e) => {
            log::error!("flows.json unreadable: {flows_path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut farm = engines::EngineFarm::new(reg);
    // Outcomes emitted on the fly (stdout → deploy acknowledgement): a tab slow
    // at boot does not delay the announcement of the following ones.
    farm.boot(tabs, &mut emit_outcome_streamed).await;

    // No version meta here: the artifact is multi-flow and
    // runtime.json only carries process health — the deployed version
    // of a flow lives in the DB, kept by the acknowledged deploy.
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

    // Debug pump per engine (each engine has its own broadcast channel) —
    // (re)started after each boot/reload cycle for new or swapped
    // engines (the old pump dies by itself: channel closed on drop).
    let mut pumps: HashMap<i64, tokio::task::JoinHandle<()>> = HashMap::new();
    reconcile_pumps(&farm, &mut pumps, &[]);

    // SIGUSR1 = diff reload (unix). Without it (non-unix), only ctrl_c
    // stops.
    #[cfg(unix)]
    let mut sigusr1 =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Cannot install SIGUSR1: {e}");
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
                    // Unreadable file/invalid JSON: keep the current
                    // engines (never exit for a faulty artifact — the
                    // supervisor will re-signal on the next deploy).
                    emit("reload_failed", serde_json::json!({ "error": e }));
                }
                Ok((tabs, rev)) => {
                    let report = farm.reload(tabs, &mut emit_outcome_streamed).await;
                    // Counter incremented per applied cycle (readable artifact),
                    // even with no change — supervisor acknowledgement compat.
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

/// (Re)starts the debug pumps: new engines after boot/reload, removal
/// of the pumps of deleted flows. A swapped pump dies by itself
/// (old engine's channel closed on drop) — `is_finished` covers that case.
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

/// Next scheduling event of the runtime. Since tokio's `select!`
/// does not accept `#[cfg]` on its branches, SIGUSR1 handling
/// (unix) is isolated here.
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

/// Adapter for the `boot`/`reload` callbacks (`FnMut` by value) —
/// immediate stdout emission of each cycle outcome.
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
                // Editor id when known (the fallback = raw m.id covers the
                // pnex-display nodes, which identify themselves).
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
                    // Source marker: the pnex-display probe identifies itself
                    // via `format`; builtin debug nodes carry a display
                    // format ("Number", "Object"…) → everything else
                    // stays "debug" (badge + cyan tag of the drawer).
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
                log::warn!("flow {flow_id}: debug channel saturated: {n} messages lost");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
