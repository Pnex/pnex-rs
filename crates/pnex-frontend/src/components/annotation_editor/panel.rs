//! Panneau d'édition d'un ensemble d'annotations (D59 + pivots UX 2026-09-16)
//! — l'ensemble est imposé par la page dédiée (Data > Annotations) : média
//! déclaré (000027) ou tour (000028 — le média suit la scène naviguée via
//! `media_override`). Versionné append-only (save = PATCH, 409 optimiste),
//! publiable. Items du média courant (liste + inspecteur), mode pose
//! (clic pano), drag d'ajustement, drawer versions + publication.
//! L'application des marqueurs (editable) est un effet du PANNEAU — le
//! TourViewer passe annotations_enabled=false dès que le panneau est
//! monté (un seul écrivain du host à la fois).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{validate_annotation_doc, UpdateAnnotationLayer};

use crate::api;
use crate::api::annotation_layers::{classify_save_error, SaveError};
use crate::components::annotation_editor::inspector::AnnotationInspector;
use crate::components::annotation_editor::state::{
    adopt_bound_controls, has_item, item_rows, item_rows_flat, move_item, place_item,
    AnnotationEditorCx, EditorItemRow,
};
use crate::components::annotation_editor::versions::AnnotationVersionsDrawer;
use crate::components::modal::Modal;
use crate::tour_viewer as bridge;
use crate::tour_viewer::AnnotMarkerView;

