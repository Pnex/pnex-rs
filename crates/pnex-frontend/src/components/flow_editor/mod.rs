//! Drag & drop flow editor (Phase 5 of the ETL work, D18).
//!
//! The editor only talks to the Loco API — never to the runtime (PRD guard,
//! docs/architecture/flow-engine.md). All graph mutations go
//! through the pure reducers of `state.rs`, the geometry is tested in
//! `geometry.rs`, rendering/gestures in `canvas.rs`, node config
//! in `inspector.rs`, history in `versions.rs`; the editor's loading,
//! save/409, staleness and deploy/runtime concerns live in `load.rs`,
//! `save.rs`, `staleness.rs` and `runtime.rs`.

pub(crate) mod canvas;
pub(crate) mod debug;
pub(crate) mod function_picker;
pub(crate) mod geometry;
pub(crate) mod inspector;
mod load;
mod runtime;
mod save;
mod staleness;
pub(crate) mod state;
pub(crate) mod versions;
pub(crate) mod wire_types;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FlowGraph, FlowVersionDetail, FlowViolation};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::editor_shell::{
    Chip, EditorShell, InspectorPanel, PalettePopover, StatusTone,
};
use crate::components::icons;
use crate::components::modal::Modal;
pub(crate) mod cx;

use crate::components::gerbe::Gerbe;
use crate::components::ui::{BTN_DANGER_SOLID, BTN_PRIMARY_SM, BTN_SECONDARY, BTN_SECONDARY_SM};
use cx::{EditorCx, Interaction, RuntimeAction};

