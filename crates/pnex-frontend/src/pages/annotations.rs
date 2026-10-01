//! Page dédiée « Annotations » (pivot UX §9 v3, 2026-09-16) — une LISTE
//! classique d'objets « ensembles d'annotations » : chaque entrée porte un
//! nom global, un média associé (colonne media_asset_id, 000027) et ses
//! annotations, versionnées et publiables. Ouvrir un média = voir les
//! ensembles existants (filtre media=) pour réutiliser plutôt que dupliquer.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TourDoc;

use crate::api;
use crate::app::Route;
use crate::components::annotation_editor::panel::AnnotationLayerPanel;
use crate::components::annotation_editor::state::{
    item_rows_flat, move_item_flat, place_item_flat, AnnotationEditorCx,
};
use crate::components::confirm::ConfirmDialog;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::tour_viewer::{TourViewer, ViewerSource};
use crate::state::annotations::OPEN_LAYER;
use crate::state::tours::OPEN_TOUR;
use crate::state::{org, session, toasts};
use crate::util::media_blob_url;

/// Rôle de l'utilisateur dans l'org courante (école media.rs).
fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

/// Doc synthétique à une scène : le viewer tour sert de visionneuse
/// pannellum du média de l'ensemble (école D60(4)), sans chrome de tour.
fn synthetic_doc(media_asset_id: &str) -> TourDoc {
    let mut doc = TourDoc::default();
    doc.scenes.push(pnex_core::TourScene {
        id: "annot-scene".into(),
        floor_id: "f".into(),
        label: String::new(),
        media_asset_id: media_asset_id.to_string(),
        x: 0.0,
        y: 0.0,
        initial_yaw: 0.0,
        initial_pitch: 0.0,
        initial_fov: 100.0,
    });
    doc.start_scene = Some("annot-scene".into());
    doc
}

#[derive(Clone, PartialEq)]
enum View {
    List,
    Editor(String),
}

