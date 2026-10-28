//! Tour editor (Studio) — floor plan + panoramic scenes + links,
//! versioned save, flows approach. `flow_editor` skeleton: every
//! mutation goes through the pure reducers of `state.rs`, the geometry
//! is tested in `geometry.rs`, rendering/gestures in `canvas.rs`, the
//! forms in `inspector.rs`, history + publication in
//! `versions.rs`.

pub(crate) mod canvas;
pub(crate) mod geometry;
pub(crate) mod inspector;
pub(crate) mod media_picker;
pub(crate) mod state;
pub(crate) mod versions;

pub(crate) use media_picker::MediaPicker;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{TourDoc, TourViolation, UpdateTour};

use crate::api;
use crate::api::media::MediaKind;
use crate::api::tours::classify_save_error;
use crate::components::editor_shell::floating_panel::FloatingPanel;
use crate::components::editor_shell::palette::PaletteIcon;
use crate::components::editor_shell::{
    Chip, EditorShell, EditorStatus, InspectorPanel, PaletteItem, PalettePopover, StatusTone,
};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::tour_viewer;
use crate::components::ui::{
    BTN_DANGER_SOLID, BTN_PRIMARY, BTN_PRIMARY_SM, BTN_SECONDARY, BTN_SECONDARY_SM,
};
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::state::session;
use crate::state::toasts;

/// Gesture in progress (flat state machine — a single active variant).
#[derive(Clone, PartialEq)]
pub(crate) enum Interaction {
    /// No gesture.
    Idle,
    /// Background pan: client delta since the start of the gesture.
    Panning {
        start_client: (f64, f64),
        start_pan: (f64, f64),
    },
    /// Scene drag: `grab` = cursor↔pin offset, measured at the start of the
    /// gesture (frozen rect — flow_editor approach).
    Dragging {
        id: String,
        grab: (f64, f64),
        rect: (f64, f64, f64, f64),
    },
}

/// Selected element (inspector + Delete key).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Select {
    Scene(String),
    Floor(String),
    Link(String),
}

/// Bundle of `Copy` signals shared between the editor and its subcomponents
/// (`EditorCx` approach).
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TourEditorCx {
    /// Document being edited (what Save will send).
    pub(crate) doc: Signal<TourDoc>,
    /// Dirty baseline: document of the last saved version.
    pub(crate) saved_doc: Signal<TourDoc>,
    /// Number of the latest version known server side.
    pub(crate) saved_version: Signal<i64>,
    /// Current selection (inspector).
    pub(crate) selected: Signal<Option<Select>>,
    /// Gesture in progress.
    pub(crate) interaction: Signal<Interaction>,
    pub(crate) pan: Signal<(f64, f64)>,
    pub(crate) zoom: Signal<f64>,
    /// Floor shown in the canvas.
    pub(crate) active_floor: Signal<String>,
    /// "Link" mode: clicking two scenes creates a hotspot.
    pub(crate) link_mode: Signal<bool>,
    /// Source of the pending link (link mode active).
    pub(crate) link_from: Signal<Option<String>>,
    /// Current violations (local or received as a 400 from the server).
    pub(crate) violations: Signal<Vec<TourViolation>>,
}

impl TourEditorCx {
    /// Document mutation through a pure reducer of `state.rs`.
    pub(crate) fn update_doc(mut self, f: impl FnOnce(&mut TourDoc)) {
        self.doc.with_mut(f);
    }
}

