//! Studio SCADA — éditeur **canvas libre** des dashboards (D40/D41,
//! école `flow_editor` : canvas sans lib, reducers purs, gestes
//! pointeur). Deux outils : **Sélection** (déplacer/resize/supprimer) et
//! **Trait** (accroches aux milieux des côtés, liaison purement
//! visuelle widget→widget qui suit les déplacements).
//!
//! D34 : en édition le polling est **suspendu** — les valeurs affichées
//! sont une **photo** prise à l'entrée (badge figé) ; le rendu du corps
//! de widget est le même qu'en live ([`crate::components::dashboard_widget`]).
//!
//! Save (D24) : `PATCH` = nouvelle version = live — la validation
//! `pnex_core::validate_layout` tourne **dans le navigateur** avant
//! l'appel ; 409 → modale recharger/écraser (école flow editor).

mod actions;
mod appearance;
mod canvas;
mod control_panel;
mod device_panel;
pub mod geometry;
mod home_panel;
pub mod inspector;
pub mod library;
mod mobile;
pub mod state;
mod symbol_options;
mod symbol_panel;
pub mod thermo_panel;
pub mod versions;

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardLayout, TelemetryPoint, VizDashboard, VizViolation, WireEndpoint};

use crate::api;
use crate::components::editor_shell::palette::PaletteIcon;
use crate::components::editor_shell::{
    Chip, EditorShell, EditorStatus, InspectorPanel, PalettePopover, StatusTone,
};
use crate::components::icons;
use state::History;

use actions::*;
use canvas::*;
use geometry::GRID;
use library::TemplateList;
use symbol_panel::SymbolPanel;
use versions::VersionsDrawer;

/// Outil courant.
#[derive(Clone, Copy, PartialEq)]
pub enum Tool {
    Select,
    Wire,
}

/// Sélection courante : widget ou trait.
#[derive(Clone, PartialEq)]
pub enum Selection {
    Widget(String),
    Wire(String),
}

/// Geste en cours (école `Interaction` du flow editor).
#[derive(Clone, PartialEq)]
pub enum Interaction {
    Idle,
    /// Pan du viewport (fond) — origine client + pan d'origine.
    Panning {
        start: (f64, f64),
        origin: (f64, f64),
    },
    /// Déplacement de widget — décalage point→position du widget.
    Dragging {
        id: String,
        grab: (f64, f64),
    },
    /// Resize par la poignée bas-droit.
    Resizing {
        id: String,
    },
}

/// Bundle de signaux partagés avec les sous-composants (école
/// `flow_editor::EditorCx`).
#[derive(Clone, Copy, PartialEq)]
pub struct EditorCx {
    pub dashboard_id: Signal<String>,
    /// Name shown in the shell title, updated by a successful rename.
    pub name: Signal<String>,
    pub layout: Signal<DashboardLayout>,
    pub saved_layout: Signal<DashboardLayout>,
    /// Version **courante côté serveur** (le pointeur live visé par le
    /// prochain save).
    pub saved_version: Signal<i64>,
    pub selected: Signal<Option<Selection>>,
    pub interaction: Signal<Interaction>,
    pub pan: Signal<(f64, f64)>,
    pub zoom: Signal<f64>,
    pub tool: Signal<Tool>,
    /// Première accroche armée en outil Trait.
    pub wire_draft: Signal<Option<WireEndpoint>>,
    pub history: Signal<History>,
    pub counter: Signal<u32>,
    pub violations: Signal<Vec<VizViolation>>,
    pub saving: Signal<bool>,
    pub conflict: Signal<bool>,
    /// Valeurs figées (photo au mount) — clé `metric|device_id`.
    pub live: Signal<HashMap<String, Option<Vec<TelemetryPoint>>>>,
    /// Modèle en cours de drag depuis la bibliothèque.
    pub drag_template: Signal<Option<pnex_core::VizWidget>>,
    /// Re-load de la palette (après save-as-template).
    pub palette_reload: Signal<u32>,
    /// Modale « enregistrer comme modèle » (id du widget source).
    pub save_as: Signal<Option<String>>,
    /// Symbol library panel shown (tools slot).
    pub symbols_open: Signal<bool>,
    /// Mobile: section receiving the palette additions (`None`: the last).
    pub section: Signal<Option<String>>,
    /// Mobile: card being dragged (HTML drag and drop).
    pub drag_card: Signal<Option<String>>,
    /// Guided "from a device" panel shown (tools slot).
    pub devices_open: Signal<bool>,
}