#[component]
pub fn Annotations() -> Element {
    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    // ─── Listes de base (médias pour les noms + filtre) ───
    let mut view = use_signal(|| View::List);
    let navigator = use_navigator();

    // Global search deep link (D69): consume the requested layer id once
    // (guard prevents the effect from re-arming — see flows.rs comment).
    let mut deep_link_done = use_signal(|| false);
    use_effect(move || {
        if deep_link_done() {
            return;
        }
        if let Some(id) = OPEN_LAYER() {
            view.set(View::Editor(id));
            OPEN_LAYER.with_mut(|f| *f = None);
            deep_link_done.set(true);
        }
    });
    let mut sets_reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
    let mut selected_media_filter = use_signal(String::new);
    let mut new_open = use_signal(|| false);
    let mut delete_target = use_signal(|| None::<pnex_core::AnnotationLayerSummary>);

    let media_res = use_resource(move || {
        let _ = sets_reload();
        async move {
            api::media::list(&api::media::MediaFilters {
                kinds: vec![
                    api::media::MediaKind::Photo,
                    api::media::MediaKind::Panorama,
                ],
                limit: Some(200),
                ..Default::default()
            })
            .await
        }
    });
    // (nom, kind) par média — le kind choisit le viewer (sphère vs plat).
    let media_map: std::collections::HashMap<String, (String, String)> = media_res
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref().ok())
        .map(|p| {
            p.results
                .iter()
                .map(|m| (m.id.clone(), (m.name.clone(), m.kind.clone())))
                .collect()
        })
        .unwrap_or_default();

    // Tours de l'org (noms de la liste + picker du modal) — école media_map.
    let tours_res = use_resource(move || {
        let _ = sets_reload();
        async move {
            api::tours::list(&api::tours::TourFilters {
                limit: Some(200),
                ..Default::default()
            })
            .await
        }
    });
    let tour_map: std::collections::HashMap<String, String> = tours_res
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref().ok())
        .map(|p| {
            p.results
                .iter()
                .map(|t| (t.id.clone(), t.name.clone()))
                .collect()
        })
        .unwrap_or_default();

    let sets_res = use_resource(move || {
        let _ = sets_reload();
        let s = search.cloned();
        let media = selected_media_filter.cloned();
        async move {
            api::annotation_layers::list(&api::annotation_layers::LayerFilters {
                search: if s.trim().is_empty() {
                    None
                } else {
                    Some(s.trim().to_string())
                },
                media: if media.trim().is_empty() {
                    None
                } else {
                    Some(media.trim().to_string())
                },
                limit: Some(100),
                ..Default::default()
            })
            .await
        }
    });
    let sets: Vec<pnex_core::AnnotationLayerSummary> = sets_res
        .value()
        .read()
        .as_ref()
        .and_then(|o| o.as_ref().ok())
        .map(|p| p.results.clone())
        .unwrap_or_default();

    // ─── Précalculs rendu ───
    let search_placeholder = t!("annot-page-search").to_string();
    let host_id = "pnex-annot-page-host".to_string();
    // Média courant de l'éditeur : fixe pour un ensemble-média (000027).
    let mut cur_media: Signal<Option<String>> = use_signal(|| None);

    // À l'ouverture d'un ensemble-média : fixer le média courant (les
    // ensembles-tour le mettent à jour à chaque navigation de scène).
    use_effect(move || {
        let _ = sets_reload.cloned();
        if let View::Editor(id) = view.cloned() {
            let found = sets_res
                .value()
                .read()
                .as_ref()
                .and_then(|o| o.as_ref().ok())
                .and_then(|p| p.results.iter().find(|s| s.id == id).cloned());
            if let Some(m) = found.and_then(|s| s.media_asset_id) {
                if cur_media.cloned().as_deref() != Some(m.as_str()) {
                    cur_media.set(Some(m));
                }
            }
        }
    });
    // État éditeur possédé par la PAGE : les marqueurs plats sont rendus
    // ici (les hotspots pannellum ne portent que l'équirect).
    let cx = AnnotationEditorCx::new();
    let sets_now = sets.clone();
    let sets_other = sets.clone();
    let media_filter_now = selected_media_filter.cloned();
    let open_set = match view.cloned() {
        View::Editor(id) => sets.iter().find(|s| s.id == id).cloned(),
        _ => None,
    };

    // Lecture synchrone de la ressource des ensembles (doctrine socle CRUD)
    // — la liste historique n'avait AUCUN état (pending/erreur rendaient une
    // table vide) ; ListStates les introduit.
    let (list_state, is_empty) = match &*sets_res.value().read() {
        None => (None, false),
        Some(Ok(paged)) => (Some(Ok(())), paged.results.is_empty()),
        Some(Err(err)) => (Some(Err(err.clone())), false),
    };

    // Colonnes de la table — la ligne entière est cliquable (on_row_click,
    // extension socle motivée par cette page) ; le bouton delete fait
    // stop_propagation pour ne pas déclencher l'ouverture.
    let columns = vec![
        Column::new(
            t!("annot-page-col-name").to_string(),
            |s: &pnex_core::AnnotationLayerSummary| {
                rsx! { {s.name.clone()} }
            },
        )
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("annot-page-col-media").to_string(), {
            let media_map = media_map.clone();
            let tour_map = tour_map.clone();
            move |s: &pnex_core::AnnotationLayerSummary| {
                let attached_label = match s.media_asset_id.as_ref().and_then(|m| media_map.get(m))
                {
                    Some((n, _)) => n.clone(),
                    None => match s.tour_id.as_ref().and_then(|t| tour_map.get(t)) {
                        Some(n) => format!("{n} ({})", t!("annot-page-tour-badge")),
                        None => t!("annot-page-missing-media").to_string(),
                    },
                };
                rsx! { {attached_label} }
            }
        })
        .with_td_class("text-gray-600"),
        Column::new(
            t!("annot-page-col-status").to_string(),
            |s: &pnex_core::AnnotationLayerSummary| {
                rsx! {
                    if s.published_version_number.is_some() {
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                            {t!("annot-page-published")}
                        }
                    } else {
                        span { class: "text-gray-400 text-xs", {t!("annot-page-draft")} }
                    }
                }
            },
        ),
        Column::new(
            String::new(),
            move |s: &pnex_core::AnnotationLayerSummary| {
                let s_delete = s.clone();
                // Tour-scoped sets are edited in the tour editor (Studio) —
                // offer the jump; media sets keep the inline editor.
                let studio_tour = s.tour_id.clone().unwrap_or_default();
                let nav_col = navigator.clone();
                rsx! {
                    div { class: "inline-flex items-center gap-1.5",
                        if !studio_tour.is_empty() {
                            button {
                                class: "px-3 py-1 text-sm bg-blue-100 text-blue-700 rounded-lg hover:bg-blue-200 transition-colors",
                                onclick: move |e| {
                                    e.stop_propagation();
                                    OPEN_TOUR.with_mut(|v| *v = Some(studio_tour.clone()));
                                    nav_col.push(Route::Studio {});
                                },
                                {t!("annot-open-in-studio")}
                            }
                        }
                        button {
                            class: DANGER_BTN,
                            onclick: move |e| {
                                e.stop_propagation();
                                delete_target.set(Some(s_delete.clone()));
                            },
                            icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                            {t!("annot-inspector-delete")}
                        }
                    }
                }
            },
        )
        .with_td_class("text-right"),
    ];

    rsx! {
        {match view.cloned() {
        View::List => rsx! {
            ListLayout {
                title: t!("annot-page-title").to_string(),
                subtitle: Some(t!("annot-page-subtitle").to_string()),
                can_write: can_write,
                add_label: Some(t!("annot-page-new").to_string()),
                on_add: move |_| new_open.set(true),
                // Filtre : recherche + médias (recherche live, canon socle).
                FilterBar {
                    SearchInput {
                        placeholder: search_placeholder,
                        value: search,
                        on_submit: move |_| {},
                    }
                    RefreshButton {
                        on_click: move |_| sets_reload.with_mut(|r| *r += 1),
                    }
                    select {
                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        value: "{media_filter_now}",
                        onchange: move |e| selected_media_filter.set(e.value()),
                        option { value: "", {t!("annot-page-all-medias")} }
                        for (m_id, (m_name, _)) in media_map.clone() {
                            option { key: "{m_id}", value: "{m_id}", {m_name} }
                        }
                    }
                }
                ListStates {
                    state: list_state,
                    is_empty: is_empty,
                    empty_message: t!("annot-page-no-sets").to_string(),
                    DataTable {
                        columns: columns,
                        rows: sets_now,
                        row_key: RowKey::new(|s: &pnex_core::AnnotationLayerSummary| s.id.clone()),
                        on_row_click: Callback::new(move |id: String| view.set(View::Editor(id))),
                    }
                }
            }
        },
        View::Editor(_open_id) => rsx! {
            div { class: "p-6 space-y-3",
                // Barre : retour + nom + autres ensembles du même média
                div { class: "flex items-center gap-3 flex-wrap",
                    button {
                        class: "px-3 py-1.5 text-xs text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| view.set(View::List),
                        {t!("annot-page-back")}
                    }
                    {open_set.as_ref().map(|s| rsx! {
                        span { class: "text-lg font-semibold text-gray-900", {s.name.clone()} }
                    })}
                }
                // Other sets on the same media (avoid duplicates) — tour
                // sets are edited in the Studio and are not listed here.
                if let Some(cur) = open_set.clone() {
                    {
                        let others: Vec<pnex_core::AnnotationLayerSummary> = sets_other
                            .iter()
                            .filter(|s| {
                                let same_media = cur
                                    .media_asset_id
                                    .as_deref()
                                    .zip(s.media_asset_id.as_deref())
                                    .is_some_and(|(a, b)| a == b);
                                same_media && s.id != cur.id
                            })
                            .cloned()
                            .collect();
                            rsx! {
                                if !others.is_empty() {
                                    div { class: "flex items-center gap-2 flex-wrap text-xs",
                                        span { class: "text-gray-500", {t!("annot-page-other-sets")} }
                                        for o in others {
                                            button {
                                                key: "other-{o.id}",
                                                class: "px-2 py-1 rounded-lg bg-gray-100 text-gray-700 hover:bg-gray-200",
                                                onclick: move |_| view.set(View::Editor(o.id.clone())),
                                                {o.name.clone()}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                match (
                    open_set.clone(),
                    open_set.clone().and_then(|s| s.media_asset_id),
                    open_set.clone().and_then(|s| s.tour_id),
                ) {
                    (Some(set), Some(media), _) => {
                        let kind = media_map
                            .get(&media)
                            .map(|(_, k)| k.clone())
                            .unwrap_or_else(|| "panorama".to_string());
                        let is_pano = kind == "panorama";
                        rsx! {
                        div { class: "flex gap-4 items-start",
                            if is_pano {
                                TourViewer {
                                    key: "viewer-{set.id}",
                                    doc: synthetic_doc(&media),
                                    assets: Default::default(),
                                    source: ViewerSource::Auth,
                                    on_hotspot_move: move |_: (String, f64, f64)| {},
                                    on_scene_change: move |_: String| {},
                                    host_id: host_id.clone(),
                                    compact: false,
                                    show_side_panel: false,
                                    annotations_enabled: false,
                                    // Editor page: annotation markers go
                                    // through the panel writer (editable host),
                                    // nav arrows stay fixed anyway.
                                    editable: false,
                                }
                            } else {
                                FlatAnnotViewer {
                                    key: "flat-{set.id}",
                                    media_id: media.clone(),
                                    cx,
                                    set_name: set.name.clone(),
                                }
                            }
                            AnnotationLayerPanel {
                                key: "panel-{set.id}",
                                layer_id: set.id.clone(),
                                media_override: cur_media,
                                host_id: host_id.clone(),
                                can_write,
                                cx,
                                media_kind: kind.clone(),
                            }
                        }
                        }
                    },
                    (Some(set), None, Some(tour_id)) => {
                        // Tour-scoped set (000028): read-only inventory here —
                        // editing lives in the tour editor (Studio). Orphaned
                        // tours (FK SET NULL after tour deletion) never reach
                        // this arm: tour_id would be None.
                        let tour_label = tour_map
                            .get(&tour_id)
                            .map(|n| format!("{n} ({})", t!("annot-page-tour-badge")))
                            .unwrap_or_else(|| t!("annot-page-missing-media").to_string());
                        let set_for_delete = set.clone();
                        let studio_tour = set.tour_id.clone().unwrap_or_default();
                        let nav_open = navigator.clone();
                        rsx! {
                        div { class: "bg-white rounded-xl border border-gray-200 p-6 max-w-2xl space-y-3",
                            div { class: "flex items-center gap-2 flex-wrap",
                                span { class: "text-base font-semibold text-gray-900",
                                    {set.name.clone()}
                                }
                                if set.published_version_number.is_some() {
                                    span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-100 text-green-800",
                                        {t!("annot-page-published")}
                                    }
                                } else {
                                    span { class: "text-gray-400 text-xs", {t!("annot-page-draft")} }
                                }
                            }
                            p { class: "text-sm text-gray-600",
                                {format!("{} : {tour_label}", t!("annot-page-col-media"))}
                            }
                            p { class: "text-sm text-gray-500", {t!("annot-tour-readonly-hint")} }
                            div { class: "flex gap-2 pt-1",
                                button {
                                    class: "px-3 py-1.5 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                                    disabled: studio_tour.is_empty(),
                                    onclick: move |_| {
                                        OPEN_TOUR.with_mut(|v| *v = Some(studio_tour.clone()));
                                        nav_open.push(Route::Studio {});
                                    },
                                    {t!("annot-open-in-studio")}
                                }
                                button {
                                    class: DANGER_BTN,
                                    onclick: move |_| {
                                        delete_target.set(Some(set_for_delete.clone()));
                                    },
                                    icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                    {t!("annot-inspector-delete")}
                                }
                            }
                        }
                        }
                    },
                    (Some(set), None, None) => rsx! {
                        div { class: "bg-white rounded-xl border border-gray-200 p-8 text-center",
                            p { class: "text-sm text-gray-400", {t!("annot-page-missing-media-body")} }
                            span { {set.name.clone()} }
                        }
                    },
                    _ => rsx! {},
                }
            }
        },
        }}
        // ─── Modal : nouvel ensemble (nom + média associé) ───
        if new_open() {
            NewSetModal {
                key: "new-set-{sets_reload.cloned()}",
                media_map: media_map.clone(),
                on_created: move |id: String| {
                    new_open.set(false);
                    sets_reload.with_mut(|r| *r += 1);
                    view.set(View::Editor(id));
                },
                on_close: move |_| new_open.set(false),
            }
        }
        // ─── Confirmation : suppression d'un ensemble ───
        if let Some(s) = delete_target.cloned() {
            ConfirmDialog {
                key: "del-{s.id}",
                title: t!("annot-page-delete-confirm-title"),
                message: t!("annot-page-delete-confirm-message"),
                confirm_label: t!("annot-inspector-delete"),
                on_confirm: move |_| {
                    delete_target.set(None);
                    let id = s.id.clone();
                    spawn(async move {
                        match api::annotation_layers::delete(&id).await {
                            Ok(_) => {
                                toasts::success(t!("toast-annot-deleted"));
                                sets_reload.with_mut(|r| *r += 1);
                            }
                            Err(err) => toasts::error(err),
                        }
                    });
                },
                on_cancel: move |_| delete_target.set(None),
            }
        }
    }
}

/// Modal de création : nom global + média associé (pivot UX — un ensemble
/// déclare son média dès la création).
#[component]
fn NewSetModal(
    media_map: std::collections::HashMap<String, (String, String)>,
    on_created: Callback<String>,
    on_close: Callback<()>,
) -> Element {
    let mut name = use_signal(String::new);
    let mut choice = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let name_placeholder = t!("annot-page-name-placeholder").to_string();
    let media_hint = t!("annot-page-new-media-hint").to_string();

    let media_map_render = media_map.clone();
    // Choice preview (name + readable kind) — `media:` prefixed value
    // (UUIDs are indistinguishable otherwise).
    let raw_choice = choice.cloned();
    let chosen: Option<(String, String)> = if let Some(id) = raw_choice.strip_prefix("media:") {
        media_map.get(id).map(|(n, k)| {
            let kind_label = if k == "panorama" {
                t!("annot-page-kind-pano").to_string()
            } else {
                t!("annot-page-kind-photo").to_string()
            };
            (n.clone(), kind_label)
        })
    } else {
        None
    };

    rsx! {
        Modal {
            title: t!("annot-page-new"),
            max_width: "max-w-lg".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "space-y-5",
                // Nom de l'ensemble
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1.5",
                        {t!("annot-page-col-name")}
                    }
                    input {
                        class: "w-full rounded-lg border-gray-300 text-sm px-3 py-2 focus:ring-2 focus:ring-blue-500 focus:border-blue-500",
                        placeholder: "{name_placeholder}",
                        value: "{name()}",
                        onchange: move |e| name.set(e.value()),
                    }
                    p { class: "mt-1.5 text-xs text-gray-400",
                        {t!("annot-page-new-name-hint")}
                    }
                }
                // Média associé
                div {
                    label { class: "block text-sm font-medium text-gray-700 mb-1.5",
                        {t!("annot-page-col-media")}
                    }
                    select {
                        class: "w-full rounded-lg border-gray-300 text-sm px-3 py-2 bg-white focus:ring-2 focus:ring-blue-500 focus:border-blue-500",
                        value: "{choice()}",
                        onchange: move |e| choice.set(e.value()),
                        option { value: "", {t!("annot-page-media-select")} }
                        optgroup { label: t!("annot-page-media-optgroup"),
                            for (m_id, (m_name, m_kind)) in media_map_render {
                                option { key: "{m_id}", value: "media:{m_id}",
                                    "{m_name} ({m_kind})"
                                }
                            }
                        }
                    }
                    if let Some((n, k)) = chosen {
                        div { class: "mt-2 flex items-center gap-2 text-xs bg-blue-50 border border-blue-100 text-blue-800 rounded-lg px-3 py-2",
                            span { class: "inline-block h-2.5 w-2.5 rounded-full bg-blue-500" }
                            span { class: "font-medium truncate", "{n}" }
                            span { class: "text-blue-400 ml-auto", "{k}" }
                        }
                    }
                    p { class: "mt-1.5 text-xs text-gray-400", "{media_hint}" }
                }
                // Actions
                div { class: "pt-2 border-t border-gray-100 flex justify-end gap-2",
                    button {
                        class: "px-4 py-2 text-sm text-gray-700 bg-white border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                        onclick: move |_| on_close.call(()),
                        {t!("annot-page-cancel")}
                    }
                    button {
                        class: "px-4 py-2 text-sm bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                        disabled: creating() || name.read().trim().is_empty()
                            || choice.read().trim().is_empty(),
                        onclick: move |_| {
                            let name = name.read().trim().to_string();
                            let raw = choice.read().trim().to_string();
                            // Media sets only — tour sets are created by the
                            // tour editor (Studio, find-or-create).
                            let media = raw.strip_prefix("media:").map(|id| id.to_string());
                            creating.set(true);
                            spawn(async move {
                                match api::annotation_layers::create(pnex_core::CreateAnnotationLayer {
                                    name,
                                    media_asset_id: media,
                                    tour_id: None,
                                    description: None,
                                    author: None,
                                    note: None,
                                })
                                .await
                                {
                                    Ok(d) => {
                                        on_created.call(d.id);
                                    }
                                    Err(err) => toasts::error(err),
                                }
                                creating.set(false);
                            });
                        },
                        {if creating() { t!("annot-page-creating").to_string() } else { t!("annot-layer-create").to_string() }}
                    }
                }
            }
        }
    }
}

/// Visionneuse PLATE d'un média annoté (photo/floorplan) : `<img>` blob,
/// marqueurs en % du conteneur (école MediaPreview) et pose au clic
/// (x/y ∈ [0,1]). Les marqueurs vivent dans le DOM dioxus — les hotspots
/// pannellum ne portent que la géométrie sphère.
#[component]
fn FlatAnnotViewer(media_id: String, cx: AnnotationEditorCx, set_name: String) -> Element {
    // Blob de la version courante (école MediaPreview — le mime n'est pas
    // requis : l'URL blob web est auto-typée, la data-URI native passe).
    let mut blob_url = use_signal(|| None::<String>);
    let id_for_blob = media_id.clone();
    use_effect(move || {
        let id = id_for_blob.clone();
        spawn(async move {
            blob_url.set(None);
            let url = media_blob_url(&api::media::content_path(&id), None).await;
            blob_url.set(url);
        });
    });

    // Rect de l'img pour convertir client px → fraction (mesure post-mount
    // par polling court — le DOM n'est pas encore mesurable au 1er tick).
    // Pas de break : le rect reste FRAIS (resize, shift de layout) — pose
    // au clic et drag de repositionnement toujours exacts.
    let mut img_rect = use_signal(|| None::<(f64, f64, f64, f64)>);
    let img_id = format!("annot-flat-img-{}", media_id.replace('-', ""));
    let img_id_for_rect = img_id.clone();
    use_effect(move || {
        let img_id = img_id_for_rect.clone();
        spawn(async move {
            loop {
                crate::util::sleep(std::time::Duration::from_millis(250)).await;
                let rect = crate::util::element_rect(&img_id);
                if img_rect.cloned() != rect {
                    img_rect.set(rect);
                }
            }
        });
    });

    // ─── Drag de repositionnement des marqueurs ───
    // pointerdown sur le marqueur → suivi move/up/leave au conteneur
    // (école dashboard_editor : la surface porte les events de suivi).
    // Seuil 4 px (école viewers.js) : < seuil = simple clic → sélection.
    let mut drag_item = use_signal(|| None::<String>);
    let mut drag_start = use_signal(|| None::<(f64, f64)>);
    let mut drag_moved = use_signal(|| false);

    // ─── Précalculs ───
    let doc_now = cx.doc.cloned();
    let media_for_rows = media_id.clone();
    let markers: Vec<(String, f64, f64, String, String)> =
        item_rows_flat(&doc_now, &media_for_rows)
            .into_iter()
            .map(|f| (f.id, f.x, f.y, f.kind, f.label))
            .collect();
    let placing_now = cx.placing.cloned();
    let _ = set_name;

    rsx! {
        div { class: "flex-1 min-w-0",
            div { class: "relative bg-gray-900 rounded-lg overflow-hidden flex items-center justify-center h-[70vh] p-3",
                match blob_url.cloned() {
                    Some(url) => rsx! {
                        div {
                            class: "relative inline-flex",
                            // ─── Suivi du drag au conteneur ───
                            onpointermove: move |e: PointerEvent| {
                                let Some(item) = drag_item.cloned() else {
                                    return;
                                };
                                let Some((sx, sy)) = drag_start.cloned() else {
                                    return;
                                };
                                let c = e.data().client_coordinates();
                                if !drag_moved.cloned()
                                    && (c.x - sx).abs() + (c.y - sy).abs() < 4.0
                                {
                                    return;
                                }
                                drag_moved.set(true);
                                let Some((l, t, w, h)) = img_rect.cloned() else {
                                    return;
                                };
                                if w <= 0.0 || h <= 0.0 {
                                    return;
                                }
                                let x = ((c.x - l) / w).clamp(0.0, 1.0);
                                let y = ((c.y - t) / h).clamp(0.0, 1.0);
                                cx.update_doc(move |doc| {
                                    move_item_flat(doc, &item, x, y);
                                });
                            },
                            onpointerup: move |_| {
                                let item = drag_item.cloned();
                                drag_item.set(None);
                                drag_start.set(None);
                                // Sélection après clic OU fin de drag —
                                // l'inspecteur s'ouvre dans les deux cas.
                                if let Some(id) = item {
                                    cx.selected.set(Some(id));
                                }
                                drag_moved.set(false);
                            },
                            onpointerleave: move |_| {
                                // Sortie du conteneur pendant un drag :
                                // le marqueur reste à sa dernière position
                                // valide, l'état est réinitialisé.
                                drag_item.set(None);
                                drag_start.set(None);
                                drag_moved.set(false);
                            },
                            img {
                                id: "{img_id}",
                                src: "{url}",
                                class: "max-h-[calc(70vh-24px)] max-w-full object-contain select-none",
                                draggable: "false",
                                onclick: move |e: Event<MouseData>| {
                                    if !cx.placing.cloned() {
                                        return;
                                    }
                                    let Some((l, t, w, h)) = img_rect.cloned() else {
                                        return;
                                    };
                                    if w <= 0.0 || h <= 0.0 {
                                        return;
                                    }
                                    let px = e.data.client_coordinates().x;
                                    let py = e.data.client_coordinates().y;
                                    let x = ((px - l) / w).clamp(0.0, 1.0);
                                    let y = ((py - t) / h).clamp(0.0, 1.0);
                                    let media = media_for_rows.clone();
                                    cx.update_doc(move |doc| {
                                        let id = place_item_flat(doc, &media, x, y);
                                        cx.selected.set(Some(id));
                                    });
                                },
                            }
                            for (m_id, m_x, m_y, m_kind, m_label) in markers {
                                button {
                                    key: "flat-{m_id}",
                                    class: "absolute h-5 w-5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow-md hover:scale-110 transition-transform touch-none {crate::components::annotation_editor::popover::annot_dot_color(&m_kind)}",
                                    style: "left: calc({m_x} * 100%); top: calc({m_y} * 100%);",
                                    title: "{m_label}",
                                    // pointerdown ONLY : le suivi (move/up)
                                    // vit au conteneur — < seuil = clic
                                    // simple → sélection via onpointerup.
                                    onpointerdown: move |e: PointerEvent| {
                                        e.stop_propagation();
                                        drag_item.set(Some(m_id.clone()));
                                        let c = e.data().client_coordinates();
                                        drag_start.set(Some((c.x, c.y)));
                                        drag_moved.set(false);
                                    },
                                }
                            }
                            if placing_now {
                                div { class: "absolute top-2 left-2 px-2 py-1 text-xs rounded-lg bg-blue-600/90 text-white pointer-events-none",
                                    {t!("annot-place-hint")}
                                }
                            }
                        }
                    },
                    None => rsx! {
                        span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-white" }
                    },
                }
            }
        }
    }
}
