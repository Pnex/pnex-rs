//! Rendu live READ-ONLY d'un dashboard (studio SCADA) — extrait de
//! `pages/dashboards.rs` pour être réutilisable **hors de la page** : le
//! panneau d'aperçu POI (/map) embarque un dashboard en inline, sans
//! navigation ni popup.
//!
//! Le rendu est pur (positions en % du canvas + SVG viewBox — aucune
//! mesure DOM, s'adapte à n'importe quelle largeur) et sans état global ;
//! le fetch/polling est autonome (detail + `series-batch` fenêtre 1h,
//! un seul timer 15 s — école `LiveSubView`).

use std::collections::HashMap;
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{DashboardFormat, DashboardLayout, TelemetryPoint, VizDashboard, Widget};

use crate::api;
use crate::components::dashboard_widget::WidgetBody;
use crate::util::sleep;

/// Cadence de rafraîchissement live (même cadence que la page Dashboards).
const POLL_SECS: u64 = 15;

/// Dashboard live read-only autonome : detail + series-batch + polling
/// 15 s auto-entretenu + rendu. Les items sans donnée sont grisés avec
/// tooltip (dégradé = dérivé au rendu, jamais un set de signal).
#[component]
pub fn DashboardLive(dashboard_id: String) -> Element {
    let dashboard_id_for_via = dashboard_id.clone();
    let mut reload = use_signal(|| 0u32);
    let mut polling = use_signal(|| false);

    // Failed fetch (slow or lost network): the last good layout stays on
    // screen; without one the body shows the error and a retry button
    // instead of an endless placeholder.
    let mut load_failed = use_signal(|| false);
    let mut last_ok = use_signal(|| None::<VizDashboard>);
    let detail = use_resource(move || {
        let id = dashboard_id.clone();
        async move {
            let _ = reload();
            match api::dashboards::detail(&id).await {
                Ok(d) => {
                    load_failed.set(false);
                    last_ok.set(Some(d.clone()));
                    Some(d)
                }
                Err(_) => {
                    load_failed.set(true);
                    last_ok.peek().clone()
                }
            }
        }
    });
    let detail_loaded: Option<VizDashboard> = detail.read().as_ref().cloned().flatten();

    // ── Specs du batch — déps trackées lues dans la partie synchrone de
    // la closure (pattern devices.rs) : reload (cadence 15 s) ET la
    // resource `detail` elle-même. Au premier rendu le détail est absent
    // → sources vides ; si seule `reload` est abonnée, le batch ne repart
    // qu'au premier tick (15 s d'écran vide en vue/plan) — lire `detail`
    // ici garantit le re-run dès qu'il se résout. Un SEUL timer 15 s
    // pour tout le dashboard (D31).
    // shortcut: no object picker here, so a type dashboard (D187) embedded
    // in a POI preview reads its object sources as no data; add the picker
    // if type dashboards get placed on POIs.
    let batch = use_resource(move || {
        let sources: Vec<pnex_core::SourceRef> = detail
            .read()
            .as_ref()
            .cloned()
            .flatten()
            .map(|d| {
                d.layout
                    .widgets
                    .iter()
                    .flat_map(|w| w.source.iter().cloned())
                    .collect()
            })
            .unwrap_or_default();
        let _ = reload();
        async move {
            if sources.is_empty() {
                return None;
            }
            Some(fetch_live_values(sources).await)
        }
    });
    let values: HashMap<String, Option<Vec<TelemetryPoint>>> =
        batch.read().as_ref().cloned().flatten().unwrap_or_default();

    // Control cards (D125): definitions and last commanded values follow
    // the same polling tick.
    let via = format!("dashboard:{}", dashboard_id_for_via);
    crate::components::surface::use_surface_controls(
        move || {
            detail
                .read()
                .as_ref()
                .cloned()
                .flatten()
                .map(|d| crate::components::surface::control_ids(&d.layout))
                .unwrap_or_default()
        },
        reload,
        via,
        crate::state::org::current_can_write(),
    );

    // Polling auto-entretenu tant que la page est montée (école
    // visualisation.rs : le spawn se réarme lui-même via le signal).
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    rsx! {
        {
            match detail_loaded {
                Some(d) => rsx! {
                    {live_layout(&d.layout, &values)}
                },
                None if load_failed() => rsx! {
                    div { class: "flex flex-col items-center gap-3 py-12",
                        p { class: "text-sm text-gray-500", {t!("dashboard-live-unavailable")} }
                        button {
                            class: "px-3 py-1.5 text-sm text-blue-700 border border-blue-200 rounded-lg hover:bg-blue-50 transition-colors",
                            onclick: move |_| reload.with_mut(|r| *r += 1),
                            {t!("common-retry")}
                        }
                    }
                },
                None => rsx! {
                    div { class: "flex justify-center py-12",
                        span { class: "animate-spin rounded-full h-6 w-6 border-b-2 border-blue-600" }
                    }
                },
            }
        }
    }
}

