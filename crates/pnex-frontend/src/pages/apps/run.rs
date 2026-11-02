//! A running application (app-builder.md D513): its screens with their own
//! navigation, the published versions, the variables of this session.
//! `?v.<name>=<value>` fills a variable (deep link, D509).

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::apps::AppView;
use pnex_core::{DashboardFormat, Widget};

use super::use_app_by_key;
use crate::api;
use crate::app::Route;
use crate::components::app_widgets::AppRunCx;
use crate::components::dashboard_live::{fetch_live_values, live_layout, widget_points};
use crate::components::dashboard_widget::WidgetBody;
use crate::components::gerbe::Gerbe;
use crate::util::sleep;

const POLL_SECS: u64 = 15;

#[component]
pub fn AppRun(app: String, screen: String, query: String) -> Element {
    let app = use_app_by_key(app);
    let state = app.read().clone();
    match state {
        None => rsx! {
            div { class: "flex justify-center py-12",
                Gerbe { size: 28 }
            }
        },
        Some(None) => rsx! {
            p { class: "text-gray-500 text-center py-12", {t!("err-app-not-found")} }
        },
        Some(Some(a)) => rsx! {
            Runner {
                key: "{a.id}",
                app: a.clone(),
                screen,
                query,
            }
        },
    }
}

/// Defaults of the variables, then what the address gives. Only declared
/// variables are taken: a variable is a value, never an expression (D512).
fn initial_values(app: &AppView, query: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = app
        .variables
        .iter()
        .filter_map(|v| Some((v.name.clone(), v.default.clone()?)))
        .collect();
    for pair in query.trim_start_matches('?').split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let Some(name) = k.strip_prefix("v.") else {
            continue;
        };
        if app.variables.iter().any(|d| d.name == name) {
            out.insert(name.to_string(), percent_decoded(v));
        }
    }
    out
}

fn percent_decoded(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%')
            .then(|| raw.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (hex, bytes[i]) {
            (Some(b), _) => {
                out.push(b);
                i += 3;
            }
            (None, b'+') => {
                out.push(b' ');
                i += 1;
            }
            (None, b) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[component]
fn Runner(app: AppView, screen: String, query: String) -> Element {
    let start = app.clone();
    let app_signal = use_signal(move || start);
    let start = app.clone();
    let vars = use_signal(move || initial_values(&start, &query));
    let mut tick = use_signal(|| 0u32);
    let mut types_signal = use_signal(Vec::new);
    use_future(move || async move {
        if let Ok(types) = api::ontology::types().await {
            types_signal.set(types);
        }
    });
    use_context_provider(|| AppRunCx {
        app: app_signal,
        types: types_signal,
        vars,
        tick,
    });
    // One timer for the whole screen (D31), re-armed by itself.
    let mut polling = use_signal(|| false);
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            tick.with_mut(|t| *t += 1);
        });
    }
    let navigable: Vec<_> = app
        .screens
        .iter()
        .filter(|s| s.published_version.is_some())
        .cloned()
        .collect();
    let app_key = app.key.clone();
    let via = app
        .screens
        .iter()
        .find(|s| s.key == screen)
        .map(|s| format!("dashboard:{}", s.dashboard_id))
        .unwrap_or_default();
    rsx! {
        div { class: "flex flex-col gap-3 md:flex-row",
            nav {
                class: "flex shrink-0 flex-row gap-1 overflow-x-auto md:w-48 md:flex-col",
                "data-testid": "app-nav",
                Link {
                    class: "rounded-md px-3 py-1.5 text-xs text-slate hover:bg-surface",
                    to: Route::Apps {},
                    {t!("apps-back")}
                }
                p { class: "hidden px-3 pt-2 text-sm font-semibold text-ink md:block",
                    {crate::components::ontology::tr(&app.name, &app.names)}
                }
                for s in navigable {
                    Link {
                        key: "{s.key}",
                        class: if s.key == screen { "rounded-md bg-surface px-3 py-1.5 text-sm font-semibold text-ink shadow-[0_0_0_1px_var(--color-line)]" } else { "rounded-md px-3 py-1.5 text-sm text-ink hover:bg-surface" },
                        to: Route::AppRun {
                            app: app_key.clone(),
                            screen: s.key.clone(),
                            query: String::new(),
                        },
                        {crate::components::ontology::tr(&s.name, &s.names)}
                    }
                }
            }
            div { class: "min-w-0 flex-1",
                ScreenBody {
                    key: "{screen}",
                    app_id: app.id.clone(),
                    screen: screen.clone(),
                    via,
                    tick,
                }
            }
        }
    }
}

