//! Flow editor deploy, runtime controls, runtime/debug polls and unified status.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{Flow, FlowRuntimeStatus};

use super::cx::RuntimeAction;
use super::debug;
use crate::api;
use crate::api::error::ApiError;
use crate::components::editor_shell::{EditorStatus, StatusTone};
use crate::state::toasts;

/// Deploy click handler (deploys the version shown in the editor).
pub(super) fn deploy_action(
    flow_id: i64,
    saved_version: Signal<i64>,
    mut deploying: Signal<bool>,
    mut seen_deployed: Signal<Option<Option<i64>>>,
    mut reload_meta: Signal<u32>,
    mut propose_deploy: Signal<Option<i64>>,
    on_changed: Callback<()>,
) -> impl FnMut(MouseEvent) + Copy {
    move |_: MouseEvent| {
        if deploying() {
            return;
        }
        deploying.set(true);
        // Deploy exactly the version shown in the editor: without it the
        // server picks the latest saved version, possibly one saved by
        // another user that this editor never displayed.
        let version = Some(saved_version()).filter(|v| *v > 0);
        spawn(async move {
            match api::flows::deploy(flow_id, version).await {
                Ok(deployed) => {
                    // Our own deploy: never reported as a remote change.
                    seen_deployed.set(Some(deployed.flow.deployed_version_number));
                    toasts::success("toast-flow-deployed");
                    // Stale notify references (D50): dedicated info toast —
                    // the deploy goes through, but missing or disabled
                    // channels/templates will send nothing (degraded node, warn).
                    let stale = deployed.stale_notify_nodes.len();
                    if stale > 0 {
                        toasts::info(t!("flows-deploy-stale-notify", count: stale).to_string());
                    }
                    // Tabs of other flows isolated at projection (unreadable
                    // graph): the deploy goes through, but those flows no longer
                    // run — to rebuild or delete.
                    let skipped = deployed.skipped_tabs.len();
                    if skipped > 0 {
                        toasts::info(t!("flows-deploy-skipped-tabs", count: skipped).to_string());
                    }
                    on_changed.call(());
                    reload_meta.with_mut(|r| *r += 1);
                    propose_deploy.set(None);
                }
                // Pre-flight rejected (400 engine_load): the **real** engine
                // message is shown — never again a bare "HTTP 400".
                // `ToastMessage::Api` resolves violations + code at render.
                Err(err) => toasts::error(err),
            }
            deploying.set(false);
        });
    }
}

/// Runtime resource, re-fetched every 5 s: `(reload_runtime, runtime)`.
pub(super) fn use_runtime_poll(
    flow_id: i64,
) -> (Signal<u32>, Resource<Result<FlowRuntimeStatus, ApiError>>) {
    // --- Runtime chip (5 s poll, devices.rs pattern) ---
    let mut reload_runtime = use_signal(|| 0u32);
    let mut runtime_polling = use_signal(|| false);
    let runtime = use_resource(move || async move {
        let _ = reload_runtime();
        api::flows::runtime(flow_id).await
    });
    if !runtime_polling() {
        runtime_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            runtime_polling.set(false);
            reload_runtime.with_mut(|r| *r += 1);
        });
    }
    (reload_runtime, runtime)
}

