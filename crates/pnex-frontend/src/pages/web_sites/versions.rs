//! Versions of a rule (D221), in the shared drawer.

use dioxus::prelude::*;
use pnex_core::site_recipe::{SiteRecipe, SiteRecipeVersion};

use crate::api;
use crate::api::error_i18n::localize;
use crate::components::versions_drawer::{self, VersionRow};

#[component]
pub(super) fn VersionsDrawer(
    recipe_id: String,
    active_version: i32,
    can_write: bool,
    on_close: EventHandler<()>,
    on_open: EventHandler<SiteRecipeVersion>,
    on_restored: EventHandler<SiteRecipe>,
) -> Element {
    let id = recipe_id.clone();
    let versions = use_resource(move || {
        let id = id.clone();
        async move { api::site_recipes::versions(&id).await }
    });
    let mut error = use_signal(|| None::<String>);
    let rows = versions.read().clone().map(|r| {
        r.map(|list| {
            list.into_iter()
                .map(|v| VersionRow {
                    version: v.version,
                    yaml: v.yaml,
                    created_at: v.created_at,
                    author: v.author,
                })
                .collect()
        })
    });
    rsx! {
        versions_drawer::VersionsDrawer {
            rows,
            active_version,
            can_write,
            error: error(),
            on_close,
            on_open: move |row: VersionRow| {
                let found = match &*versions.read() {
                    Some(Ok(list)) => list.iter().find(|v| v.version == row.version).cloned(),
                    _ => None,
                };
                if let Some(v) = found {
                    on_open.call(v);
                }
            },
            on_restore: move |version: i32| {
                let id = recipe_id.clone();
                spawn(async move {
                    match api::site_recipes::restore(&id, version).await {
                        Ok(r) => on_restored.call(r),
                        Err(e) => error.set(Some(localize(&e))),
                    }
                });
            },
        }
    }
}
