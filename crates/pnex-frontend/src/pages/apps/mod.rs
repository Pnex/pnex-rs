//! Applications of the org (app-builder.md D513): the launcher, a running
//! application and its builder. Declaring one stays on the Access page.

mod build;
mod run;

pub use build::AppBuild;
pub use run::AppRun;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::AppView;

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::BTN_SECONDARY_SM;

/// The screen an application opens on.
pub(crate) fn home_of(app: &AppView) -> Option<String> {
    app.home
        .clone()
        .filter(|h| {
            app.screens
                .iter()
                .any(|s| &s.key == h && s.published_version.is_some())
        })
        .or_else(|| {
            app.screens
                .iter()
                .find(|s| s.published_version.is_some())
                .map(|s| s.key.clone())
        })
}

#[component]
pub fn Apps() -> Element {
    let navigator = use_navigator();
    let mut reload = use_signal(|| 0u32);
    let apps = use_resource(move || {
        let _ = reload();
        async move { api::apps::list().await }
    });
    let can_build = crate::state::access::can("apps:build");
    let can_declare = crate::state::access::can("apps:write");
    let (state, rows) = match &*apps.value().read() {
        None => (None, Vec::new()),
        // Q-B3: every member sees what has a published screen; a builder also
        // sees what has none yet.
        Some(Ok(r)) => (
            Some(Ok(())),
            r.iter()
                .filter(|a| can_build || home_of(a).is_some())
                .cloned()
                .collect(),
        ),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let is_empty = matches!(&state, Some(Ok(()))) && rows.is_empty();
    let columns = vec![
        Column::new(t!("apps-col-app").to_string(), |a: &AppView| {
            rsx! {
                div { class: "text-sm font-medium text-ink", {crate::components::ontology::tr(&a.name, &a.names)} }
                if !a.description.is_empty() {
                    p { class: "text-xs text-slate", {a.description.clone()} }
                }
            }
        }),
        Column::new(t!("apps-col-screens").to_string(), |a: &AppView| {
            rsx! { "{a.screens.len()}" }
        }),
        Column::new(t!("common-actions").to_string(), move |a: &AppView| {
            rsx! {
                div { class: "flex justify-end gap-2",
                    if let Some(screen) = home_of(a) {
                        Link {
                            class: BTN_SECONDARY_SM,
                            "data-testid": "app-open",
                            to: Route::AppRun {
                                app: a.key.clone(),
                                screen,
                                query: String::new(),
                            },
                            {t!("apps-open")}
                        }
                    }
                    if can_build {
                        Link {
                            class: BTN_SECONDARY_SM,
                            "data-testid": "app-build",
                            to: Route::AppBuild {
                                app: a.key.clone(),
                            },
                            {t!("apps-build")}
                        }
                    }
                }
            }
        })
        .header_sr_only(),
    ];
    rsx! {
        ListLayout {
            title: t!("nav-apps").to_string(),
            subtitle: Some(t!("apps-subtitle").to_string()),
            add_label: t!("apps-declare").to_string(),
            can_write: can_declare,
            on_add: move |_| {
                navigator.push(Route::Access {});
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            ListStates {
                state,
                is_empty,
                empty_message: t!("apps-empty").to_string(),
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|a: &AppView| a.id.clone()),
                }
            }
        }
    }
}

/// The application of this key, for the routes that name it.
pub(crate) fn use_app_by_key(key: String) -> Resource<Option<AppView>> {
    use_resource(move || {
        let key = key.clone();
        async move {
            api::apps::list()
                .await
                .ok()?
                .into_iter()
                .find(|a| a.key == key)
        }
    })
}