#[component]
pub fn DashboardEditor(
    detail: VizDashboard,
    can_write: bool,
    on_back: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let dashboard_id = use_signal(move || detail.id.clone());
    let name = use_signal(move || detail.name.clone());
    let initial_layout = detail.layout.clone();
    // Amorce AVANT que le closure de `layout` ne consomme `initial_layout`.
    let counter_seed = state::seed_counter(&initial_layout);
    let layout = use_signal(move || initial_layout.clone());
    let saved_layout = use_signal(move || detail.layout.clone());
    let saved_version = use_signal(move || detail.current_version_number);
    let selected: Signal<Option<Selection>> = use_signal(|| None);
    let interaction = use_signal(|| Interaction::Idle);
    let pan = use_signal(|| (0.0_f64, 0.0_f64));
    let zoom = use_signal(|| 1.0_f64);
    let tool = use_signal(|| Tool::Select);
    let wire_draft: Signal<Option<WireEndpoint>> = use_signal(|| None);
    let history = use_signal(History::default);
    // Compteur amorcé sur les ids persistés du layout chargé : repartir de
    // 0 ré-émettrait `w-0001` en collision ⇒ panic « keyed siblings » au
    // premier ajout de la session.
    let counter = use_signal(move || counter_seed);
    let violations: Signal<Vec<VizViolation>> = use_signal(Vec::new);
    let saving = use_signal(|| false);
    let conflict = use_signal(|| false);
    let mut live: Signal<HashMap<String, Option<Vec<TelemetryPoint>>>> = use_signal(HashMap::new);
    let drag_template: Signal<Option<pnex_core::VizWidget>> = use_signal(|| None);
    let palette_reload = use_signal(|| 0_u32);
    let save_as: Signal<Option<String>> = use_signal(|| None);
    let symbols_open = use_signal(|| false);
    let section: Signal<Option<String>> = use_signal(|| None);
    let drag_card: Signal<Option<String>> = use_signal(|| None);
    let devices_open = use_signal(|| false);
    let mut versions_open = use_signal(|| false);

    let mut cx = EditorCx {
        dashboard_id,
        name,
        layout,
        saved_layout,
        saved_version,
        selected,
        interaction,
        pan,
        zoom,
        tool,
        wire_draft,
        history,
        counter,
        violations,
        saving,
        conflict,
        live,
        drag_template,
        palette_reload,
        save_as,
        symbols_open,
        section,
        drag_card,
        devices_open,
    };
    let mobile_format = cx.layout.read().format == pnex_core::DashboardFormat::Mobile;

    // Control cards show their control (label, kind, "no effect" badge) in
    // the preview too; never operable from the editor.
    let mut preview_tick = use_signal(|| 0u32);
    // Each save may provision controls (D131): reload the definitions.
    let saved_version = cx.saved_version;
    use_effect(move || {
        let _ = saved_version.read();
        let next = *preview_tick.peek() + 1;
        preview_tick.set(next);
    });
    let via = format!("dashboard:{}", cx.dashboard_id.peek());
    crate::components::surface::use_surface_controls(
        move || crate::components::surface::control_ids(&layout.read()),
        preview_tick,
        via,
        false,
    );

    // ── Photo des valeurs à l'entrée (D34) : un seul series-batch, pas
    // de polling en édition. Sources lues **non trackées** : la photo ne
    // se reprend pas à chaque pixel de drag.
    use_effect(move || {
        let sources: Vec<pnex_core::SourceRef> = {
            let l = layout.read_unchecked();
            l.widgets
                .iter()
                .flat_map(|w| w.source.iter().cloned())
                .collect()
        };
        spawn(async move {
            let map = crate::components::dashboard_live::fetch_live_values(sources).await;
            live.set(map);
        });
    });

    // dirty dérivé au rendu (école flow editor).
    let dirty = layout() != saved_layout();
    let has_violations = !violations.is_empty();
    let save_disabled = !can_write || cx.saving.cloned() || !dirty;

    // ── Catalogue télémétrie hissé ici (ex-`Inspector`) : le remount de
    // l'inspecteur par sélection ne doit pas refetch /telemetry/catalog.
    let reload = cx.palette_reload;
    let catalog = use_resource(move || async move {
        let _ = reload();
        crate::state::org::current()?;
        api::telemetry::catalog().await.ok()
    });
    // Org shared memory keys (flow memory-write), same reload trigger.
    let memory_keys = use_resource(move || async move {
        let _ = reload();
        crate::state::org::current()?;
        api::memory::keys().await.ok()
    });
    let memory_catalog: std::collections::BTreeMap<String, Vec<String>> = memory_keys
        .read()
        .as_ref()
        .cloned()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .map(|k| (k.key, k.fields))
        .collect();
    let cat = catalog.read().as_ref().cloned().flatten();
    let metrics: std::collections::BTreeMap<String, Vec<String>> = cat
        .as_ref()
        .filter(|c| c.available)
        .map(|c| {
            let mut map: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            for s in &c.series {
                map.entry(s.metric.clone())
                    .or_default()
                    .push(s.device_id.clone());
            }
            map
        })
        .unwrap_or_default();
    // Catalogue **par source** (device ou device virtuel de flow) pour
    // l'inspecteur : l'appareil se choisit d'abord, la métrique ensuite
    // (filtrée à la source). `pred_dev="virtual_device"` identifie les
    // devices virtuels publiés par les flows (`flow_{id}`).
    let sources_catalog = inspector::SourceCatalog {
        by_source: cat
            .as_ref()
            .filter(|c| c.available)
            .map(|c| {
                let mut map: std::collections::BTreeMap<String, Vec<String>> =
                    std::collections::BTreeMap::new();
                for s in &c.series {
                    map.entry(s.device_id.clone())
                        .or_default()
                        .push(s.metric.clone());
                }
                map
            })
            .unwrap_or_default(),
        flows: cat
            .as_ref()
            .filter(|c| c.available)
            .map(|c| {
                c.series
                    .iter()
                    .filter(|s| s.pred_dev.as_deref() == Some("virtual_device"))
                    .map(|s| s.device_id.clone())
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default(),
        ready: cat.is_some(),
        memory: memory_catalog,
    };

    let device_metrics = sources_catalog.by_source.clone();

    // ── Slot inspecteur : monté seulement sur sélection (principe 4).
    // Titre = titre du widget sinon libellé du type ; « Trait » pour un lien.
    let inspector_slot: Option<Element> = cx.selected.cloned().map(|sel| match sel {
        Selection::Widget(id) => {
            let w = state::find_widget(&cx.layout.read(), &id);
            let kind = w.as_ref().map(|w| w.widget_type.clone());
            let name: Option<String> = w.map(|w| w.title).filter(|title| !title.is_empty());
            let (icon, _) = match &kind {
                Some(k) => library::kind_icon(k),
                None => (PaletteIcon::Puzzle, ""),
            };
            let title = name.unwrap_or_else(|| match &kind {
                Some(k) => library::kind_label(k),
                None => id.clone(),
            });
            rsx! {
                InspectorPanel {
                    key: "{id}",
                    icon,
                    title,
                    subtitle: Some(format!("#{id}")),
                    on_close: move |_| cx.selected.set(None),
                    body: rsx! {
                        inspector::InspectorBody {
                            cx,
                            can_write,
                            metrics,
                            catalog: sources_catalog,
                        }
                    },
                }
            }
        }
        Selection::Wire(id) => rsx! {
            InspectorPanel {
                key: "{id}",
                icon: PaletteIcon::Spline,
                title: t!("db-tool-wire").to_string(),
                subtitle: Some(format!("#{id}")),
                on_close: move |_| cx.selected.set(None),
                body: rsx! {
                    inspector::InspectorBody {
                        cx,
                        can_write,
                        metrics,
                        catalog: sources_catalog,
                    }
                },
            }
        },
    });

    rsx! {
        EditorShell {
            on_back: move |_| on_back.call(()),
            title: cx.name.cloned(),
            on_rename: if can_write { Some(Callback::new(move |name: String| save(cx, Some(name)))) } else { None },
            status: EditorStatus::new(StatusTone::Green, t!("eshell-status-live")),
            version: Some(cx.saved_version.cloned()),
            extra_chips: rsx! {
                if dirty {
                    Chip { tone: StatusTone::Amber, label: t!("db-unsaved").to_string() }
                }
            },
            actions: rsx! {
                button {
                    class: if save_disabled { "inline-flex items-center px-4 py-1.5 text-sm font-medium text-white bg-blue-600 rounded-lg opacity-40 cursor-not-allowed" } else { "inline-flex items-center px-4 py-1.5 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700" },
                    disabled: save_disabled,
                    onclick: move |_| save(cx, None),
                    icons::Save { class: "h-4 w-4 mr-1" }
                    {t!("db-save")}
                }
                button {
                    class: "inline-flex items-center px-3 py-1.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                    onclick: move |_| versions_open.set(true),
                    icons::History { class: "h-4 w-4 mr-1" }
                    {t!("ver-title")}
                }
            },
            banner: rsx! {
                if has_violations {
                    div { class: "rounded-lg border border-red-200 bg-red-50 p-3 text-sm text-red-700",
                        for v in violations.read().iter() {
                            ViolationLine { v: v.clone() }
                        }
                    }
                }
                if !can_write {
                    p { class: "rounded-lg border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800",
                        {t!("db-frozen")}
                    }
                }
            },
            canvas: rsx! {
                if mobile_format {
                    mobile::MobileComposer { cx }
                } else {
                    CanvasView { cx }
                }
            },
            palette: rsx! {
                PalettePopover {
                    add_title: t!("eshell-add-widget").to_string(),
                    title: t!("lib-title").to_string(),
                    search_placeholder: t!("eshell-search").to_string(),
                    items: library::palette_items(),
                    on_pick: move |kind: String| {
                        if kind == library::SYMBOLS_KEY {
                            cx.symbols_open.set(true);
                        } else if kind == library::FROM_DEVICE_KEY {
                            cx.devices_open.set(true);
                        } else {
                            library::add_new_widget(cx, &kind);
                        }
                    },
                    footer: rsx! {
                        TemplateList { cx }
                    },
                }
            },
            tools: rsx! {
                ToolsPill { cx, mobile: mobile_format }
                if cx.symbols_open.cloned() {
                    SymbolPanel { cx }
                }
                if cx.devices_open.cloned() {
                    device_panel::DevicePanel { cx, by_source: device_metrics }
                }
            },
            inspector: inspector_slot,
            empty_hint: if cx.layout.cloned().widgets.is_empty() && !mobile_format { Some(t!("eshell-empty-hint").to_string()) } else { None },
        }
        // ── Modales / tiroir versions
        if conflict() {
            ConflictModal { cx, on_changed }
        }
        if let Some(widget_id) = save_as() {
            SaveAsTemplate { key: "{widget_id}", cx, widget_id }
        }
        if versions_open() {
            VersionsDrawer {
                cx,
                on_close: move |_| versions_open.set(false),
                on_changed,
            }
        }
    }
}

/// Pill d'outils flottante (sélection / trait / undo / redo) — slot
/// `tools` de la coquille, à côté du bouton « + ». Les libellés deviennent
/// des infobulles : la pill gagne la place que la barre pleine largeur
/// occupait.
#[component]
fn ToolsPill(mut cx: EditorCx, mobile: bool) -> Element {
    let select_title = t!("db-tool-select");
    let wire_title = t!("db-tool-wire");
    let undo_title = t!("db-undo");
    let redo_title = t!("db-redo");
    let symbols_title = t!("sym-library");
    rsx! {
        div { class: "flex items-center gap-1 rounded-full border border-gray-200 bg-white p-1 shadow-lg",
            button {
                hidden: mobile,
                class: if cx.tool.cloned() == Tool::Select { "flex h-8 w-8 items-center justify-center rounded-full bg-blue-600 text-white" } else { "flex h-8 w-8 items-center justify-center rounded-full text-gray-600 hover:bg-gray-100" },
                title: "{select_title}",
                onclick: move |_| {
                    cx.tool.set(Tool::Select);
                    cx.wire_draft.set(None);
                },
                icons::MousePointer { class: "h-4 w-4" }
            }
            button {
                hidden: mobile,
                class: if cx.tool.cloned() == Tool::Wire { "flex h-8 w-8 items-center justify-center rounded-full bg-blue-600 text-white" } else { "flex h-8 w-8 items-center justify-center rounded-full text-gray-600 hover:bg-gray-100" },
                title: "{wire_title}",
                onclick: move |_| {
                    cx.tool.set(Tool::Wire);
                    cx.selected.set(None);
                    cx.wire_draft.set(None);
                },
                icons::Spline { class: "h-4 w-4" }
            }
            button {
                class: if cx.symbols_open.cloned() { "flex h-8 w-8 items-center justify-center rounded-full bg-violet-600 text-white" } else { "flex h-8 w-8 items-center justify-center rounded-full text-gray-600 hover:bg-gray-100" },
                title: "{symbols_title}",
                onclick: move |_| cx.symbols_open.toggle(),
                icons::Shapes { class: "h-4 w-4" }
            }
            button {
                class: "flex h-8 w-8 items-center justify-center rounded-full text-gray-600 hover:bg-gray-100 disabled:opacity-40",
                disabled: cx.history.read().undo_disabled(),
                title: "{undo_title}",
                onclick: move |_| {
                    let current = cx.layout.read().clone();
                    let restored = cx.history.with_mut(|h| h.undo(&current));
                    if let Some(l) = restored {
                        cx.layout.set(l);
                        cx.selected.set(None);
                    }
                },
                icons::Undo { class: "h-4 w-4" }
            }
            button {
                class: "flex h-8 w-8 items-center justify-center rounded-full text-gray-600 hover:bg-gray-100 disabled:opacity-40",
                disabled: cx.history.read().redo_disabled(),
                title: "{redo_title}",
                onclick: move |_| {
                    let current = cx.layout.read().clone();
                    let restored = cx.history.with_mut(|h| h.redo(&current));
                    if let Some(l) = restored {
                        cx.layout.set(l);
                        cx.selected.set(None);
                    }
                },
                icons::Redo { class: "h-4 w-4" }
            }
        }
    }
}

// GRID sert au snap côté gestes ; import gardé explicite pour la lisibilité
// des constantes partagées avec geometry.
#[allow(unused)]
const _GRID: f64 = GRID;

#[component]
fn ViolationLine(v: VizViolation) -> Element {
    rsx! {
        p { key: "{v.code}-{v.widget_id.clone().unwrap_or_default()}", "{v.message}" }
    }
}
