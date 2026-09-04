//! Éditeur de flows drag & drop (Phase 5 du chantier ETL, D18).
//!
//! L'éditeur ne parle qu'à l'API Loco — jamais au runtime (garde-fou PRD,
//! docs/architecture/flow-engine.md). Toutes les mutations du graphe passent
//! par les réducteurs purs de `state.rs`, la géométrie est testée dans
//! `geometry.rs`, le rendu/gestes dans `canvas.rs`, la config des nœuds
//! dans `inspector.rs`, l'historique dans `versions.rs`.

pub(crate) mod canvas;
pub(crate) mod geometry;
pub(crate) mod inspector;
pub(crate) mod state;
pub(crate) mod versions;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{
    validate_graph, FlowGraph, FlowVersionDetail, FlowViolation, UpdateFlow,
};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::state::session;
use crate::state::toasts;

/// Geste en cours (machine à états plate — un seul variant actif).
#[derive(Clone, PartialEq)]
pub(crate) enum Interaction {
    /// Aucun geste.
    Idle,
    /// Pan du fond : delta client depuis le début du geste.
    Panning { start_client: (f64, f64), start_pan: (f64, f64) },
    /// Drag d'un nœud : `grab` = décalage curseur↔origine du nœud, mesuré au
    /// début du geste.
    Dragging { id: String, grab: (f64, f64), rect: (f64, f64, f64, f64) },
    /// Câblage depuis un port de sortie : curseur en coords graphe + cible
    /// survolée (hit-test bbox à chaque move).
    Wiring { from_id: String, port: usize, cursor: (f64, f64), hover_target: Option<String> },
}

/// Paquet de signaux `Copy` partagés entre l'éditeur et ses sous-composants.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct EditorCx {
    /// Graphe en cours d'édition (ce que le Save enverra).
    pub(crate) graph: Signal<FlowGraph>,
    /// Baseline du dirty : graphe de la dernière version enregistrée.
    pub(crate) saved_graph: Signal<FlowGraph>,
    /// Numéro de la dernière version connue côté serveur.
    pub(crate) saved_version: Signal<i64>,
    /// Nœud sélectionné (inspecteur + touche Delete).
    pub(crate) selected_node: Signal<Option<String>>,
    /// Geste en cours.
    pub(crate) interaction: Signal<Interaction>,
    pub(crate) pan: Signal<(f64, f64)>,
    pub(crate) zoom: Signal<f64>,
    /// Violations courantes (locales ou reçues en 400 du serveur).
    pub(crate) violations: Signal<Vec<FlowViolation>>,
    /// Violations de **staleness** (client-only, jamais persistées) : pin
    /// passé en sortie, pin disparu du pinout, device inconnu — le graphe
    /// est structurellement valide mais sa lecture ne remontera rien.
    pub(crate) stale: Signal<Vec<FlowViolation>>,
}

impl EditorCx {
    /// Mutation du graphe via un réducteur pur de `state.rs` — `mut self`
    /// car `Signal::with_mut` exige `&mut` (et `EditorCx` est `Copy`).
    pub(crate) fn update_graph(mut self, f: impl FnOnce(&mut FlowGraph)) {
        self.graph.with_mut(f);
    }

    /// Violations localisées sur un nœud donné (validation + staleness).
    pub(crate) fn violations_of(&self, node_id: &str) -> Vec<FlowViolation> {
        self.violations
            .read()
            .iter()
            .chain(self.stale.read().iter())
            .filter(|v| v.node_id.as_deref() == Some(node_id))
            .cloned()
            .collect()
    }
}

