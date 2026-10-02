//! Page publique `/share/:token` — consultation d'un tour **publié** sans
//! login. Route **hors layout Shell** (la Shell porte la garde de session —
//! précédent `AuthCallback`). Le fetch public passe par `api::tours::
//! public_detail` (chemin `/api/v1/public/` exempté d'auth côté client).
//!
//! ⚠ dioxus #2784 : les props de route ne sont pas réactives — le token est
//! lu **une fois** (cloné dans les closures/resource).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::PublicTour;

use crate::components::tour_viewer::{AssetInfo, TourViewer, ViewerSource};

#[component]
pub fn ShareTour(token: String) -> Element {
    // Lu une fois : props de route ≠ signaux (dioxus #2784).
    let token = use_signal(|| token);

    let fetch = use_resource(move || {
        let token = token.cloned();
        async move { crate::api::tours::public_detail(&token).await }
    });

    rsx! {
        div { class: "min-h-screen bg-gray-100 flex flex-col",
            // En-tête minimal (pas de nav authentifiée ici).
            header { class: "bg-white shadow-sm px-6 py-4 flex items-center gap-3",
                span { class: "text-base font-semibold text-gray-900", {t!("share-title")} }
                match &*fetch.value().read() {
                    Some(Ok(tour)) => rsx! {
                        span { class: "text-sm text-gray-500", {tour.name.clone()} }
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] font-medium bg-green-50 text-green-700 border border-green-200",
                            {t!("share-published-tag", version : tour.published_version_number)}
                        }
                    },
                    _ => rsx! {},
                }
            }
            main { class: "flex-1 p-4",
                match &*fetch.value().read() {
                    Some(Ok(tour)) => rsx! {
                        // Carte des assets : version + mime servis par le doc
                        // public (cache immutable côté octets).
                        TourViewer {
                            doc: tour.doc.clone(),
                            assets: asset_infos(tour),
                            source: ViewerSource::Public {
                                token: token.cloned(),
                            },
                            // Page publique en lecture seule : le drag d'une
                            // flèche reste visuel (rien à persister).
                            on_hotspot_move: move |_: (String, f64, f64)| {},
                            on_scene_change: move |_: String| {},
                            host_id: crate::components::tour_viewer::HOST_ID.to_string(),
                            editable: false,
                            compact: false,
                            // D58 : annotations EXCLUES du share public V1 —
                            // elles peuvent révéler des données device.
                            annotations_enabled: false,
                            show_side_panel: true,
                        }
                    },
                    Some(Err(err)) if err.status == Some(404) => rsx! {
                        div { class: "max-w-md mx-auto mt-16 bg-white rounded-lg shadow-sm p-8 text-center",
                            p { class: "text-lg font-medium text-gray-900", {t!("share-invalid-title")} }
                            p { class: "text-sm text-gray-500 mt-2", {t!("share-invalid-message")} }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "max-w-md mx-auto mt-16 bg-red-50 border border-red-200 rounded-lg p-4 text-sm text-red-700",
                            {err.message.clone()}
                        }
                    },
                    None => rsx! {
                        div { class: "text-center py-16",
                            span { class: "animate-spin inline-block rounded-full h-8 w-8 border-b-2 border-blue-600" }
                        }
                    },
                }
            }
        }
    }
}

/// Viewer asset map (version + mime) from the public tour document.
fn asset_infos(tour: &PublicTour) -> HashMap<String, AssetInfo> {
    tour.assets
        .iter()
        .map(|(id, reference)| {
            let info = AssetInfo {
                version: Some(reference.version_number),
                content_type: Some(reference.content_type.clone()),
            };
            (id.clone(), info)
        })
        .collect()
}
