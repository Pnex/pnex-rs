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

    let detail = use_resource(move || {
        let id = dashboard_id.clone();
        async move {
            let _ = reload();
            api::dashboards::detail(&id).await.ok()
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
                None => rsx! {
                    p { class: "text-gray-500 text-center py-12", "…" }
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
    let mut map: HashMap<String, Option<Vec<TelemetryPoint>>> =
        sources.iter().map(|s| (s.series_key(), None)).collect();
    let mut specs = series_specs(&sources);
    specs.sort_by(|a, b| (&a.metric, &a.device_id).cmp(&(&b.metric, &b.device_id)));
    let mut memory = Vec::new();
    for s in sources {
        if let Some(m) = s.memory {
            memory.push(m);
        }
    }
    memory.sort();
    memory.dedup();
    if !specs.is_empty() {
        if let Ok(resp) =
            api::dashboards::series_batch(pnex_core::SeriesBatchRequest { specs }).await
        {
            for r in resp.results {
                map.insert(
                    format!("{}|{}", r.metric, r.device_id),
                    if r.available { Some(r.points) } else { None },
                );
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
        DashboardFormat::Mobile => live_stack(layout, values),
    }
}

/// Grid classes of a mobile card (D124): column span (1 = half, 2 = full
/// row, default full) and a height fitted to the widget type.
pub fn mobile_card_classes(w: &Widget) -> &'static str {
    let half = w.options.span == Some(1);
    match (w.widget_type.as_str(), half) {
        ("thermo_chart", _) => "col-span-2 h-80",
        ("gauge", true) => "col-span-1 h-40",
        ("gauge", false) => "col-span-2 h-48",
        ("line", true) => "col-span-1 h-36",
        ("line", false) => "col-span-2 h-40",
        ("slider" | "number", true) => "col-span-1 h-32",
        ("slider" | "number", false) => "col-span-2 h-32",
        ("text", true) => "col-span-1 min-h-20",
        ("text", false) => "col-span-2 min-h-20",
        (_, true) => "col-span-1 h-28",
        (_, false) => "col-span-2 h-28",
    }
}

/// Mobile stack (D124): sections in order, each a 2-column grid of cards.
/// Pure CSS, phone-width column centred on a large screen.
pub fn live_stack(
    layout: &DashboardLayout,
    values: &HashMap<String, Option<Vec<TelemetryPoint>>>,
) -> Element {
    let sections: Vec<(String, String, Vec<Widget>)> = layout
        .sections
        .iter()
        .map(|sec| {
            let cards = sec
                .items
                .iter()
                .filter_map(|id| layout.widgets.iter().find(|w| &w.id == id).cloned())
                .collect();
            (sec.id.clone(), sec.title.clone(), cards)
        })
        .collect();
    rsx! {
        div { class: "mx-auto w-full max-w-md space-y-5",
            for (id, title, cards) in sections {
                section { key: "{id}", class: "space-y-2",
                    if !title.trim().is_empty() {
                        h2 { class: "px-1 text-xs font-semibold uppercase tracking-wide text-gray-500",
                            "{title}"
                        }
                    }
                    div { class: "grid grid-cols-2 gap-3",
                        for w in cards {
                            StackCard {
                                key: "{w.id}",
                                w: w.clone(),
                                values: values.clone(),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StackCard(w: Widget, values: HashMap<String, Option<Vec<TelemetryPoint>>>) -> Element {
    let (points, degraded) = widget_points(&w, &values);
    let classes = mobile_card_classes(&w);
    let fade = if degraded { "opacity-50" } else { "" };
    rsx! {
        div { class: "{classes} {fade} overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm",
            WidgetBody { widget: w.clone(), points, values: Some(values.clone()) }
        }
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
    let mut by_series: HashMap<(String, String), String> = HashMap::new();
    for s in sources.iter().filter(|s| s.memory.is_none()) {
        let window = if secs(&s.window).is_some() {
            s.window.clone()
        } else {
            "1h".to_string()
        };
        by_series
            .entry((s.metric.clone(), s.device_id.clone()))
            .and_modify(|cur| {
                if secs(&window) > secs(cur) {
                    *cur = window.clone();
                }
            })
            .or_insert(window);
    }
    by_series
        .into_iter()
        .map(|((metric, device_id), window)| pnex_core::SeriesSpec {
            metric,
            device_id,
            window,
        })
        .collect()
}

#[cfg(test)]
mod window_tests {
    use super::*;

    fn src(metric: &str, window: &str) -> pnex_core::SourceRef {
        pnex_core::SourceRef {
            role: "primary".into(),
            metric: metric.into(),
            device_id: "dev".into(),
            window: window.into(),
            memory: None,
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