/// Live values of `sources`, keyed by [`pnex_core::SourceRef::series_key`]:
/// telemetry series through one `series-batch` (each series fetched over
/// the widest window its widgets ask for), memory values through one
/// `memory/values` call. `None` = unavailable (degraded).
pub async fn fetch_live_values(
    sources: Vec<pnex_core::SourceRef>,
) -> HashMap<String, Option<Vec<TelemetryPoint>>> {
    let mut map: HashMap<String, Option<Vec<TelemetryPoint>>> = sources
        .iter()
        .filter(|s| !s.is_unset())
        .map(|s| (s.series_key(), None))
        .collect();
    let mut specs = series_specs(&sources);
    specs.sort_by(|a, b| {
        (&a.metric, &a.device_id, &a.labels).cmp(&(&b.metric, &b.device_id, &b.labels))
    });
    let mut memory = Vec::new();
    for s in sources {
        if let Some(m) = s.memory {
            memory.push(m);
        }
    }
    memory.sort();
    memory.dedup();
    if !specs.is_empty() {
        // Results come back in spec order: the spec gives the key (labels
        // included, D171).
        let keys: Vec<String> = specs.iter().map(|s| s.series_key()).collect();
        if let Ok(resp) =
            api::dashboards::series_batch(pnex_core::SeriesBatchRequest { specs }).await
        {
            for (key, r) in keys.into_iter().zip(resp.results) {
                map.insert(key, if r.available { Some(r.points) } else { None });
            }
        }
    }
    map.extend(api::memory::live_entries(memory).await);
    map
}