/// Stop / Start / Restart handler.
pub(super) fn runtime_action(
    flow_id: i64,
    mut runtime_busy: Signal<bool>,
    mut reload_meta: Signal<u32>,
    mut reload_runtime: Signal<u32>,
    on_changed: Callback<()>,
) -> impl FnMut(RuntimeAction) + Copy {
    move |action: RuntimeAction| {
        if runtime_busy() {
            return;
        }
        runtime_busy.set(true);
        spawn(async move {
            let outcome = match action {
                RuntimeAction::Stop => api::flows::stop(flow_id).await.map(|_| ()),
                RuntimeAction::Start => api::flows::start(flow_id).await.map(|_| ()),
                RuntimeAction::Restart => api::flows::restart(flow_id).await.map(|_| ()),
            };
            match outcome {
                Ok(()) => {
                    toasts::success(match action {
                        RuntimeAction::Stop => "toast-flow-stopped",
                        RuntimeAction::Start => "toast-flow-started",
                        RuntimeAction::Restart => "toast-flow-restarted",
                    });
                    on_changed.call(());
                    reload_meta.with_mut(|r| *r += 1);
                    // The chip reflects the new health on the next poll.
                    reload_runtime.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err),
            }
            runtime_busy.set(false);
        });
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn use_remote_watch(
    runtime: Resource<Result<FlowRuntimeStatus, ApiError>>,
    detail: Resource<Result<Flow, ApiError>>,
    saved_version: Signal<i64>,
    mut remote_version: Signal<Option<i64>>,
    mut seen_deployed: Signal<Option<Option<i64>>>,
    deploying: Signal<bool>,
    runtime_busy: Signal<bool>,
    mut reload_meta: Signal<u32>,
) {
    // Changes made by another user, observed through the runtime poll: a
    // newer saved version (banner offering a reload) and a redeploy or a
    // status change (detail refresh so the runtime controls stay right).
    use_effect(move || {
        let value = runtime.value();
        let value = value.read();
        let Some(Ok(status)) = &*value else {
            return;
        };
        let mine = *saved_version.peek();
        if let Some(latest) = status.latest_version_number {
            if mine > 0 && latest > mine && *remote_version.peek() != Some(latest) {
                remote_version.set(Some(latest));
            }
        }
        let mut refresh = false;
        let deployed = status.deployed_version_number;
        let previous_seen = *seen_deployed.peek();
        match previous_seen {
            None => seen_deployed.set(Some(deployed)),
            Some(previous) if previous != deployed => {
                seen_deployed.set(Some(deployed));
                if deployed.is_some() && !*deploying.peek() && !*runtime_busy.peek() {
                    toasts::info("toast-flow-deployed-remote");
                }
                refresh = true;
            }
            _ => {}
        }
        let detail_status = match &*detail.peek() {
            Some(Ok(flow)) => Some(flow.status),
            _ => None,
        };
        if status.flow_status.is_some()
            && detail_status.is_some()
            && status.flow_status != detail_status
        {
            refresh = true;
        }
        if refresh {
            reload_meta.with_mut(|r| *r += 1);
        }
    });
}

/// Debug tools flag + Display/status badges folding; returns `debug_tools`.
pub(super) fn use_display_values(
    flow_id: i64,
    runtime: Resource<Result<FlowRuntimeStatus, ApiError>>,
    mut display_values: Signal<std::collections::HashMap<String, debug::DisplayBadge>>,
    mut expanded_display: Signal<Option<(String, String)>>,
) -> Signal<bool> {
    // Debug tools enabled? Carried by the runtime chip (no dedicated
    // endpoint) — run mode: buttons not rendered, no poll. Signal (not a
    // derived boolean): the feed resource must **track** it.
    let mut debug_tools = use_signal(|| false);
    use_effect(move || {
        let active = match &*runtime.value().read() {
            Some(Ok(status)) => status.debug_tools,
            _ => false,
        };
        if debug_tools() != active {
            debug_tools.set(active);
        }
    });

    // --- Debug feed (Display badges) — 5 s poll while debug_tools ---
    let mut reload_debug = use_signal(|| 0u32);
    let mut debug_polling = use_signal(|| false);
    let debug_feed = use_resource(move || async move {
        let _ = reload_debug();
        if !debug_tools() {
            return None;
        }
        api::flows::debug(flow_id).await.ok()
    });
    if debug_tools() && !debug_polling() {
        debug_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            debug_polling.set(false);
            reload_debug.with_mut(|r| *r += 1);
        });
    }
    // --- Camera/vision node statuses (D103) — polled 5 s in every mode
    // (health, not debug output: no debug_tools gate) ---
    let mut reload_status = use_signal(|| 0u32);
    let mut status_polling = use_signal(|| false);
    let node_status = use_resource(move || async move {
        let _ = reload_status();
        api::flows::node_status(flow_id).await.ok()
    });
    if !status_polling() {
        status_polling.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_secs(5)).await;
            status_polling.set(false);
            reload_status.with_mut(|r| *r += 1);
        });
    }
    // Folding: last value per Display node, purged if the engine is
    // stopped (the values of a stopped engine are no longer true) —
    // and the open pretty modal closes with them.
    use_effect(move || {
        let running = match &*runtime.value().read() {
            Some(Ok(status)) => status.running,
            // Unknown status: no purge, out of caution.
            _ => true,
        };
        let mut fresh: std::collections::HashMap<String, debug::DisplayBadge> =
            std::collections::HashMap::new();
        if running {
            if let Some(Some(feed)) = &*debug_feed.value().read() {
                for entry in feed.entries.iter().filter(|e| e.source == "pnex-display") {
                    fresh.insert(
                        entry.node_id.clone(),
                        debug::DisplayBadge {
                            label: debug::display_value_label(&entry.msg),
                            pretty: debug::display_value_pretty(&entry.msg),
                            status: None,
                        },
                    );
                }
            }
            // Camera/vision node statuses (D103): latest one per node;
            // label and detail are localized at render time. Same poll: the
            // last message of every debug/display node, in every mode (the
            // side panel history alone stays behind debug_tools).
            if let Some(Some(feed)) = &*node_status.value().read() {
                for entry in &feed.entries {
                    if matches!(entry.source.as_str(), "debug" | "pnex-display") {
                        fresh.insert(
                            entry.node_id.clone(),
                            debug::DisplayBadge {
                                label: debug::debug_value_label(&entry.msg),
                                pretty: debug::display_value_pretty(&entry.msg)
                                    .or_else(|| Some(debug::display_value_compact(&entry.msg))),
                                status: None,
                            },
                        );
                        continue;
                    }
                    if let Ok(st) =
                        serde_json::from_value::<pnex_core::vision::NodeStatus>(entry.msg.clone())
                    {
                        fresh.insert(
                            entry.node_id.clone(),
                            debug::DisplayBadge {
                                label: String::new(),
                                pretty: None,
                                status: Some(st),
                            },
                        );
                    }
                }
            }
        }
        display_values.set(fresh);
        if !running && expanded_display.read().is_some() {
            expanded_display.set(None);
        }
    });
    debug_tools
}