#[component]
pub fn AnnotationLayerPanel(
    /// Ensemble édité (page dédiée — liste d'objets).
    layer_id: String,
    /// Média courant porté par l'appelant : fixe pour un ensemble-média
    /// (000027), média de la scène naviguée pour un ensemble-tour
    /// (000028). Le panneau le suit.
    media_override: ReadSignal<Option<String>>,
    host_id: String,
    can_write: bool,
    /// État éditeur (signaux Copy) — possédé par l'appelant : la page
    /// rend elle-même les marqueurs PLATS (les hotspots pannellum ne
    /// portent que l'équirect).
    cx: AnnotationEditorCx,
    /// Kind du média annoté : `panorama` (géométrie sphère) sinon plat
    /// (photo/floorplan — x/y en fraction du conteneur).
    media_kind: String,
) -> Element {
    let is_pano = media_kind == "panorama";
    let mut versions_open = use_signal(|| false);
    // Dernier lot de marqueurs appliqués au host (dé-dup des effects).
    let mut applied_markers = use_signal(|| None::<String>);
    // Ensemble dont le détail est déjà chargé (dé-dup du boot).
    let mut booted_for = use_signal(|| None::<String>);
    // Version publiée de l'ensemble (read du détail).
    let mut published_version = use_signal(|| None::<i64>);
    // Média DÉCLARÉ de l'ensemble (000027) — posé au boot ; le média
    // COURANT est lu à l'usage (media_override, écrase le déclaré).
    let mut declared_media = use_signal(|| None::<String>);

    // ─── Boot : charge le détail de l'ensemble imposé (page dédiée) — le
    // média annoté initial = son média déclaré (000027), puis SUIT
    // `media_override` (ensemble-tour 000028 : média de la scène courante).
    use_effect(move || {
        if booted_for.cloned().as_deref() == Some(layer_id.as_str()) {
            return;
        }
        let layer = layer_id.clone();
        booted_for.set(Some(layer.clone()));
        cx.layer_id.set(None);
        spawn(async move {
            if let Ok(d) = api::annotation_layers::detail(&layer, None).await {
                published_version.set(d.published_version_number);
                declared_media.set(d.media_asset_id.clone());
                cx.layer_id.set(Some(d.id.clone()));
                cx.doc.set(d.doc.clone());
                cx.saved_doc.set(d.doc.clone());
                cx.saved_version.set(d.doc_version_number);
                cx.violations.set(Vec::new());
                cx.selected.set(None);
            }
        });
    });

    // Mode pose : piloter la glue JS (retry inclus côté bridge) — pano
    // uniquement (sur image plate, la page gère le clic elle-même).
    let host_place = host_id.clone();
    use_effect(move || {
        let placing = cx.placing.cloned() && is_pano;
        let host = host_place.clone();
        spawn(async move {
            bridge::set_annot_place_mode(&host, placing).await;
        });
    });

    // Poll place/move/click (boucle unique, tâche scopée au panneau).
    let mut poll_started = use_signal(|| false);
    let mut place_seq = use_signal(|| 0u64);
    let mut move_seq = use_signal(|| 0u64);
    let mut click_seq = use_signal(|| 0u64);
    use_effect(move || {
        if poll_started() {
            return;
        }
        poll_started.set(true);
        spawn(async move {
            loop {
                crate::util::sleep(std::time::Duration::from_millis(250)).await;
                if let Some(p) = bridge::take_annot_place(place_seq.cloned()).await {
                    place_seq.set(p.seq);
                    // Lecture DIRECTE des signaux au moment de l'event
                    // (le média courant du tour, pas un cache d'effet).
                    let asset = media_override
                        .cloned()
                        .or_else(|| declared_media.cloned())
                        .unwrap_or_default();
                    if asset.is_empty() {
                        continue;
                    }
                    cx.update_doc(move |doc| {
                        let id = place_item(doc, &asset, p.yaw, p.pitch);
                        cx.selected.set(Some(id));
                    });
                }
                if let Some(m) = bridge::take_annot_move(move_seq.cloned()).await {
                    move_seq.set(m.seq);
                    cx.update_doc(move |doc| move_item(doc, &m.item_id, m.yaw, m.pitch));
                }
                if let Some(c) = bridge::take_annot_click(click_seq.cloned()).await {
                    click_seq.set(c.seq);
                    cx.selected.set(Some(c.item_id));
                }
            }
        });
    });

    // Marqueurs : application post-mount au host (editable=true), dé-dup
    // par json. Retry interne au bridge (viewer pas encore monté au 1er tick).
    // PANORAMA uniquement — les marqueurs plats sont rendus par la page.
    let host_markers = host_id.clone();
    use_effect(move || {
        if !is_pano {
            return;
        }
        let doc = cx.doc.cloned();
        let asset = media_override
            .cloned()
            .or_else(|| declared_media.cloned())
            .unwrap_or_default();
        let rows: Vec<AnnotMarkerView> = item_rows(&doc, &asset)
            .into_iter()
            .map(|r| AnnotMarkerView {
                id: r.id,
                yaw: r.yaw,
                pitch: r.pitch,
                kind: r.kind,
                label: r.label,
            })
            .collect();
        let json = serde_json::to_string(&rows).unwrap_or_default();
        if applied_markers.cloned().as_deref() == Some(json.as_str()) {
            return;
        }
        applied_markers.set(Some(json.clone()));
        let host = host_markers.clone();
        spawn(async move {
            bridge::set_annotations(&host, &rows, true).await;
        });
    });

    // ─── Précalculs ───
    let asset_now = media_override
        .cloned()
        .or_else(|| declared_media.cloned())
        .unwrap_or_default();
    let doc_now = cx.doc.cloned();
    let rows_now = if is_pano {
        item_rows(&doc_now, &asset_now)
    } else {
        item_rows_flat(&doc_now, &asset_now)
            .into_iter()
            .map(|f| EditorItemRow {
                id: f.id,
                kind: f.kind,
                label: f.label,
                yaw: f.x * 100.0,
                pitch: f.y * 100.0,
            })
            .collect::<Vec<EditorItemRow>>()
    };
    let selected_id = cx.layer_id.cloned();
    let dirty = cx.is_dirty();
    let violations_now = cx.violations.cloned();
    let placing_now = cx.placing.cloned();
    let conflict_now = cx.conflict.cloned();
    let versions_open_now = versions_open.cloned();
    let published_now = published_version.cloned();

    rsx! {
        div { class: "w-96 shrink-0 flex flex-col border-l border-gray-200 bg-white h-full max-h-[70vh]",
            // ─── En-tête : titre + versions ───
            div { class: "flex items-center justify-between px-3 py-2 border-b border-gray-200",
                span { class: "text-sm font-semibold text-gray-900", {t!("annot-panel-title")} }
                button {
                    class: "inline-flex items-center px-2 py-1 text-xs text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50",
                    onclick: move |_| versions_open.set(true),
                    disabled: cx.layer_id.cloned().is_none(),
                    crate::components::icons::History { class: "h-3.5 w-3.5 mr-1" }
                    {t!("annot-versions")}
                }
            }
            // ─── Toolbar : pose + save ───
            if can_write && cx.layer_id.cloned().is_some() {
                div { class: "px-3 py-2 border-b border-gray-200 flex items-center gap-2",
                    button {
                        class: if placing_now { "px-3 py-1.5 text-xs bg-blue-600 text-white rounded-lg font-medium" } else { "px-3 py-1.5 text-xs bg-white text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50" },
                        onclick: move |_| cx.placing.toggle(),
                        {t!("annot-place-toggle")}
                    }
                    button {
                        class: "px-3 py-1.5 text-xs bg-emerald-600 text-white rounded-lg hover:bg-emerald-700 font-medium disabled:opacity-40 ml-auto",
                        disabled: !dirty || cx.saving.cloned() || !violations_now.is_empty(),
                        onclick: move |_| {
                            if !cx.is_dirty() {
                                return;
                            }
                            let doc = cx.doc.cloned();
                            cx.saving.set(true);
                            cx.conflict.set(None);
                            let layer = cx.layer_id.cloned().unwrap_or_default();
                            let expected = cx.saved_version.cloned();
                            spawn(async move {
                                let res = api::annotation_layers::update(
                                        &layer,
                                        UpdateAnnotationLayer {
                                            expected_version_number: expected,
                                            doc,
                                            name: None,
                                            author: None,
                                            note: None,
                                        },
                                    )
                                    .await;
                                match res {
                                    Ok(d) => {
                                        let local = adopt_bound_controls(&cx.doc.cloned(), &d.doc);
                                        cx.doc.set(local);
                                        cx.saved_doc.set(d.doc.clone());
                                        cx.saved_version.set(d.doc_version_number);
                                        crate::state::toasts::success(t!("toast-annot-saved"));
                                    }
                                    Err(err) => {
                                        match classify_save_error(&err) {
                                            SaveError::Conflict { description } => {
                                                cx.conflict.set(Some(description));
                                            }
                                            SaveError::Invalid(vs) => {
                                                cx.violations.set(vs);
                                            }
                                            SaveError::Other(msg) => {
                                                crate::state::toasts::error(msg);
                                            }
                                        }
                                    }
                                }
                                cx.saving.set(false);
                            });
                        },
                        {t!("annot-save")}
                    }
                }
                if placing_now {
                    div { class: "px-3 pb-2 text-xs text-blue-700 bg-blue-50 border-b border-blue-100",
                        {t!("annot-place-hint")}
                    }
                }
            }
            // ─── Bandeau violations ───
            if !violations_now.is_empty() {
                div { class: "px-3 py-2 bg-red-50 border-b border-red-200 text-xs text-red-700 space-y-1",
                    p { class: "font-semibold", {t!("annot-violations")} }
                    for v in violations_now {
                        p { key: "{v.code}-{v.subject.clone().unwrap_or_default()}",
                            {v.message}
                        }
                    }
                }
            }

            // ─── Liste des items du média courant ───
            div { class: "px-3 py-2 border-b border-gray-200",
                span { class: "text-xs font-semibold text-gray-500 uppercase tracking-wide",
                    {t!("annot-items-title")}
                }
            }
            div { class: "flex-1 overflow-y-auto",
                if rows_now.is_empty() {
                    p { class: "p-4 text-sm text-gray-400", {t!("annot-items-empty")} }
                }
                ul { class: "divide-y divide-gray-100",
                    for r in rows_now.clone() {
                        li { key: "row-{r.id}",
                            button {
                                class: if cx.selected.cloned().as_deref() == Some(r.id.as_str()) { "w-full text-left px-3 py-2 hover:bg-gray-50 bg-blue-50" } else { "w-full text-left px-3 py-2 hover:bg-gray-50" },
                                onclick: move |_| cx.selected.set(Some(r.id.clone())),
                                div { class: "flex items-center gap-2",
                                    span { class: "inline-block h-2.5 w-2.5 rounded-full {crate::components::annotation_editor::popover::annot_dot_color(&r.kind)}" }
                                    span { class: "text-sm text-gray-900 truncate",
                                        {
                                            if r.label.is_empty() {
                                                t!("annot-kind-device").to_string()
                                            } else {
                                                r.label.clone()
                                            }
                                        }
                                    }
                                    span { class: "text-[11px] text-gray-400 ml-auto font-mono",
                                        // Pano : degrés ; plat : pourcentages
                                        // (x/y ∈ [0,1] mappés ×100 dans rows_now).
                                        {
                                            if is_pano {
                                                format!("{:.0}°/{:.0}°", r.yaw, r.pitch)
                                            } else {
                                                format!("{:.0}%/{:.0}%", r.yaw, r.pitch)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // Inspecteur (item sélectionné).
                if let Some(id) = cx.selected.cloned() {
                    if has_item(&doc_now, &id) {
                        AnnotationInspector { cx, can_write }
                    }
                }
            }
        }
        // ─── Drawer versions ───
        if versions_open_now {
            AnnotationVersionsDrawer {
                key: "annot-versions-{selected_id.clone().unwrap_or_default()}",
                layer_id: selected_id.clone().unwrap_or_default(),
                can_write,
                published_version: published_now,
                on_close: move |_| versions_open.set(false),
                on_loaded: move |detail: pnex_core::AnnotationLayerVersionDetail| {
                    versions_open.set(false);
                    cx.layer_id.set(Some(detail.id.clone()));
                    cx.doc.set(detail.doc.clone());
                    cx.saved_doc.set(detail.doc);
                    cx.saved_version.set(detail.version_number);
                    cx.violations.set(validate_annotation_doc(&cx.doc.cloned()));
                    cx.selected.set(None);
                },
                // Publication/dépublication : rafraîchir l'état publié local
                // via le détail (la liste des couches n'est plus chargée).
                on_changed: move |_| {
                    let id = cx.layer_id.cloned().unwrap_or_default();
                    spawn(async move {
                        if let Ok(d) = api::annotation_layers::detail(&id, None).await {
                            published_version.set(d.published_version_number);
                        }
                    });
                },
            }
        }
        // ─── Modal conflit 409 (recharger / écraser, école tours) ───
        if conflict_now.is_some() {
            Modal {
                title: t!("annot-conflict-title"),
                max_width: "max-w-md".to_string(),
                on_close: move |_| cx.conflict.set(None),
                div { class: "space-y-4",
                    p { class: "text-sm text-gray-600", {t!("annot-conflict-text")} }
                    div { class: "flex justify-end gap-2",
                        button {
                            class: "px-3 py-1.5 text-sm text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| {
                                cx.conflict.set(None);
                                let id = cx.layer_id.cloned().unwrap_or_default();
                                spawn(async move {
                                    if let Ok(d) = api::annotation_layers::detail(&id, None).await {
                                        cx.layer_id.set(Some(d.id.clone()));
                                        cx.doc.set(d.doc.clone());
                                        cx.saved_doc.set(d.doc.clone());
                                        cx.saved_version.set(d.doc_version_number);
                                    }
                                });
                            },
                            {t!("annot-conflict-reload")}
                        }
                        button {
                            class: "px-3 py-1.5 text-sm bg-red-600 text-white rounded-lg hover:bg-red-700",
                            onclick: move |_| {
                                cx.conflict.set(None);
                                let id = cx.layer_id.cloned().unwrap_or_default();
                                spawn(async move {
                                    // Écraser : repartir du numéro courant serveur.
                                    if let Ok(d) = api::annotation_layers::detail(&id, None).await {
                                        let expected = d.doc_version_number;
                                        let doc = cx.doc.cloned();
                                        match api::annotation_layers::update(
                                                &id,
                                                UpdateAnnotationLayer {
                                                    expected_version_number: expected,
                                                    doc,
                                                    name: None,
                                                    author: None,
                                                    note: None,
                                                },
                                            )
                                            .await
                                        {
                                            Ok(d) => {
                                                let local = adopt_bound_controls(&cx.doc.cloned(), &d.doc);
                                                cx.doc.set(local);
                                                cx.saved_doc.set(d.doc.clone());
                                                cx.saved_version.set(d.doc_version_number);
                                                crate::state::toasts::success(t!("toast-annot-saved"));
                                            }
                                            Err(err) => crate::state::toasts::error(err),
                                        }
                                    }
                                });
                            },
                            {t!("annot-conflict-overwrite")}
                        }
                    }
                }
            }
        }
    }
}
