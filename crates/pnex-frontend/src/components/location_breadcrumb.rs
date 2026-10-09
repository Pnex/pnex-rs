//! "Location" breadcrumb of a resource in the sites tree: Site › folder ›
//! folder. Each step opens the Sites page on that site, so a detail page
//! and the tree link both ways.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
use crate::components::icons;
use crate::state::map::{OPEN_POI, OPEN_POI_FOLDERS};

#[component]
pub fn LocationBreadcrumb(
    kind: String,
    id: String,
    /// Toolbar form: renders nothing while the resource is not placed.
    #[props(default)]
    compact: bool,
) -> Element {
    let locations = use_resource(move || {
        let (kind, id) = (kind.clone(), id.clone());
        async move { api::resources::location(&kind, &id).await.ok() }
    });
    let Some(Some(found)) = locations.read().clone() else {
        return rsx! {};
    };
    if found.locations.is_empty() {
        if compact {
            return rsx! {};
        }
        return rsx! {
            p { class: "flex items-center gap-1.5 text-xs text-gray-500",
                icons::MapPin { class: "h-3.5 w-3.5 shrink-0" }
                {t!("location-none")}
                Link {
                    to: Route::Sites {},
                    class: "text-blue-600 hover:underline",
                    {t!("location-open-sites")}
                }
            }
        };
    }
    let text_class = if compact {
        "flex flex-wrap items-center gap-1 text-xs text-gray-600"
    } else {
        "flex flex-wrap items-center gap-1 text-sm text-gray-700"
    };
    // Owned view rows: site step text + each folder with the path to unfold.
    let rows: Vec<(String, String, Vec<(String, String, Vec<String>)>)> = found
        .locations
        .iter()
        .map(|loc| {
            let site_text = match &loc.site.emoji {
                Some(emoji) => format!("{emoji} {}", loc.site.name),
                None => loc.site.name.clone(),
            };
            let folders = loc
                .folders
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    let path = loc.folders[..=i].iter().map(|n| n.id.clone()).collect();
                    (f.id.clone(), f.name.clone(), path)
                })
                .collect();
            (loc.site.id.clone(), site_text, folders)
        })
        .collect();
    rsx! {
        div { class: "space-y-1",
            for (site_id, site_text, folders) in rows {
                nav {
                    key: "{site_id}",
                    class: "{text_class}",
                    aria_label: t!("location-label"),
                    icons::MapPin { class: "h-3.5 w-3.5 shrink-0 text-gray-400" }
                    SiteStep {
                        site_id: site_id.clone(),
                        text: site_text,
                        strong: true,
                        unfold: Vec::new(),
                    }
                    for (folder_id, name, path) in folders {
                        FolderStep {
                            key: "{folder_id}",
                            site_id: site_id.clone(),
                            text: name,
                            unfold: path,
                        }
                    }
                }
            }
        }
    }
}

/// "› folder" step: opens the folder's site with the folder unfolded.
#[component]
fn FolderStep(site_id: String, text: String, unfold: Vec<String>) -> Element {
    rsx! {
        span { class: "text-gray-400", "›" }
        SiteStep {
            site_id,
            text,
            strong: false,
            unfold,
        }
    }
}

/// One clickable step: opens the Sites page on the step's site.
#[component]
fn SiteStep(site_id: String, text: String, strong: bool, unfold: Vec<String>) -> Element {
    let navigator = use_navigator();
    rsx! {
        button {
            r#type: "button",
            class: if strong { "font-medium text-blue-700 hover:underline" } else { "hover:underline" },
            onclick: move |_| {
                *OPEN_POI.write() = Some(site_id.clone());
                *OPEN_POI_FOLDERS.write() = unfold.clone();
                navigator.push(Route::Sites {});
            },
            "{text}"
        }
    }
}
