//! Public `/share/:token` page — viewing a **published** tour without
//! login. Route **outside the Shell layout** (the Shell carries the session
//! guard — `AuthCallback` precedent). The public fetch goes through `api::tours::
//! public_detail` (`/api/v1/public/` path exempt from auth on the client side).
//!
//! ⚠ dioxus #2784: route props are not reactive — the token is
//! read **once** (cloned into the closures/resource).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::PublicTour;

use crate::components::gerbe::Gerbe;
use crate::components::tour_viewer::{AssetInfo, TourViewer, ViewerSource};

#[component]
pub fn ShareTour(token: String) -> Element {
    // Read once: route props ≠ signals (dioxus #2784).
    let token = use_signal(|| token);

    let fetch = use_resource(move || {
        let token = token.cloned();
        async move { crate::api::tours::public_detail(&token).await }
    });

    rsx! {
        div { class: "min-h-screen bg-gray-100 flex flex-col",
            // Minimal header (no authenticated nav here).
            header { class: "bg-white border border-line px-6 py-4 flex items-center gap-3",
                span { class: "text-base font-semibold text-gray-900", {t!("share-title")} }
                match &*fetch.value().read() {
                    Some(Ok(tour)) => rsx! {
                        span { class: "text-sm text-gray-500", {tour.name.clone()} }
                        span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-green-50 text-green-700 border border-green-200",
                            {t!("share-published-tag", version : tour.published_version_number)}
                        }
                    },
                    _ => rsx! {},
                }
            }
            main { class: "flex-1 p-4",
                match &*fetch.value().read() {
                    Some(Ok(tour)) => rsx! {
                        // Asset map: version + mime served by the public
                        // doc (immutable cache on the bytes side).
                        TourViewer {
                            doc: tour.doc.clone(),
                            assets: asset_infos(tour),
                            source: ViewerSource::Public {
                                token: token.cloned(),
                            },
                            // Read-only public page: dragging an
                            // arrow stays visual (nothing to persist).
                            on_hotspot_move: move |_: (String, f64, f64)| {},
                            on_scene_change: move |_: String| {},
                            host_id: crate::components::tour_viewer::HOST_ID.to_string(),
                            editable: false,
                            compact: false,
                            // D58: annotations EXCLUDED from the public share V1 —
                            // they may reveal device data.
                            annotations_enabled: false,
                            show_side_panel: true,
                        }
                    },
                    Some(Err(err)) if err.status == Some(404) => rsx! {
                        div { class: "max-w-md mx-auto mt-16 bg-white rounded-lg border border-line p-8 text-center",
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
                            Gerbe { size: 36 }
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