/// Canvas live : le document est mis à l'échelle **en CSS pur** — chaque
/// widget est positionné en pourcentage du canvas (aucune mesure DOM,
/// aucun zoom manuel : le synoptique s'adapte à la largeur disponible) ;
/// les traits vivent dans un SVG en viewBox document (mise à l'échelle
/// gratuite).
pub fn live_canvas(
    layout: &DashboardLayout,
    values: &HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> Element {
    let (cw, ch) = (layout.canvas.width.max(1), layout.canvas.height.max(1));
    let bg = layout
        .canvas
        .background
        .clone()
        .unwrap_or_else(|| "#f8fafc".into());
    rsx! {
        div {
            class: "relative w-full rounded-lg border border-gray-200 overflow-hidden",
            style: "aspect-ratio: {cw} / {ch}; background-color: {bg};",
            for w in &layout.widgets {
                LiveWidget {
                    key: "{w.id}",
                    w: w.clone(),
                    canvas: (cw, ch),
                    values: values.clone(),
                }
            }
            svg {
                xmlns: "http://www.w3.org/2000/svg",
                view_box: "0 0 {cw} {ch}",
                class: "absolute inset-0 w-full h-full pointer-events-none",
                for wire in &layout.wires {
                    {live_wire(layout, wire)}
                }
            }
        }
    }
}

fn live_wire(layout: &DashboardLayout, wire: &pnex_core::Wire) -> Element {
    let Some(path) = crate::components::dashboard_editor::geometry::wire_path(
        layout,
        &wire.from.widget_id,
        wire.from.side,
        &wire.to.widget_id,
        wire.to.side,
    ) else {
        return rsx! {};
    };
    rsx! {
        path {
            d: "{path}",
            fill: "none",
            stroke: "#94a3b8",
            "stroke-width": "2",
            "stroke-dasharray": "6 4",
            "stroke-linecap": "round",
        }
    }
}

/// Points of the primary source of a widget and whether it is degraded
/// (the key exists but at `None`: O2 unreachable for this series).
fn widget_points(
    w: &Widget,
    values: &HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> (Option<Vec<TelemetryPoint>>, bool) {
    let points = w
        .source
        .first()
        .and_then(|s| values.get(&s.series_key()))
        .cloned()
        .flatten();
    let degraded = w
        .source
        .first()
        .map(|s| values.contains_key(&s.series_key()) && points.is_none())
        .unwrap_or(false);
    (points, degraded)
}

#[component]
pub fn LiveWidget(
    w: Widget,
    canvas: (i64, i64),
    values: HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> Element {
    let (cw, ch) = canvas;
    let left = w.x as f64 / cw as f64 * 100.0;
    let top = w.y as f64 / ch as f64 * 100.0;
    let width = w.w as f64 / cw as f64 * 100.0;
    let height = w.h as f64 / ch as f64 * 100.0;
    // Degraded: greyed out with a tooltip, never a toast in a loop.
    let (points, degraded) = widget_points(&w, &values);
    let tooltip = if degraded {
        t!("db-degraded", widget: w.title.clone()).to_string()
    } else {
        String::new()
    };

    rsx! {
        div {
            class: "absolute",
            style: "left: {left}%; top: {top}%; width: {width}%; height: {height}%;",
            title: "{tooltip}",
            div { class: if degraded { "w-full h-full opacity-50" } else { "w-full h-full" },
                WidgetBody {
                    widget: w.clone(),
                    points,
                    values: Some(values.clone()),
                }
            }
        }
    }
}

/// Live rendering of a layout in its format (D123): free canvas on
/// desktop, stack of cards on mobile.
pub fn live_layout(
    layout: &DashboardLayout,
    values: &HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> Element {
    match layout.format {
        DashboardFormat::Desktop => live_canvas(layout, values),
        DashboardFormat::Mobile => rsx! {
            crate::components::dashboard_live_mobile::LiveStack { layout: layout.clone(), values: values.clone() }
        },
    }
}

/// One `series-batch` spec per telemetry series, over the widest window
/// requested for it (unknown windows fall back to 1 h).
fn series_specs(sources: &[pnex_core::SourceRef]) -> Vec<pnex_core::SeriesSpec> {
    let secs = |w: &str| {
        pnex_core::VIZ_WINDOW_PRESETS
            .iter()
            .find(|(k, _)| *k == w)
            .map(|(_, s)| *s)
    };
    type SeriesId = (String, String, std::collections::BTreeMap<String, String>);
    let mut by_series: HashMap<SeriesId, String> = HashMap::new();
    for s in sources
        .iter()
        .filter(|s| s.memory.is_none() && !s.is_unset())
    {
        let window = if secs(&s.window).is_some() {
            s.window.clone()
        } else {
            "1h".to_string()
        };
        by_series
            .entry((s.metric.clone(), s.device_id.clone(), s.labels.clone()))
            .and_modify(|cur| {
                if secs(&window) > secs(cur) {
                    *cur = window.clone();
                }
            })
            .or_insert(window);
    }
    by_series
        .into_iter()
        .map(
            |((metric, device_id, labels), window)| pnex_core::SeriesSpec {
                metric,
                device_id,
                window,
                labels,
            },
        )
        .collect()
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn src(metric: &str, window: &str) -> pnex_core::SourceRef {
        pnex_core::SourceRef {
            object_property: None,
            role: "primary".into(),
            metric: metric.into(),
            device_id: "dev".into(),
            window: window.into(),
            memory: None,
            labels: Default::default(),
        }
    }

    #[test]
    fn each_series_uses_its_widest_window() {
        let mut specs = series_specs(&[
            src("t", "5m"),
            src("t", "24h"),
            src("h", "15m"),
            src("p", "bogus"),
        ]);
        specs.sort_by(|a, b| a.metric.cmp(&b.metric));
        let got: Vec<(&str, &str)> = specs
            .iter()
            .map(|s| (s.metric.as_str(), s.window.as_str()))
            .collect();
        assert_eq!(got, vec![("h", "15m"), ("p", "1h"), ("t", "24h")]);
    }
}