#[component]
fn ScreenBody(app_id: String, screen: String, via: String, tick: Signal<u32>) -> Element {
    // Reactive on the props: a route change keeps this component mounted.
    let loaded = use_resource(use_reactive((&app_id, &screen), |(id, key)| async move {
        api::apps::screen(&id, &key).await
    }));
    let batch = use_resource(move || {
        let sources: Vec<pnex_core::SourceRef> = loaded
            .read()
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|s| {
                s.layout
                    .widgets
                    .iter()
                    .flat_map(|w| w.source.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        let _ = tick();
        async move {
            if sources.is_empty() {
                return None;
            }
            Some(fetch_live_values(sources).await)
        }
    });
    let narrow = use_resource(|| async {
        dioxus::document::eval("return window.innerWidth < 768")
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    });
    let narrow = narrow.read().unwrap_or(false);
    let values: HashMap<_, _> = batch.read().as_ref().cloned().flatten().unwrap_or_default();
    crate::components::surface::use_surface_controls(
        move || {
            loaded
                .read()
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .map(|s| crate::components::surface::control_ids(&s.layout))
                .unwrap_or_default()
        },
        tick,
        via,
        crate::state::access::can("controls:operate"),
    );
    let state = loaded.read().clone();
    let view = match state {
        None => {
            return rsx! {
                div { class: "flex justify-center py-12",
                    Gerbe { size: 28 }
                }
            }
        }
        Some(Err(e)) => {
            return rsx! {
                p { class: "text-gray-500 text-center py-12",
                    {crate::api::error_i18n::localize(&e)}
                }
            }
        }
        Some(Ok(v)) => v,
    };
    if view.layout.format == DashboardFormat::Mobile {
        return rsx! {
            {live_layout(&view.layout, &values)}
        };
    }
    // A narrow screen stacks the widgets in reading order (Q-B6) instead of
    // shrinking a canvas drawn for a desktop. Rendered once: hidden widgets
    // would still query.
    if !narrow {
        return rsx! {
            {live_layout(&view.layout, &values)}
        };
    }
    let mut stacked: Vec<Widget> = view.layout.widgets.clone();
    stacked.sort_by_key(|w| (w.y, w.x));
    rsx! {
        div { class: "space-y-3",
            for w in stacked {
                div {
                    key: "{w.id}",
                    class: "min-h-24 overflow-hidden rounded-lg border border-gray-200 bg-surface",
                    WidgetBody {
                        widget: w.clone(),
                        points: widget_points(&w, &values).0,
                        values: Some(values.clone()),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_fills_only_declared_variables() {
        let app: AppView = serde_json::from_value(serde_json::json!({
            "id": "a", "key": "k", "name": "n", "scope": {}, "version": 1,
            "created_at": "", "updated_at": "",
            "variables": [{"name": "lang", "default": "en"}, {"name": "q"}]
        }))
        .unwrap();
        assert_eq!(initial_values(&app, "")["lang"], "en");
        let v = initial_values(&app, "?v.lang=fr&v.q=caf%C3%A9+noir&v.ghost=1&other=2");
        assert_eq!(v["lang"], "fr");
        assert_eq!(v["q"], "café noir");
        assert!(!v.contains_key("ghost") && !v.contains_key("other"));
    }
}
