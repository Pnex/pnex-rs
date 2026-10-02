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
use pnex_core::{DashboardLayout, TelemetryPoint, VizDashboard, Widget};

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
                    {live_canvas(&d.layout, &values)}
                },
                None => rsx! {
                    p { class: "text-gray-500 text-center py-12", "…" }
                },
            }
        }
    }
}

/// Live values of `sources`, keyed by [`pnex_core::SourceRef::series_key`]:
/// telemetry series through one `series-batch` (1 h window), memory values
/// through one `memory/values` call. `None` = unavailable (degraded).
pub async fn fetch_live_values(
    sources: Vec<pnex_core::SourceRef>,
) -> HashMap<String, Option<Vec<TelemetryPoint>>> {
    let mut map: HashMap<String, Option<Vec<TelemetryPoint>>> =
        sources.iter().map(|s| (s.series_key(), None)).collect();
    let mut specs = Vec::new();
    let mut memory = Vec::new();
    for s in sources {
        match s.memory {
            Some(m) => memory.push(m),
            None => specs.push(pnex_core::SeriesSpec {
                metric: s.metric,
                device_id: s.device_id,
                window: "1h".into(),
            }),
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
    let points = w
        .source
        .first()
        .and_then(|s| values.get(&s.series_key()))
        .cloned()
        .flatten();
    // Dégradé = la clé existe mais à None (O2 injoignable pour cette
    // série) — grisée + tooltip, jamais de toast en boucle.
    let degraded = w
        .source
        .first()
        .map(|s| values.contains_key(&s.series_key()) && points.is_none())
        .unwrap_or(false);
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