/// Éditeur monté par la page `/flows` — orchestration : chargement, save,
/// conflit 409, deploy, chip runtime, drawer versions.
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

    // --- État éditeur (les signaux vivent dans le paquet `cx`) ---
    let mut graph = use_signal(FlowGraph::default);
    let mut saved_graph = use_signal(FlowGraph::default);
    let mut saved_version = use_signal(|| 0i64);
    let mut selected_node = use_signal(|| None::<String>);
    let interaction = use_signal(|| Interaction::Idle);
    let pan = use_signal(|| (0.0_f64, 0.0_f64));
    let zoom = use_signal(|| 1.0_f64);
    let mut violations = use_signal(Vec::<FlowViolation>::new);
    let mut stale = use_signal(Vec::<FlowViolation>::new);
    let mut loaded_from = use_signal(|| None::<i64>);

    // Garde du chargement initial (jamais de `.set()` pendant le render).
    let mut loaded = use_signal(|| false);
    use_effect(move || {
        let resource = detail.value();
        let value = resource.read();
        let Some(Ok(flow)) = &*value else {
            return;
        };
        if loaded() {
            return;
        }
        let mut fresh = flow.graph.clone();
        geometry::ensure_positions(&mut fresh);
        graph.set(fresh.clone());
        saved_graph.set(fresh);
        saved_version.set(flow.latest_version_number);
        loaded.set(true);
    });

    let cx = EditorCx {
        graph,
        saved_graph,
        saved_version,
        selected_node,
        interaction,
        pan,
        zoom,
        violations,
        stale,
    };

    // Dirty dérivé — jamais de signal dédié à tenir à jour.
    let dirty = graph() != saved_graph();

    // ─── Staleness pin/device (Phase 6) ───
    // Un graphe structurellement valide peut ne plus rien remonter : le pin
    // a basculé in↔out (set_mode), le pin a disparu du pinout, le device est
    // inconnu. Violations client-only (jamais persistées) → nœud + câble en
    // rouge. Re-fetch uniquement au changement de CONFIG de lecture (pas au
    // drag — la signature exclut les positions).
    let mut stale_signature = use_signal(String::new);
    use_effect(move || {
        let g = graph.cloned(); // lecture suivie : re-déclenche au changement
        let mut reads: Vec<(String, String, String)> = Vec::new();
        let mut signature = String::new();
        for n in &g.nodes {
            if let pnex_core::FlowNodeKind::Device { config } = &n.kind {
                for r in &config.reads {
                    reads.push((n.id.clone(), r.device_id.clone(), r.pin.clone()));
                    signature.push_str(&format!("{}:{}/{};", n.id, r.device_id, r.pin));
                }
            }
        }
        if signature == *stale_signature.cloned() {
            return; // config inchangée (drag/pan) : rien à re-scruter
        }
        stale_signature.set(signature);
        spawn(async move {
            if reads.is_empty() {
                stale.set(Vec::new());
                return;
            }
            let devices = api::devices::list(&api::devices::DeviceFilters {
                active: Some(true),
                limit: Some(200),
                ..Default::default()
            })
            .await
            .map(|page| page.results)
            .unwrap_or_default();
            let mut pins_by_slug: std::collections::HashMap<String, Vec<api::pins::PinInfo>> =
                std::collections::HashMap::new();
            for slug in reads.iter().map(|(_, d, _)| d.clone()).collect::<std::collections::HashSet<_>>() {
                let Some(pk) = devices.iter().find(|d| d.device_id == slug).map(|d| d.id) else {
                    continue;
                };
                if let Ok(resp) = api::pins::pins(pk).await {
                    pins_by_slug.insert(slug, resp.pins);
                }
            }
            let mut found: Vec<FlowViolation> = Vec::new();
            for (node_id, device_slug, pin_label) in reads {
                match pins_by_slug.get(&device_slug) {
                    None => found.push(FlowViolation::new(
                        Some(&node_id),
                        "pin_unavailable",
                        format!("device « {device_slug} » introuvable (supprimé ou inactif)"),
                    )),
                    Some(pins) => match pins.iter().find(|p| p.label == pin_label) {
                        None => found.push(FlowViolation::new(
                            Some(&node_id),
                            "pin_unavailable",
                            format!("pin « {pin_label} » absent du device « {device_slug} »"),
                        )),
                        Some(pin) if pin.mode == "digital_out" => found.push(FlowViolation::new(
                            Some(&node_id),
                            "pin_unavailable",
                            format!(
                                "pin « {pin_label} » est en sortie (digital_out) — la lecture ne remontera aucune donnée"
                            ),
                        )),
                        _ => {}
                    },
                }
            }
            stale.set(found);
        });
    });

    // --- Save (validate → PATCH) ---
    let mut saving = use_signal(|| false);
    let mut conflict = use_signal(|| None::<String>);
    let save = move |_| {
        if saving() {
            return;
        }
        let local = validate_graph(&graph.peek().clone());
        if !local.is_empty() {
            violations.set(local);
            return;
        }
        saving.set(true);
        let params = UpdateFlow {
            expected_version_number: saved_version(),
            graph: graph(),
            name: None,
            author: session::user().map(|user| user.username),
            note: None,
        };
        spawn(async move {
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    loaded_from.set(None);
                    toasts::success("toast-flow-saved");
                    on_changed.call(());
                }
                Err(err) => match api::flows::classify_save_error(&err) {
                    api::flows::SaveError::Conflict { description } => {
                        conflict.set(Some(description));
                    }
                    api::flows::SaveError::Invalid(invalid) => violations.set(invalid),
                    api::flows::SaveError::Other(message) => toasts::error(message),
                },
            }
            saving.set(false);
        });
    };

    // --- Résolution du conflit 409 ---
    let conflict_reload = move |_| {
        conflict.set(None);
        spawn(async move {
            match api::flows::detail(flow_id).await {
                Ok(flow) => {
                    let mut fresh = flow.graph.clone();
                    geometry::ensure_positions(&mut fresh);
                    graph.set(fresh.clone());
                    saved_graph.set(fresh);
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };
    let conflict_overwrite = move |_| {
        conflict.set(None);
        spawn(async move {
            let Ok(server) = api::flows::detail(flow_id).await else {
                toasts::error(t!("common-error").to_string());
                return;
            };
            let params = UpdateFlow {
                // Version fraîche du serveur : écrasement assumé.
                expected_version_number: server.latest_version_number,
                graph: graph.peek().clone(),
                name: None,
                author: session::user().map(|user| user.username),
                note: None,
            };
            match api::flows::update(flow_id, params).await {
                Ok(flow) => {
                    saved_graph.set(flow.graph.clone());
                    saved_version.set(flow.latest_version_number);
                    violations.set(Vec::new());
                    toasts::success("toast-flow-saved");
                    on_changed.call(());
                }
                Err(err) => toasts::error(err.message),
            }
        });
    };

    // --- Deploy ---
    let mut deploying = use_signal(|| false);
    let deploy = move |_| {
        if deploying() {
            return;
        }
        deploying.set(true);
        spawn(async move {
            match api::flows::deploy(flow_id, None).await {
                Ok(_) => {
                    toasts::success("toast-flow-deployed");
                    on_changed.call(());
                    reload_meta.with_mut(|r| *r += 1);
                }
                Err(err) => toasts::error(err.message),
            }
            deploying.set(false);
        });
    };

    // --- Drawer versions ---
    let mut versions_open = use_signal(|| false);
    // Câble à couper (from, port, to) — confirmation à la demande.
    let mut pending_wire = use_signal(|| None::<(String, usize, String)>);

    // --- Chip runtime (poll 5 s, pattern devices.rs) ---
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

    let (status_badge, status_label) = match &*detail.value().read() {
        Some(Ok(flow)) => {
            let (badge, label) = status_badge(&flow.status);
            (badge, label)
        }
        _ => ("bg-gray-100 text-gray-800", t!("common-loading")),
    };

    // Bandeau : violations de validation + staleness pin/device (Phase 6).
    let banner: Vec<FlowViolation> =
        violations.cloned().into_iter().chain(stale.cloned()).collect();

    rsx! {
        div { class: "flex flex-col gap-3",
            // ─── Toolbar ───
            div { class: "flex flex-wrap items-center gap-2 bg-white rounded-lg shadow-sm px-4 py-2.5",
                button {
                    class: "text-sm text-blue-600 hover:text-blue-700",
                    onclick: move |_| on_back.call(()),
                    { "← " }
                    {t!("flows-back-list")}
                }
                div { class: "h-5 w-px bg-gray-200" }
                {match &*detail.value().read() {
                    Some(Ok(flow)) => rsx! {
                        h2 { class: "text-base font-semibold text-gray-900", {flow.name.clone()} }
                        span { class: "text-xs text-gray-400", {format!("#{}", flow.id)} }
                    },
                    _ => rsx! {},
                }}
                span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {status_badge}",
                    {status_label}
                }
                if dirty {
                    span { class: "inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-amber-50 text-amber-700 border border-amber-200",
                        span { class: "h-1.5 w-1.5 rounded-full bg-amber-500" }
                        {t!("flows-dirty-unsaved")}
                    }
                }
                if let Some(version) = loaded_from() {
                    span { class: "inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-purple-50 text-purple-700 border border-purple-200",
                        {format!("v{version}")}
                        button {
                            class: "text-purple-500 hover:text-purple-700",
                            title: t!("flows-versions-back-to-latest"),
                            onclick: conflict_reload,
                            { "✕" }
                        }
                    }
                }
                div { class: "flex-1" }
                if can_write {
                    button {
                        class: "px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        disabled: saving() || !dirty,
                        onclick: save,
                        {t!("common-save")}
                    }
                    button {
                        class: if !dirty {
                            "px-3 py-1.5 text-sm bg-emerald-600 text-white rounded-lg hover:bg-emerald-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed"
                        } else {
                            "px-3 py-1.5 text-sm bg-emerald-600 text-white rounded-lg transition-colors font-medium opacity-40 cursor-not-allowed"
                        },
                        disabled: deploying() || dirty,
                        title: if dirty { t!("flows-deploy-need-save") } else { t!("flows-deploy") },
                        onclick: deploy,
                        icons::Zap { class: "h-4 w-4 inline mr-1" }
                        {t!("flows-deploy")}
                    }
                }
                // Chip runtime (superviseur backend, acquittement du moteur).
                {match &*runtime.value().read() {
                    Some(Ok(status)) => rsx! {
                        span {
                            class: if status.running {
                                "inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-green-50 text-green-700 border border-green-200"
                            } else {
                                "inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium bg-gray-50 text-gray-600 border border-gray-200"
                            },
                            title: match (status.pid, status.deployed_version_number) {
                                (Some(pid), Some(version)) => format!("pid {pid} · v{version}"),
                                (Some(pid), None) => format!("pid {pid}"),
                                _ => String::new(),
                            },
                            if status.running {
                                {t!("flows-runtime-running")}
                            } else {
                                {t!("flows-runtime-stopped")}
                            }
                        }
                    },
                    _ => rsx! {},
                }}
                button {
                    class: "px-3 py-1.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                    onclick: move |_| versions_open.set(true),
                    {t!("flows-versions")}
                }
            }

            // ─── Bandeau de violations (locales, serveur) + staleness ───
            if !banner.is_empty() {
                div { class: "bg-red-50 border border-red-200 rounded-lg px-4 py-2 text-sm text-red-700",
                    span { class: "font-semibold mr-2", {t!("flows-violations-banner-title")} }
                    ul { class: "list-disc list-inside",
                        for violation in banner.clone() {
                            li { key: "{violation.code}-{violation.node_id:?}",
                                {match &violation.node_id {
                                    Some(node_id) => format!("#{node_id} : {}", violation.message),
                                    None => violation.message.clone(),
                                }}
                            }
                        }
                    }
                }
            }

            // ─── Corps : palette · canevas · inspecteur ───
            div { class: "flex gap-3 h-[calc(100vh-16rem)] min-h-96",
                canvas::Palette { cx }
                canvas::Canvas {
                    cx,
                    on_cut_wire: move |wire| {
                        pending_wire.set(Some(wire));
                    },
                }
                inspector::Inspector {
                    // Remonté à chaque changement de sélection : les champs
                    // repartent de la config du nœud.
                    key: "{selected_node.cloned():?}",
                    cx,
                    can_write,
                }
            }

            // ─── Drawer versions ───
            if versions_open() {
                versions::VersionsDrawer {
                    flow_id,
                    can_write,
                    on_close: move |_| versions_open.set(false),
                    on_loaded: move |version: FlowVersionDetail| {
                        // Chargement d'une version dans l'éditeur : si c'est
                        // la dernière, pas de dirty ; sinon save créera
                        // v(n+1) avec ce graphe (restauration par édition).
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

            // ─── Modales ───
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
                                class: "px-4 py-2 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                                onclick: conflict_reload,
                                {t!("flows-conflict-reload")}
                            }
                            button {
                                class: "px-4 py-2 text-sm font-semibold text-white bg-red-600 rounded-lg hover:bg-red-700 transition-colors",
                                onclick: conflict_overwrite,
                                {t!("flows-conflict-overwrite")}
                            }
                        }
                    }
                }
            }
            if let Some((from, port, to)) = pending_wire() {
                ConfirmDialog {
                    title: t!("flows-wire-remove-title"),
                    message: t!("flows-wire-remove-message"),
                    confirm_label: t!("flows-wire-remove"),
                    on_confirm: move |_| {
                        cx.update_graph(|g| state::remove_target(g, &from, port, &to));
                        pending_wire.set(None);
                    },
                    on_cancel: move |_| pending_wire.set(None),
                }
            }
        }
    }
}

/// Badge de statut du flow (mêmes classes littérales que la liste).
fn status_badge(status: &str) -> (&'static str, String) {
    let label = match status {
        "deployed" => t!("flows-status-deployed"),
        "error" => t!("flows-status-error"),
        _ => t!("flows-status-draft"),
    };
    let badge = match status {
        "deployed" => "bg-green-100 text-green-800",
        "error" => "bg-red-100 text-red-800",
        _ => "bg-gray-100 text-gray-800",
    };
    (badge, label)
}