// ─── Unified status (dot + label) — merge of the DB status and the
// runtime health (the old "Engine active · v12" chip). Priority:
// engine error > stopped > stale ("Deployed · to redeploy") > deployed
// > not deployed > detail not loaded. The tooltip keeps pid · vN + the
// last engine error, verbatim.
pub(super) fn editor_status(
    detail: Resource<Result<Flow, ApiError>>,
    runtime: Resource<Result<FlowRuntimeStatus, ApiError>>,
    saved_version: Signal<i64>,
) -> EditorStatus {
    let detail_value = detail.value();
    let runtime_value = runtime.value();
    let detail_ref = detail_value.read();
    let runtime_ref = runtime_value.read();
    match (&*detail_ref, &*runtime_ref) {
        (Some(Ok(_flow)), Some(Ok(status))) => {
            let engine_error = status.engine_status == Some(pnex_core::FlowEngineStatus::Error);
            let engine_stopped = status.engine_status == Some(pnex_core::FlowEngineStatus::Stopped);
            let deployed_version = status.deployed_version_number;
            let outdated = status.running
                && !engine_error
                && !engine_stopped
                && deployed_version.is_some()
                && deployed_version != Some(saved_version());
            let tooltip = {
                let base = match (status.pid, deployed_version) {
                    (Some(pid), Some(version)) => format!("pid {pid} · v{version}"),
                    (Some(pid), None) => format!("pid {pid}"),
                    _ => String::new(),
                };
                match &status.last_error {
                    Some(err) if !err.is_empty() => format!("{base} — {err}"),
                    _ => base,
                }
            };
            if engine_error {
                EditorStatus::new(StatusTone::Red, t!("flows-status-error")).with_tooltip(tooltip)
            } else if engine_stopped || !status.running {
                EditorStatus::new(StatusTone::Slate, t!("flows-status-stopped"))
                    .with_tooltip(tooltip)
            } else if outdated {
                EditorStatus::new(
                    StatusTone::Amber,
                    format!(
                        "{} · {}",
                        t!("flows-status-deployed"),
                        t!("flows-runtime-outdated")
                    ),
                )
                .with_tooltip(tooltip)
            } else if deployed_version.is_some() {
                EditorStatus::new(StatusTone::Green, t!("flows-status-deployed"))
                    .with_tooltip(tooltip)
            } else {
                EditorStatus::new(StatusTone::Slate, t!("flows-status-draft")).with_tooltip(tooltip)
            }
        }
        (Some(Ok(flow)), _) => match flow.status.as_str() {
            "deployed" => EditorStatus::new(StatusTone::Green, t!("flows-status-deployed")),
            "stopped" => EditorStatus::new(StatusTone::Slate, t!("flows-status-stopped")),
            "error" => EditorStatus::new(StatusTone::Red, t!("flows-status-error")),
            _ => EditorStatus::new(StatusTone::Slate, t!("flows-status-draft")),
        },
        _ => EditorStatus::new(StatusTone::Slate, t!("common-loading")),
    }
}