/// Editor mounted by the `/flows` page — orchestration: loading, save,
/// 409 conflict, deploy, runtime chip, versions drawer.
#[component]
pub fn FlowEditor(
    flow_id: i64,
    can_write: bool,
    on_back: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload_meta = use_signal(|| 0u32);
    let detail = use_resource(move || async move {
        let _ = reload_meta();
        api::flows::detail(flow_id).await
    });

    // --- Editor state (the signals live in the `cx` bundle) ---
    let mut graph = use_signal(FlowGraph::default);
    let mut saved_graph = use_signal(FlowGraph::default);
    let saved_version = use_signal(|| 0i64);
    let mut selected_node = use_signal(|| None::<String>);
    let interaction = use_signal(|| Interaction::Idle);
    let pan = use_signal(|| (0.0_f64, 0.0_f64));
    let zoom = use_signal(|| 1.0_f64);
    let mut violations = use_signal(Vec::<FlowViolation>::new);
    let stale = use_signal(Vec::<FlowViolation>::new);
    let mut loaded_from = use_signal(|| None::<i64>);
    let display_values = use_signal(std::collections::HashMap::<String, debug::DisplayBadge>::new);
    let mut expanded_display = use_signal(|| None::<(String, String)>);

    load::use_subtitle_caches();

    load::use_initial_load(detail, graph, saved_graph, saved_version, violations);

    let mut cx = EditorCx {
        graph,
        saved_graph,
        saved_version,
        selected_node,
        interaction,
        pan,
        zoom,
        violations,
        stale,
        display_values,
        expanded_display,
    };

    // Derived dirty — never a dedicated signal to keep up to date.
    let dirty = graph() != saved_graph();

    staleness::use_pin_staleness(graph, stale);

    let (saving, mut conflict, mut propose_deploy, save) =
        save::use_save(flow_id, cx, loaded_from, reload_meta, on_changed);

    let mut conflict_reload = save::conflict_reload(flow_id, cx, conflict);
    let conflict_overwrite = save::conflict_overwrite(flow_id, cx, conflict, on_changed);

    // --- Deploy ---
    let deploying = use_signal(|| false);
    // Multi-user awareness (fed by the 5 s runtime poll): last deployed
    // version observed (`None` = not observed yet) and the newer version
    // saved by someone else, if any.
    let seen_deployed = use_signal(|| None::<Option<i64>>);
    let mut remote_version = use_signal(|| None::<i64>);
    let mut deploy = runtime::deploy_action(
        flow_id,
        saved_version,
        deploying,
        seen_deployed,
        reload_meta,
        propose_deploy,
        on_changed,
    );

    // --- Runtime controls (Stop / Start / Restart) ---
    // A single busy signal: stop/start/restart all touch the same
    // runtime, no concurrent action.
    let runtime_busy = use_signal(|| false);
    // --- Versions drawer ---
    let mut versions_open = use_signal(|| false);
    // Debug drawer (same dev/debug guard as the button).
    let mut debug_open = use_signal(|| false);
    // Wire to cut (from, port, to) — confirmation on demand.
    let mut pending_wire = use_signal(|| None::<(String, usize, String, String)>);

    let (reload_runtime, runtime) = runtime::use_runtime_poll(flow_id);
    let mut runtime_action = runtime::runtime_action(
        flow_id,
        runtime_busy,
        reload_meta,
        reload_runtime,
        on_changed,
    );
    runtime::use_remote_watch(
        runtime,
        detail,
        saved_version,
        remote_version,
        seen_deployed,
        deploying,
        runtime_busy,
        reload_meta,
    );

    let debug_tools =
        runtime::use_display_values(flow_id, runtime, display_values, expanded_display);

    // DB status gating the runtime controls (Stop/Restart
    // on `deployed`, Start on `stopped`).
    let flow_status = match &*detail.value().read() {
        Some(Ok(flow)) => flow.status.as_str(),
        _ => "",
    };

    let editor_status = runtime::editor_status(detail, runtime, saved_version);

    // Header name + ref (None until the detail is loaded).
    let (flow_name, flow_subtitle) = match &*detail.value().read() {
        Some(Ok(flow)) => (flow.name.clone(), Some(format!("#{}", flow.id))),
        _ => (String::new(), None),
    };

    // Inspector slot: mounted only if a node is selected
    // (principle 4 — the panel does not exist in the empty state). Title = node
    // name else kind label; remount by `key` so the fields
    // start again from the node config.
    let inspector_slot: Option<Element> = cx.selected_node.cloned().map(|node_id| {
        let graph_read = cx.graph.read();
        let node = graph_read.nodes.iter().find(|n| n.id == node_id);
        let kind = node.map(|n| canvas::kind_of(&n.kind));
        let name = node.and_then(|n| n.name.clone()).filter(|s| !s.is_empty());
        let label = kind.map(|k| canvas::kind_labels(k).0);
        let icon = kind.map(|k| canvas::kind_icon(k).0);
        drop(graph_read);
        let title = match name {
            Some(nom) => nom,
            None => label.unwrap_or_else(|| node_id.clone()),
        };
        rsx! {
            InspectorPanel {
                key: "{node_id}",
                icon,
                title,
                subtitle: Some(format!("#{}", node_id)),
                on_close: move |_| cx.selected_node.set(None),
                body: rsx! {
                    inspector::InspectorBody { cx, can_write, flow_id }
                },
            }
        }
    });

    // Reveal the selected node when the inspector would cover its output
    // ports (node near the right edge): pan once the gesture ends, on a new
    // selection or at the end of a drag — never while the user pans.
    let mut revealed_for = use_signal(|| None::<String>);
    let mut was_dragging = use_signal(|| false);
    use_effect(move || {
        let selected = cx.selected_node.cloned();
        let idle = matches!(*cx.interaction.read(), Interaction::Idle);
        let dragging = matches!(*cx.interaction.read(), Interaction::Dragging { .. });
        if dragging {
            was_dragging.set(true);
            return;
        }
        if !idle {
            return;
        }
        let drag_ended = *was_dragging.peek();
        was_dragging.set(false);
        if selected.is_none() || (!drag_ended && *revealed_for.peek() == selected) {
            revealed_for.set(selected);
            return;
        }
        revealed_for.set(selected.clone());
        let Some(rect) = geometry::canvas_rect() else {
            return;
        };
        let node_x = cx
            .graph
            .peek()
            .nodes
            .iter()
            .find(|n| Some(&n.id) == selected.as_ref())
            .and_then(|n| n.position)
            .map(|p| p.x);
        let Some(node_x) = node_x else { return };
        let (pan_x, pan_y) = *cx.pan.peek();
        let zoom = *cx.zoom.peek();
        if let Some(next_x) = geometry::pan_to_reveal_node(node_x, pan_x, zoom, rect.2) {
            cx.pan.set((next_x, pan_y));
        }
    });

    // Banner: validation violations + pin/device staleness (Phase 6).
    let banner: Vec<FlowViolation> = violations
        .cloned()
        .into_iter()
        .chain(stale.cloned())
        .collect();

    // ─── Pretty view of a Display probe (click on the live badge) ───
    // Title resolved here (no `let` in rsx): node name, else canvas
    // id (badge rule).
    let display_modal = expanded_display().map(|(node_id_open, pretty)| {
        let name = cx
            .graph
            .read()
            .nodes
            .iter()
            .find(|n| n.id == node_id_open)
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| node_id_open.clone());
        (name, pretty)
    });

    rsx! {
        EditorShell {
            on_back,
            title: flow_name,
            subtitle: flow_subtitle,
            on_rename: if can_write { Some(Callback::new(move |name: String| save.call(Some(name)))) } else { None },
            status: editor_status,
            version: if saved_version() > 0 { Some(saved_version()) } else { None },
            extra_chips: rsx! {
                if dirty {
                    Chip { tone: StatusTone::Amber, label: t!("flows-dirty-unsaved").to_string() }
                }
                if let Some(version) = loaded_from() {
                    Chip {
                        tone: StatusTone::Purple,
                        label: format!("v{version}"),
                        on_remove: conflict_reload,
                    }
                }
            },
            actions: rsx! {
                if can_write {
                    // One filled action at a time: Save while edited, Deploy once saved.
                    button {
                        class: if dirty { BTN_PRIMARY_SM } else { BTN_SECONDARY_SM },
                        disabled: saving() || !dirty,
                        onclick: move |_| save.call(None),
                        {t!("common-save")}
                    }
                    button {
                        class: if dirty { BTN_SECONDARY_SM } else { BTN_PRIMARY_SM },
                        disabled: deploying() || runtime_busy() || dirty,
                        title: if dirty { t!("flows-deploy-need-save") } else { t!("flows-deploy") },
                        onclick: deploy,
                        if deploying() {
                            Gerbe { size: 16, class: "mr-1.5", inherit: true }
                            {t!("flows-deploying")}
                        } else {
                            icons::Zap { class: "h-4 w-4 inline mr-1" }
                            {t!("flows-deploy")}
                        }
                    }
                    // Runtime controls: Stop/Restart on a deployed flow,
                    // Start on a stopped flow — gated on the DB status
                    // (process health is the chip's business).
                    if flow_status == "deployed" {
                        button {
                            class: "px-3 py-1.5 text-sm text-rose-700 bg-rose-50 border border-rose-300 rounded-lg hover:bg-rose-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-stop-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Stop),
                            {t!("flows-stop")}
                        }
                        button {
                            class: "px-3 py-1.5 text-sm text-slate-700 bg-slate-50 border border-slate-300 rounded-lg hover:bg-slate-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-restart-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Restart),
                            {t!("flows-restart")}
                        }
                    } else if flow_status == "stopped" {
                        button {
                            class: "px-3 py-1.5 text-sm text-emerald-700 bg-emerald-50 border border-emerald-300 rounded-lg hover:bg-emerald-100 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: runtime_busy() || deploying(),
                            title: t!("flows-start-title"),
                            onclick: move |_| runtime_action(RuntimeAction::Start),
                            {t!("flows-start")}
                        }
                    }
                }
                // The historical runtime chip ("Engine active · v12") is
                // absorbed by the shell's unified status (dot + label
                // + pid · vN tooltip — error on hover).
                if debug_tools() {
                    button {
                        class: if debug_open() {
                            "px-3 py-1.5 text-sm text-cyan-700 bg-cyan-50 border border-cyan-400 rounded-lg transition-colors"
                        } else {
                            "px-3 py-1.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors"
                        },
                        title: t!("flows-debug-panel"),
                        aria_label: t!("flows-debug-panel"),
                        onclick: move |_| debug_open.set(!debug_open()),
                        icons::Bug { class: "h-4 w-4 inline sm:mr-1" }
                        span { class: "hidden sm:inline", {t!("flows-debug-panel")} }
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    title: t!("flows-versions"),
                    aria_label: t!("flows-versions"),
                    onclick: move |_| versions_open.set(true),
                    icons::History { class: "h-4 w-4 sm:mr-1" }
                    span { class: "hidden sm:inline", {t!("flows-versions")} }
                }
                // D42 labels of the flow (same editor as every resource).
                crate::components::labels_editor::LabelsButton {
                    kind: pnex_core::resources::KIND_FLOW.to_string(),
                    id: flow_id.to_string(),
                    can_write,
                }
            },
            banner: rsx! {
                // ─── Violations banner (local, server) + staleness ───
                if !banner.is_empty() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg px-4 py-2 text-sm text-red-700",
                        span { class: "font-semibold mr-2", {t!("flows-violations-banner-title")} }
                        ul { class: "list-disc list-inside",
                            for violation in banner.clone() {
                                li { key: "{violation.code}-{violation.node_id:?}-{violation.message}",
                                    {
                                        match &violation.node_id {
                                            Some(node_id) => format!("#{node_id} : {}", violation.message),
                                            None => violation.message.clone(),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ─── Newer version saved by another user ───
                {
                    match remote_version() {
                        Some(version) if version > saved_version() => rsx! {
                            div { class: "bg-amber-50 border border-amber-200 rounded-lg px-4 py-2 text-sm text-amber-800 flex items-center gap-3",
                                span { class: "flex-1",
                                    if dirty {
                                        {t!("flows-remote-version-dirty", version : version)}
                                    } else {
                                        {t!("flows-remote-version", version : version)}
                                    }
                                }
                                button {
                                    class: "px-3 py-1 text-xs bg-amber-600 text-white rounded-lg hover:bg-amber-700 transition-colors font-medium",
                                    onclick: move |_| {
                                        remote_version.set(None);
                                        propose_deploy.set(None);
                                        conflict_reload(());
                                    },
                                    {t!("flows-remote-version-reload")}
                                }
                            }
                        },
                        _ => rsx! {},
                    }
                }

                // ─── "Deploy the saved version?" box ───
                // Offered after saving an already deployed flow; hidden if
                // the user edits again (the offered version would no longer
                // be the canvas one) or after a successful deploy.
                {
                    match propose_deploy() {
                        Some(version) if !dirty => rsx! {
                            div { class: "bg-signal-50 border border-signal-200 rounded-lg px-4 py-2 text-sm text-signal-800 flex items-center gap-3",
                                span { class: "flex-1", {t!("flows-deploy-propose-text", version : version)} }
                                button {
                                    class: BTN_PRIMARY_SM,
                                    disabled: deploying(),
                                    onclick: move |evt| {
                                        propose_deploy.set(None);
                                        deploy(evt);
                                    },
                                    {t!("flows-deploy-propose-go", version : version)}
                                }
                                button {
                                    class: "px-3 py-1 text-xs text-signal-700 hover:bg-signal-100 rounded-lg transition-colors",
                                    onclick: move |_| propose_deploy.set(None),
                                    {t!("flows-deploy-propose-later")}
                                }
                            }
                        },
                        _ => rsx! {},
                    }
                }
            },
            canvas: rsx! {
                canvas::Canvas {
                    cx,
                    on_cut_wire: move |wire| {
                        pending_wire.set(Some(wire));
                    },
                }
            },
            palette: rsx! {
                PalettePopover {
                    add_title: t!("eshell-add-node").to_string(),
                    title: t!("flows-palette-title").to_string(),
                    search_placeholder: t!("eshell-search").to_string(),
                    items: canvas::palette_items(),
                    on_pick: move |key: String| {
                        if let Some(kind) = canvas::kind_from_key(&key) {
                            canvas::add_node_to_canvas(cx, kind);
                        }
                    },
                }
            },
            inspector: inspector_slot,
            empty_hint: if cx.graph.cloned().nodes.is_empty() { Some(t!("eshell-empty-hint").to_string()) } else { None },
        }

        // ─── Debug drawer (mounted only if open: the poll dies with it) ───
        if debug_open() {
            debug::DebugDrawer { flow_id, on_close: move |_| debug_open.set(false) }
        }

        // ─── Versions drawer ───
        if versions_open() {
            versions::VersionsDrawer {
                flow_id,
                can_write,
                on_close: move |_| versions_open.set(false),
                on_loaded: move |version: FlowVersionDetail| {
                    // Loading a version into the editor: if it is
                    // the latest, not dirty; otherwise save will create
                    // v(n+1) with this graph (restore by editing).
                    let mut fresh = version.graph;
                    geometry::ensure_positions(&mut fresh);
                    graph.set(fresh.clone());
                    if version.version_number == saved_version() {
                        saved_graph.set(fresh);
                        loaded_from.set(None);
                    } else {
                        saved_graph.set(FlowGraph::default());
                        loaded_from.set(Some(version.version_number));
                    }
                    selected_node.set(None);
                    violations.set(Vec::new());
                    versions_open.set(false);
                },
                on_deployed: move |_| {
                    reload_meta.with_mut(|r| *r += 1);
                    on_changed.call(());
                },
            }
        }

        // ─── Modals ───
        if let Some(description) = conflict() {
            Modal {
                title: t!("flows-conflict-title"),
                max_width: "max-w-md".to_string(),
                on_close: move |_| conflict.set(None),
                div { class: "space-y-4",
                    p { class: "text-sm text-gray-600", {description.clone()} }
                    p { class: "text-sm text-gray-600", {t!("flows-conflict-message")} }
                    div { class: "flex flex-col gap-2 pt-2",
                        button {
                            class: BTN_SECONDARY,
                            onclick: move |_| conflict_reload(()),
                            {t!("flows-conflict-reload")}
                        }
                        button {
                            class: BTN_DANGER_SOLID,
                            onclick: conflict_overwrite,
                            {t!("flows-conflict-overwrite")}
                        }
                    }
                }
            }
        }
        if let Some((from, port, to, pin)) = pending_wire() {
            ConfirmDialog {
                title: t!("flows-wire-remove-title"),
                message: t!("flows-wire-remove-message"),
                confirm_label: t!("flows-wire-remove"),
                on_confirm: move |_| {
                    cx.update_graph(|g| {
                        if pin.is_empty() {
                            // Plain source-side wire.
                            state::remove_target(g, &from, port, &to);
                        } else {
                            // Annotated pin row: the runtime wire only
                            // goes if the source feeds no other pin.
                            state::cut_input_wire(g, &from, port, &to, &pin);
                        }
                    });
                    pending_wire.set(None);
                },
                on_cancel: move |_| pending_wire.set(None),
            }
        }

        // ─── Pretty view of a Display probe (click on the live badge) ───
        if let Some((name, pretty)) = display_modal {
            Modal {
                title: t!("flows-display-value-title", name : name),
                max_width: "max-w-2xl".to_string(),
                on_close: move |_| expanded_display.set(None),
                pre { class: "text-xs font-mono whitespace-pre-wrap break-all bg-gray-50 border border-gray-200 rounded-lg p-4 text-gray-800 m-0",
                    {pretty}
                }
            }
        }
    }
}
