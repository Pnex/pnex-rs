//! Panneau d'aperçu READ-ONLY des objets attachés à un POI (/map) — le
//! panneau **couvre la zone carte** (la carte reste montée dessous), il se
//! remplace d'asset en asset quand on clique les lignes de l'arbre du
//! drawer. Jamais de popup ni de navigation pour CONSULTER ; l'édition
//! reste dans l'onglet de chaque app (bouton « Éditer » du header).
//!
//! Épingle (★) : le POI porte `preview_kind`/`preview_id` (migration
//! 000020) — premier objet attaché = épinglé par défaut, changement/retrait
//! via l'étoile des lignes de l'arbre. La logique PATCH vit dans
//! `pages/map.rs` ; ce module n'affiche que.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::api::media::MediaKind;
use crate::components::annotated_media::AnnotatedMediaView;
use crate::components::dashboard_live::DashboardLive;
use crate::components::download_progress::MediaDownloadProgress;
use crate::components::icons;
use crate::components::poi_tree::KindIcon;
use crate::components::tour_viewer::{TourViewer, ViewerSource};
use crate::util::media_blob_url_tracked;

/// Cible d'aperçu : (kind, id, nom) d'un objet attaché au POI.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewTarget {
    pub kind: String,
    pub id: String,
    pub name: String,
}