/// Editor mounted by the `/studio` page — orchestration: loading, save,
/// 409 conflict, floors, versions drawer, preview.
#[component]
pub fn TourEditor(
    tour_id: String,
    can_write: bool,
    on_back: Callback<()>,
    on_changed: Callback<()>,
) -> Element {
    let mut reload_meta = use_signal(|| 0u32);
    // String prop: one named local clone per closure (CanvasNode approach).
    let tour_id_detail = tour_id.clone();
    // D42 labels button target.
    let tour_id_labels = tour_id.clone();
    let detail = use_resource(move || {
        let _ = reload_meta();
        let tour_id = tour_id_detail.clone();
        async move { api::tours::detail(&tour_id).await }
    });

    // --- Editor state (the signals live in the `cx` bundle) ---
    let mut doc = use_signal(TourDoc::default);
    let mut saved_doc = use_signal(TourDoc::default);
    let mut saved_version = use_signal(|| 0i64);
    let mut selected = use_signal(|| None::<Select>);
    let interaction = use_signal(|| Interaction::Idle);
    let pan = use_signal(|| (0.0_f64, 0.0_f64));
    let zoom = use_signal(|| 1.0_f64);
    let mut active_floor = use_signal(String::new);
    let mut link_mode = use_signal(|| false);
    let mut link_from = use_signal(|| None::<String>);
    let mut violations = use_signal(Vec::<TourViolation>::new);
    let mut loaded_from = use_signal(|| None::<i64>);

    // Initial load guard (never a `.set()` during render).
    let mut loaded = use_signal(|| false);
    use_effect(move || {
        let resource = detail.value();
        let value = resource.read();
        let Some(Ok(tour)) = &*value else {
            return;
        };
        if loaded() {
            return;
        }
        let fresh = tour.doc.clone();
        // Active floor: first floor of the doc (the minimal doc has one).
        let floor = fresh
            .floors
            .first()
            .map(|f| f.id.clone())
            .unwrap_or_default();
        doc.set(fresh.clone());
        saved_doc.set(fresh);
        saved_version.set(tour.latest_version_number);
        active_floor.set(floor);
        loaded.set(true);
    });

    let mut cx = TourEditorCx {
        doc,
        saved_doc,
        saved_version,
        selected,
        interaction,
        pan,
        zoom,
        active_floor,
        link_mode,
        link_from,
        violations,
    };

    // Derived dirty — never a dedicated signal to keep up to date.
    let dirty = doc() != saved_doc();

    // --- Save (validate → PATCH) ---
    let mut saving = use_signal(|| false);
    let mut conflict = use_signal(|| None::<String>);
    let tour_id_save = tour_id.clone();
    // Save (button, unchanged name) and rename (shell title)
    // share the same PATCH — the doc travels along: renaming means
    // saving the current state.
    let save = Callback::new(move |name: Option<String>| {
        if saving() {
            return;
        }
        let local = pnex_core::validate_tour_doc(&doc.peek().clone());
        if !local.is_empty() {
            violations.set(local);
            return;
        }
        saving.set(true);
        let renamed = name.is_some();
        let params = UpdateTour {
            expected_version_number: saved_version(),
            doc: doc(),
            name,
            author: session::user().map(|user| user.username),
            note: None,
        };
        let id = tour_id_save.clone();
        spawn(async move {
            match api::tours::update(&id, params).await {
                Ok(tour) => {
                    saved_doc.set(tour.doc.clone());
                    saved_version.set(tour.latest_version_number);
                    violations.set(Vec::new());
                    loaded_from.set(None);
                    toasts::success("toast-tour-saved");
                    // The header title comes from the detail resource.
                    if renamed {
                        reload_meta.with_mut(|r| *r += 1);
                    }
                    on_changed.call(());
                }
                Err(err) => match classify_save_error(&err) {
                    api::tours::SaveError::Conflict { description } => {
                        conflict.set(Some(description));
                    }
                    api::tours::SaveError::Invalid(invalid) => violations.set(invalid),
                    api::tours::SaveError::Other(message) => toasts::error(message),
                },
            }
            saving.set(false);
        });
    });

    // --- 409 conflict resolution (flows approach: reload / overwrite) ---
    // Handler factory: the same behaviour is set on TWO buttons
    // ("loaded from vN" chip + conflict modal) — a captured String
    // cannot serve two `move` closures (approach: named clones).
    let tour_id_reload = tour_id.clone();
    let make_reload = || {
        let id = tour_id_reload.clone();
        // Param typed (): the same behaviour serves the purple chip
        // (Callback<()>) and the conflict modal (which wraps for
        // MouseData) without inference conflicts.
        move |_: ()| {
            conflict.set(None);
            let id = id.clone();
            spawn(async move {
                match api::tours::detail(&id).await {
                    Ok(tour) => {
                        let fresh = tour.doc.clone();
                        doc.set(fresh.clone());
                        saved_doc.set(fresh);
                        saved_version.set(tour.latest_version_number);
                        violations.set(Vec::new());
                        loaded_from.set(None);
                    }
                    Err(err) => toasts::error(err),
                }
            });
        }
    };
    let mut conflict_reload = make_reload();
    let conflict_reload_chip = make_reload();
    let tour_id_overwrite = tour_id.clone();
    let conflict_overwrite = move |_| {
        conflict.set(None);
        let id = tour_id_overwrite.clone();
        spawn(async move {
            let Ok(server) = api::tours::detail(&id).await else {
                toasts::error(t!("common-error").to_string());
                return;
            };
            let params = UpdateTour {
                // Fresh server version: overwrite on purpose.
                expected_version_number: server.latest_version_number,
                doc: doc.peek().clone(),
                name: None,
                author: session::user().map(|user| user.username),
                note: None,
            };
            match api::tours::update(&id, params).await {
                Ok(tour) => {
                    saved_doc.set(tour.doc.clone());
                    saved_version.set(tour.latest_version_number);
                    violations.set(Vec::new());
                    toasts::success("toast-tour-saved");
                    on_changed.call(());
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // --- New floor modal + scene picker + name modal ---
    let mut add_floor_open = use_signal(|| false);
    let mut add_scene_open = use_signal(|| false);
    let mut versions_open = use_signal(|| false);
    let mut preview_open = use_signal(|| false);
    // Current doc for the preview (frozen on opening).
    let mut preview_doc = use_signal(|| None::<TourDoc>);
    // Server metadata for the drawer (writers' share token, published).
    let (share_token, published_version, tour_name) = match &*detail.value().read() {
        Some(Ok(tour)) => (
            tour.share_token.clone(),
            tour.published_version_number,
            tour.name.clone(),
        ),
        _ => (None, None, String::new()),
    };

    let floors: Vec<pnex_core::TourFloor> = doc.read().floors.clone();
    let scene_count = doc.read().scenes.len();
    let banner = violations.cloned();
    // Floor buttons: (floor, active, scene count) — precomputed (no `let`
    // in the rsx body).
    let floor_buttons: Vec<(pnex_core::TourFloor, bool, usize)> = floors
        .iter()
        .map(|floor| {
            let count = doc
                .read()
                .scenes
                .iter()
                .filter(|s| s.floor_id == floor.id)
                .count();
            (floor.clone(), *active_floor.read() == floor.id, count)
        })
        .collect();

    // ─── Unified status (dot + label): published (green) / draft (grey).
    let detail_loaded = matches!(&*detail.value().read(), Some(Ok(_)));
    let editor_status = match (published_version, detail_loaded) {
        (Some(version), _) => EditorStatus::new(
            StatusTone::Green,
            t!("studio-published-tag", version: version),
        ),
        (None, true) => EditorStatus::new(StatusTone::Slate, t!("eshell-status-draft")),
        (None, false) => EditorStatus::new(StatusTone::Slate, t!("common-loading")),
    };

    // ─── Inspector slot: mounted only on selection (principle 4).
    let inspector_slot: Option<Element> = cx.selected.cloned().map(|sel| {
        let (icon, title, subtitle) = match &sel {
            Select::Scene(id) => {
                let name = doc
                    .read()
                    .scenes
                    .iter()
                    .find(|s| &s.id == id)
                    .map(|s| s.label.clone())
                    .filter(|label| !label.is_empty());
                (
                    PaletteIcon::Image,
                    name.unwrap_or_else(|| id.clone()),
                    id.clone(),
                )
            }
            Select::Floor(id) => {
                let name = doc
                    .read()
                    .floors
                    .iter()
                    .find(|f| &f.id == id)
                    .map(|f| f.name.clone())
                    .filter(|name| !name.is_empty());
                (
                    PaletteIcon::Layers,
                    name.unwrap_or_else(|| id.clone()),
                    id.clone(),
                )
            }
            Select::Link(id) => (
                PaletteIcon::Spline,
                t!("studio-inspector-link").to_string(),
                id.clone(),
            ),
        };
        rsx! {
            InspectorPanel {
                key: "{sel:?}",
                icon,
                title,
                subtitle: Some(format!("#{subtitle}")),
                on_close: move |_| cx.selected.set(None),
                body: rsx! {
                    inspector::InspectorBody { cx, can_write }
                },
            }
        }
    });

    // Floating floors/link panel (formerly the left sidebar).
    let mut floors_panel_open = use_signal(|| false);
    // Exclusive with the "+" palette; closes on a click outside.
    crate::state::ui::use_exclusive_menu(floors_panel_open);
    let floors_title = t!("studio-floors-title");

    rsx! {
        EditorShell {
            on_back,
            title: tour_name,
            on_rename: if can_write { Some(Callback::new(move |name: String| save.call(Some(name)))) } else { None },
            status: editor_status,
            version: if saved_version() > 0 { Some(saved_version()) } else { None },
            extra_chips: rsx! {
                if dirty {
                    Chip {
                        tone: StatusTone::Amber,
                        label: t!("studio-dirty-unsaved").to_string(),
                    }
                }
                if let Some(version) = loaded_from() {
                    Chip {
                        tone: StatusTone::Purple,
                        label: format!("v{version}"),
                        on_remove: conflict_reload_chip,
                    }
                }
            },
            actions: rsx! {
                if can_write {
                    button {
                        class: BTN_PRIMARY_SM,
                        disabled: saving() || !dirty,
                        onclick: move |_| save.call(None),
                        {t!("common-save")}
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| {
                        preview_doc.set(Some(doc()));
                        preview_open.set(true);
                    },
                    {t!("studio-preview")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    title: t!("studio-versions"),
                    aria_label: t!("studio-versions"),
                    onclick: move |_| versions_open.set(true),
                    icons::History { class: "h-4 w-4 sm:mr-1" }
                    span { class: "hidden sm:inline", {t!("studio-versions")} }
                }
                // Site breadcrumb (nothing while the tour is not placed).
                div { class: "hidden md:block",
                    crate::components::location_breadcrumb::LocationBreadcrumb {
                        kind: pnex_core::resources::KIND_TOUR.to_string(),
                        id: tour_id_labels.clone(),
                        compact: true,
                    }
                }
                // D42 labels of the tour (same editor as every resource).
                crate::components::labels_editor::LabelsButton {
                    kind: pnex_core::resources::KIND_TOUR.to_string(),
                    id: tour_id_labels.clone(),
                    can_write,
                }
            },
            banner: rsx! {
                // ─── Violations banner ───
                if !banner.is_empty() {
                    div { class: "bg-red-50 border border-red-200 rounded-lg px-4 py-2 text-sm text-red-700",
                        span { class: "font-semibold mr-2", {t!("studio-violations-banner-title")} }
                        ul { class: "list-disc list-inside",
                            for violation in banner.clone() {
                                li { key: "{violation.code}-{violation.subject:?}-{violation.message}",
                                    {
                                        crate::api::tours::violation_text(
                                            violation.subject.as_deref(),
                                            &violation.code,
                                            &violation.message,
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
            },
            canvas: rsx! {
                canvas::Canvas { cx, can_write }
            },
            palette: rsx! {
                PalettePopover {
                    add_title: t!("eshell-add").to_string(),
                    title: t!("eshell-add").to_string(),
                    search_placeholder: t!("eshell-search").to_string(),
                    items: vec![
                        PaletteItem::new("floor", t!("studio-add-floor").to_string())
                            .with_icon(PaletteIcon::Layers, "bg-signal-50 text-signal-600"),
                        PaletteItem::new("scene", t!("studio-add-scene").to_string())
                            .with_icon(PaletteIcon::Image, "bg-emerald-50 text-emerald-600"),
                    ],
                    on_pick: move |key: String| match key.as_str() {
                        "floor" => add_floor_open.set(true),
                        "scene" => {
                            link_from.set(None);
                            add_scene_open.set(true);
                        }
                        _ => {}
                    },
                }
            },
            tools: rsx! {
                div {
                    button {
                        class: if floors_panel_open() {
                            "flex h-10 w-10 items-center justify-center rounded-full bg-signal-600 text-white shadow-lg"
                        } else {
                            "flex h-10 w-10 items-center justify-center rounded-full border border-gray-200 bg-white text-gray-600 shadow-lg hover:bg-gray-50"
                        },
                        title: "{floors_title}",
                        onclick: move |_| floors_panel_open.toggle(),
                        crate::components::icons::Layers { class: "h-5 w-5" }
                    }
                    if floors_panel_open() {
                        FloatingPanel {
                            title: floors_title.clone(),
                            on_close: move |_| floors_panel_open.set(false),
                            div { class: "flex flex-col gap-1",
                                for (floor, is_active, count) in floor_buttons.clone() {
                                    button {
                                        key: "{floor.id}",
                                        class: if is_active {
                                            "w-full text-left px-2 py-1.5 text-sm rounded-lg bg-signal-50 text-signal-700 border border-signal-200"
                                        } else {
                                            "w-full text-left px-2 py-1.5 text-sm rounded-lg hover:bg-gray-50 text-gray-700 border border-transparent"
                                        },
                                        onclick: move |_| {
                                            // Selection + display: the inspector
                                            // edits the floor (name/plan/scale).
                                            selected.set(Some(Select::Floor(floor.id.clone())));
                                            active_floor.set(floor.id.clone());
                                            link_from.set(None);
                                        },
                                        div { class: "flex items-center justify-between gap-1",
                                            span { class: "truncate",
                                                if floor.name.is_empty() {
                                                    {floor.id.clone()}
                                                } else {
                                                    {floor.name.clone()}
                                                }
                                            }
                                            span { class: "text-xs text-gray-500", "{count}" }
                                        }
                                    }
                                }
                            }
                            if can_write {
                                button {
                                    class: if link_mode() {
                                        "w-full px-2 py-1.5 text-sm text-amber-700 bg-amber-50 border border-amber-300 rounded-lg"
                                    } else {
                                        "w-full px-2 py-1.5 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors"
                                    },
                                    onclick: move |_| {
                                        link_mode.set(!link_mode());
                                        link_from.set(None);
                                    },
                                    {t!("studio-link-mode")}
                                }
                            }
                            if link_mode() {
                                p { class: "text-xs text-gray-500",
                                    if link_from().is_some() {
                                        {t!("studio-link-hint-target")}
                                    } else {
                                        {t!("studio-link-hint-source")}
                                    }
                                }
                            }
                            span { class: "text-xs text-gray-500", {t!("studio-scene-count", count : scene_count)} }
                        }
                    }
                }
            },
            inspector: inspector_slot,
            empty_hint: if scene_count == 0 { Some(t!("eshell-empty-hint").to_string()) } else { None },
        }

        // ─── Drawer versions ───
        if versions_open() {
            versions::VersionsDrawer {
                tour_id: tour_id.clone(),
                can_write,
                share_token: share_token.clone(),
                published_version,
                on_close: move |_| versions_open.set(false),
                on_loaded: move |version: pnex_core::TourVersionDetail| {
                    // Loading a version: if it is the latest,
                    // no dirty; otherwise save will create v(n+1) with this doc
                    // (restore by editing, flows approach).
                    let fresh = version.doc.clone();
                    doc.set(fresh.clone());
                    if version.version_number == saved_version() {
                        saved_doc.set(fresh);
                        loaded_from.set(None);
                    } else {
                        saved_doc.set(TourDoc::default());
                        loaded_from.set(Some(version.version_number));
                    }
                    selected.set(None);
                    violations.set(Vec::new());
                    versions_open.set(false);
                },
                on_changed: move |_| {
                    reload_meta.with_mut(|r| *r += 1);
                    on_changed.call(());
                },
            }
        }

        // ─── Preview (in-app viewer) ───
        if preview_open() {
            Modal {
                title: t!("studio-preview-title"),
                max_width: "max-w-5xl".to_string(),
                on_close: move |_| {
                    preview_open.set(false);
                    preview_doc.set(None);
                },
                if let Some(preview) = preview_doc.cloned() {
                    div { class: "space-y-3",
                        div { class: "flex gap-3",
                            crate::components::tour_viewer::TourViewer {
                                key: "preview-{tour_id}",
                                doc: preview,
                                assets: Default::default(),
                                source: crate::components::tour_viewer::ViewerSource::Auth,
                                // Arrow drag: patch the editor doc (the
                                // change survives closing the
                                // preview) AND the displayed doc (the hotspots of
                                // the next switches read the updated angles).
                                on_hotspot_move: move |(link_id, yaw, pitch): (String, f64, f64)| {
                                    let id = link_id.clone();
                                    cx.update_doc(move |doc| {
                                        state::set_link_angles(doc, &id, yaw, pitch);
                                    });
                                    if let Some(mut preview) = preview_doc.cloned() {
                                        if let Some(link) =
                                            preview.links.iter_mut().find(|l| l.id == link_id)
                                        {
                                            link.yaw = yaw.clamp(-180.0, 180.0);
                                            link.pitch = pitch.clamp(-90.0, 90.0);
                                        }
                                        preview_doc.set(Some(preview));
                                    }
                                },
                                host_id: tour_viewer::HOST_ID.to_string(),
                                compact: false,
                                show_side_panel: true,
                                // Read-only overlay of the published
                                // annotations: they are edited in Data >
                                // Annotations only (D147).
                                annotations_enabled: true,
                                annotation_tour: Some(tour_id.clone()),
                                on_scene_change: move |_: String| {},
                                // Tour editor preview: arrow drag is
                                // allowed here and nowhere else (read-only
                                // viewers keep markers fixed).
                                editable: true,
                            }
                        }
                    }
                }
            }
        }

        // ─── New floor modal ───
        if add_floor_open() {
            AddFloorModal {
                on_close: move |_| add_floor_open.set(false),
                on_created: move |(name, level)| {
                    add_floor_open.set(false);
                    // The reducer picks the free id; we show and
                    // select the created floor.
                    let mut new_id = String::new();
                    cx.update_doc(|doc| {
                        new_id = state::add_floor(doc, name, level);
                    });
                    active_floor.set(new_id);
                },
            }
        }

        // ─── Add scene modal (panorama picker) ───
        if add_scene_open() {
            MediaPicker {
                kind: MediaKind::Panorama,
                on_picked: move |picked: (String, String)| {
                    let (asset_id, _name) = picked;
                    add_scene_open.set(false);
                    // Placed on the active floor, first free spot around the
                    // plan centre (never stacked).
                    let floor = active_floor.cloned();
                    let mut new_id = String::new();
                    cx.update_doc(|doc| {
                        // Active floor's own plan size and scenes only: other
                        // floors never push a new pin aside.
                        let plan = doc
                            .floors
                            .iter()
                            .find(|f| f.id == floor)
                            .and_then(|f| f.plan.as_ref());
                        let size = geometry::plan_size(
                            plan.and_then(|p| p.width),
                            plan.and_then(|p| p.height),
                        );
                        let taken: Vec<(f64, f64)> = doc
                            .scenes
                            .iter()
                            .filter(|s| s.floor_id == floor)
                            .map(|s| (s.x, s.y))
                            .collect();
                        let pos = geometry::new_scene_position(size, &taken);
                        new_id = state::add_scene(doc, &floor, asset_id.clone(), pos);
                    });
                    selected.set(Some(Select::Scene(new_id)));
                },
                on_close: move |_| add_scene_open.set(false),
            }
        }

        // ─── 409 conflict modal (flow_editor approach) ───
        if let Some(description) = conflict() {
            Modal {
                title: t!("studio-conflict-title"),
                max_width: "max-w-md".to_string(),
                on_close: move |_| conflict.set(None),
                div { class: "space-y-4",
                    p { class: "text-sm text-gray-600", {description.clone()} }
                    p { class: "text-sm text-gray-600", {t!("studio-conflict-message")} }
                    div { class: "flex flex-col gap-2 pt-2",
                        button {
                            class: BTN_SECONDARY,
                            onclick: move |_| conflict_reload(()),
                            {t!("studio-conflict-reload")}
                        }
                        button {
                            class: BTN_DANGER_SOLID,
                            onclick: conflict_overwrite,
                            {t!("studio-conflict-overwrite")}
                        }
                    }
                }
            }
        }
    }
}

/// Floor creation modal: name + level.
#[component]
fn AddFloorModal(on_close: Callback<()>, on_created: Callback<(String, i32)>) -> Element {
    let mut name = use_signal(String::new);
    let mut level = use_signal(|| "1".to_string());

    rsx! {
        Modal {
            title: t!("studio-add-floor-title"),
            max_width: "max-w-sm".to_string(),
            on_close,
            div { class: "space-y-4",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-floor-name")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "text",
                        value: "{name}",
                        oninput: move |event| name.set(event.value()),
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("studio-floor-level")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        step: "1",
                        value: "{level}",
                        oninput: move |event| level.set(event.value()),
                    }
                }
                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| on_close.call(()),
                        {t!("common-cancel")}
                    }
                    button {
                        class: BTN_PRIMARY,
                        r#type: "button",
                        onclick: move |_| {
                            // The id is resolved by the reducer on the parent side.
                            let level_value = level().parse::<i32>().unwrap_or(0);
                            let name_value = {
                                let value = name().trim().to_string();
                                if value.is_empty() { t!("studio-floor-unnamed").to_string() } else { value }
                            };
                            on_created.call((name_value, level_value));
                        },
                        {t!("studio-add-floor")}
                    }
                }
            }
        }
    }
}
