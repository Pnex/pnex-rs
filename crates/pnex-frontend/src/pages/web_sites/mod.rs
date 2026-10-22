//! Web sites and their navigation studio (site-recipes.md D323–D326).
//!
//! `/web-sites` lists the sites carrying a rule. `/web-sites/:site` is the
//! studio of one site, in the shared editor shell (D222): the rule as
//! blocks or YAML, Play on the server browser with the shared player
//! (D220), the run journal and the rule's versions (D221).

mod blocks;
mod player;
mod studio;
mod versions;

pub use studio::WebSiteStudio;

/// A rule the assistant proposed, waiting for the studio of its site to
/// open it as an unsaved draft (D144: the human saves it).
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedRule {
    pub site: String,
    pub recipe: pnex_core::site_recipe::Recipe,
    pub yaml: String,
    /// Saved rule the draft is a new version of.
    pub recipe_id: Option<String>,
}

pub static PROPOSED: GlobalSignal<Option<ProposedRule>> = GlobalSignal::new(|| None);

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{site_host, RunStatus, WebSite};

use crate::api;
use crate::api::error_i18n::resolve;
use crate::app::Route;
use crate::components::crud::filters::{FilterBar, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{page_slice, ListPager};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ontology::short_time;
use crate::components::ui::{FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};
use crate::state::access::can;

pub(super) async fn sleep_ms(ms: u32) {
    crate::util::sleep(std::time::Duration::from_millis(ms.into())).await;
}

pub(super) fn status_class(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Ok => "bg-green-50 text-green-800 border-green-200",
        RunStatus::Failed => "bg-red-50 text-alarm border-red-200",
        RunStatus::Running => "bg-gray-50 text-slate border-line",
    }
}

pub(super) fn status_label(s: RunStatus) -> String {
    match s {
        RunStatus::Ok => t!("web-sites-status-ok").to_string(),
        RunStatus::Failed => t!("web-sites-status-failed").to_string(),
        RunStatus::Running => t!("web-sites-status-running").to_string(),
    }
}

pub(super) fn reason_label(token: &str) -> String {
    resolve(&format!("web-sites-reason-{token}"), None)
}

pub(super) fn action_label(action: &str) -> String {
    resolve(
        &format!("web-sites-action-{}", action.replace('_', "-")),
        None,
    )
}

#[component]
pub fn WebSites() -> Element {
    let mut reload = use_signal(|| 0u32);
    let search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);
    let mut adding = use_signal(|| false);
    let mut host = use_signal(String::new);
    let navigator = use_navigator();
    let sites = use_resource(move || async move {
        let _ = reload();
        api::site_recipes::sites().await
    });
    let can_write = can("site-recipes:write");
    // Whole list loaded (one row per site): filtered and paged here.
    let query = search().trim().to_lowercase();
    let (state, all) = match &*sites.read() {
        None => (None, Vec::new()),
        Some(Ok(list)) => (Some(Ok(())), list.clone()),
        Some(Err(e)) => (Some(Err(e.clone())), Vec::new()),
    };
    let filtered: Vec<WebSite> = all
        .into_iter()
        .filter(|s| query.is_empty() || s.site.contains(&query))
        .collect();
    let count = filtered.len() as i64;
    let rows = page_slice(&filtered, page());
    let columns = vec![
        Column::new(t!("web-sites-col-site").to_string(), |s: &WebSite| {
            rsx! { "{s.site}" }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("web-sites-col-recipes").to_string(), |s: &WebSite| {
            rsx! { "{s.recipes}" }
        }),
        Column::new(t!("web-sites-col-pages").to_string(), |s: &WebSite| {
            rsx! { "{s.pages}" }
        }),
        Column::new(
            t!("web-sites-col-last-run").to_string(),
            |s: &WebSite| match (s.last_run_at.clone(), s.last_status) {
                (Some(at), Some(st)) => rsx! {
                    span { class: "inline-flex items-center gap-2",
                        span { class: "rounded border px-1.5 py-0.5 text-xs {status_class(st)}",
                            "{status_label(st)}"
                        }
                        span { class: "text-slate", "{short_time(&at)}" }
                    }
                },
                _ => rsx! {
                    span { class: "text-slate", "—" }
                },
            },
        ),
    ];
    let open_new = move |_| {
        if let Some(site) = site_host(&host()) {
            adding.set(false);
            navigator.push(Route::WebSiteStudio { site });
        }
    };
    rsx! {
        ListLayout {
            title: t!("web-sites-title").to_string(),
            subtitle: Some(t!("web-sites-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            add_label: Some(t!("web-sites-add").to_string()),
            on_add: move |_| {
                host.set(String::new());
                adding.set(true);
            },
            div { class: "space-y-0",
                FilterBar {
                    SearchInput {
                        placeholder: t!("web-sites-search").to_string(),
                        value: search,
                        on_submit: move |_| page.set(0),
                        page: Some(page),
                    }
                }
                ListStates {
                    state,
                    is_empty: count == 0,
                    empty_message: t!("web-sites-empty").to_string(),
                    div { class: "relative",
                        DataTable {
                            columns,
                            rows,
                            row_key: RowKey::new(|s: &WebSite| s.site.clone()),
                            on_row_click: move |site: String| {
                                navigator.push(Route::WebSiteStudio { site });
                            },
                        }
                        ListPager { count, page }
                    }
                }
            }
        }
        if adding() {
            FormDialog {
                title: t!("web-sites-add").to_string(),
                submit_label: t!("web-sites-open-studio").to_string(),
                on_close: move |_| adding.set(false),
                on_submit: open_new,
                busy: false,
                valid: site_host(&host()).is_some(),
                div { class: "space-y-3",
                    label { class: FIELD_LABEL, {t!("web-sites-host-label")} }
                    input {
                        class: INPUT,
                        placeholder: "of-exemple.example",
                        value: "{host}",
                        oninput: move |e| host.set(e.value()),
                    }
                    p { class: FIELD_HELP, {t!("web-sites-host-help")} }
                    if !host().trim().is_empty() && site_host(&host()).is_none() {
                        p { class: FIELD_ERROR, {t!("err-invalid")} }
                    }
                }
            }
        }
    }
}