/// Panneau d'aperçu plein-cadre sur la carte.
#[component]
pub fn PoiPreviewPanel(
    target: PreviewTarget,
    on_close: EventHandler<()>,
    on_edit: EventHandler<()>,
) -> Element {
    let kind_label = match target.kind.as_str() {
        "media_asset" => t!("poi-picker-tab-media").to_string(),
        "tour" => t!("poi-picker-tab-tour").to_string(),
        "dashboard" => t!("poi-picker-tab-dashboard").to_string(),
        _ => target.kind.clone(),
    };

    rsx! {
        div { class: "flex h-full w-full flex-col",
            // Header : icône kind + nom + badge kind + actions.
            div { class: "flex items-center gap-2 px-3 py-2 border-b border-gray-200 shrink-0 sm:px-4",
                KindIcon { kind: target.kind.clone() }
                span { class: "min-w-0 text-sm font-semibold text-gray-900 truncate",
                    "{target.name}"
                }
                span { class: "hidden text-xs uppercase tracking-wide text-gray-400 shrink-0 sm:inline",
                    "{kind_label}"
                }
                div { class: "flex-1" }
                button {
                    class: "inline-flex items-center px-2.5 py-1.5 text-xs font-medium text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50 shrink-0",
                    title: t!("poi-preview-edit"),
                    aria_label: t!("poi-preview-edit"),
                    onclick: move |_| on_edit.call(()),
                    icons::Wrench { class: "h-3.5 w-3.5 sm:mr-1.5" }
                    span { class: "hidden sm:inline", {t!("poi-preview-edit")} }
                }
                button {
                    class: "p-1.5 text-gray-400 hover:text-gray-600 rounded-lg hover:bg-gray-100 shrink-0",
                    onclick: move |_| on_close.call(()),
                    title: t!("poi-preview-close").to_string(),
                    aria_label: t!("poi-preview-close"),
                    icons::X { class: "h-4 w-4" }
                }
            }
            // Corps : dispatch par kind (read-only, aucun handler d'écriture).
            div { class: "flex-1 min-h-0 flex flex-col overflow-hidden",
                {
                    match target.kind.as_str() {
                        "media_asset" => rsx! {
                            MediaPreviewBody {
                                key: "{target.id}",
                                asset_id: target.id.clone(),
                                asset_name: target.name.clone(),
                            }
                        },
                        "tour" => rsx! {
                            TourPreviewBody { key: "{target.id}", tour_id: target.id.clone() }
                        },
                        "dashboard" => rsx! {
                            div { class: "flex-1 min-h-0 overflow-auto p-4 bg-gray-50",
                                DashboardLive { dashboard_id: target.id.clone() }
                            }
                        },
                        _ => rsx! {
                            div { class: "flex-1 flex items-center justify-center",
                                p { class: "text-sm text-gray-400", "…" }
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Corps média : fetch `media::detail` puis rendu (image ou viewer JS).
#[component]
fn MediaPreviewBody(asset_id: String, asset_name: String) -> Element {
    let detail = use_resource(move || {
        let id = asset_id.clone();
        async move { api::media::detail(&id).await.ok() }
    });
    let loaded: Option<api::media::MediaAsset> = detail.read().as_ref().cloned().flatten();

    match loaded {
        None => rsx! {
            div { class: "flex-1 flex items-center justify-center",
                span { class: "animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" }
            }
        },
        // Annotatable kinds go through the shared annotated viewer: the
        // published annotations of the media show on the map preview too.
        Some(asset) if annotatable(&asset) => rsx! {
            AnnotatedMediaView {
                key: "{asset.id}",
                asset_id: asset.id.clone(),
                kind: asset.media_kind().as_str().to_string(),
                host_id: format!("pnex-preview-pano-{}", asset.id.replace('-', "")),
                compact: true,
            }
        },
        Some(asset) => rsx! {
            MediaAssetView { key: "{asset.id}", asset }
        },
    }
}

/// Media kinds that can carry annotations (D55, D147): panorama, photo,
/// floorplan, and splats the viewer reads (`.splat` / `.ply`).
fn annotatable(asset: &api::media::MediaAsset) -> bool {
    match asset.media_kind() {
        MediaKind::Panorama | MediaKind::Photo | MediaKind::Floorplan => true,
        MediaKind::Splat => asset
            .metadata
            .as_ref()
            .and_then(|m| m.get("format"))
            .and_then(|f| f.as_str())
            .is_none_or(|f| f == "splat" || f == "ply"),
        MediaKind::Model | MediaKind::Document | MediaKind::Table => false,
    }
}

/// Rendu d'un asset média en inline (porté de `MediaViewerModal`, sans la
/// chrome modale) : Photo/Floorplan → `<img>` blob ; Panorama/Splat →
/// viewer JS (`media_viewer::mount`, hauteur inline — piège pannellum) ;
/// fallback natif si le montage JS échoue.
#[component]
fn MediaAssetView(asset: api::media::MediaAsset) -> Element {
    let asset_kind = asset.media_kind();
    let asset_name = asset.name.clone();
    let is_image = matches!(asset_kind, MediaKind::Photo | MediaKind::Floorplan);

    let host_id = format!("pnex-preview-pano-{}", asset.id.replace('-', ""));
    let host_for_drop = host_id.clone();
    let host_for_effect = host_id.clone();
    let id_for_effect = asset.id.clone();
    let mime_for_effect = asset.content_type.clone();

    let mut mount_failed = use_signal(|| false);
    let mut ready = use_signal(|| false);
    let mut image_url = use_signal(|| None::<String>);
    let progress = use_signal(|| None::<crate::util::DownloadProgress>);
    let is_image_for_effect = is_image;
    let kind_for_effect = asset_kind.as_str();

    use_effect(move || {
        let id = id_for_effect.clone();
        let host = host_for_effect.clone();
        let mime = mime_for_effect.clone();
        spawn(async move {
            let path = api::media::content_path(&id);
            let url = media_blob_url_tracked(&path, mime.as_deref(), Some(progress)).await;
            match url {
                Some(url) if is_image_for_effect => image_url.set(Some(url)),
                Some(url) => {
                    let ok = crate::media_viewer::mount(kind_for_effect, &host, &url).await;
                    mount_failed.set(!ok);
                }
                None => mount_failed.set(true),
            }
            ready.set(true);
        });
    });
    use_drop(move || {
        crate::media_viewer::unmount(&host_for_drop);
    });

    rsx! {
        div { class: "relative flex-1 min-h-0 bg-gray-900 flex items-center justify-center",
            if is_image {
                if let Some(url) = image_url() {
                    img {
                        src: "{url}",
                        alt: "{asset_name}",
                        class: "max-w-full max-h-full object-contain",
                    }
                } else {
                    div { class: "w-full h-full" }
                }
            } else {
                div { id: "{host_id}", class: "absolute inset-0" }
            }
            MediaDownloadProgress { progress: progress() }
            if ready() && mount_failed() {
                div { class: "absolute inset-0 flex items-center justify-center pointer-events-none",
                    span { class: "text-sm text-gray-400", {t!("poi-media-unavailable")} }
                }
            }
        }
    }
}

/// Corps tour : fetch `tours::detail` → `TourViewer` compact (host unique
/// par tour — deux viewers pannellum ne peuvent pas partager un host DOM).
#[component]
fn TourPreviewBody(tour_id: String) -> Element {
    let tour_id_for_view = tour_id.clone();
    let detail = use_resource(move || {
        let id = tour_id.clone();
        async move { api::tours::detail(&id).await }
    });

    rsx! {
        div { class: "flex-1 min-h-0 flex flex-col p-3 overflow-hidden",
            // Read-only map preview: markers never draggable.
            {
                match &*detail.value().read() {
                    Some(Ok(d)) => rsx! {
                        TourViewer {
                            key: "tour-preview-{tour_id_for_view}",
                            doc: d.doc.clone(),
                            assets: Default::default(),
                            source: ViewerSource::Auth,
                            on_hotspot_move: move |_| {},
                            on_scene_change: move |_: String| {},
                            host_id: format!("pnex-preview-tour-{}", tour_id_for_view.replace('-', "")),
                            compact: true,
                            annotations_enabled: true,
                            annotation_tour: Some(tour_id_for_view.clone()),
                            show_side_panel: true,
                            editable: false,
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "flex-1 flex items-center justify-center",
                            div { class: "bg-red-50 border border-red-200 rounded-lg p-3 text-sm text-red-700",
                                {err.message.clone()}
                            }
                        }
                    },
                    None => rsx! {
                        div { class: "flex-1 flex items-center justify-center",
                            span { class: "animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" }
                        }
                    },
                }
            }
        }
    }
}
