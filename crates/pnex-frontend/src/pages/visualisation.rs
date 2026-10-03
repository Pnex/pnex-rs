//! Visualisation (2026-08-19) — courbes capteur par capteur depuis
//! OpenObserve : catalogue des séries de l'org (`/api/v1/telemetry/catalog`)
//! puis points par série sur une fenêtre preset (`/api/v1/telemetry/series`).
//!
//! Chart SVG maison (aucune lib) : polyline + points, échelle Y globale
//! sur toutes les séries actives — quick and dirty, objectif = valider la
//! mécanique de collecte de bout en bout. Jusqu'à 6 séries superposées,
//! polling 15 s (voir les données arriver live), télémétrie dégradée en
//! encart (jamais de toast en boucle).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TelemetryPoint;

use crate::api;
use crate::components::charts::{ChartSeries, TimeSeriesChart, PALETTE};
use crate::components::crud::filters::RefreshButton;
use crate::components::icons;
use crate::state::org;
use crate::util::sleep;

/// Cadence de rafraîchissement (même choix que le dashboard : voir la
/// donnée arriver sans marteler O2).
const POLL_SECS: u64 = 15;

/// Séries max superposées (taille de la palette).
const MAX_SERIES: usize = 6;

/// Fenêtres preset proposées (clé API, libellé i18n, secondes) —
/// identiques à `WINDOWS` côté backend.
const WINDOWS: &[(&str, &str, i64)] = &[
    ("1h", "vis-window-1h", 3600),
    ("6h", "vis-window-6h", 21_600),
    ("24h", "vis-window-24h", 86_400),
];

/// Une série active côté page : clé d'ajout + points chargés.
#[derive(Clone)]
struct ActiveSeries {
    metric: String,
    device_id: String,
    /// `None` = requête en échec ou dégradée (encart global).
    points: Option<Vec<TelemetryPoint>>,
}

#[component]
pub fn Visualisation() -> Element {
    if session_absent() {
        return rsx! {};
    }

    // Sélection courante des pickers + séries actives + fenêtre.
    let mut reload = use_signal(|| 0u32);
    let mut window = use_signal(|| 86_400i64);
    let mut sel_metric = use_signal(String::new);
    let mut sel_device = use_signal(String::new);
    let mut active: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut polling = use_signal(|| false);

    // Catalogue : re-run au changement d'org ou au reload (polling +
    // bouton). Erreurs avalées → None (la page reste utilisable).
    let catalog = use_resource(move || async move {
        let _ = reload();
        org::current()?;
        api::telemetry::catalog().await.ok()
    });
    let cat = match catalog.read().as_ref() {
        Some(inner) => inner.clone(),
        None => None,
    };

    // Points : une requête par série active (≤ 6), séquentielles —
    // charge triviale au rythme du polling. Les signaux sont lus dans la
    // partie SYNCHRONE de la closure (pattern devices.rs) : l'abonnement
    // au re-run est garanti sur active/window/reload.
    let series = use_resource(move || {
        let _ = reload();
        let keys = active.read().clone();
        let window_key = WINDOWS
            .iter()
            .find(|(_, _, secs)| *secs == window())
            .map(|(key, _, _)| *key)
            .unwrap_or("24h");
        async move {
            let mut out = Vec::new();
            for (metric, device_id) in &keys {
                let points = api::telemetry::series(metric, device_id, window_key)
                    .await
                    .ok()
                    .filter(|s| s.available)
                    .map(|s| s.points);
                out.push(ActiveSeries {
                    metric: metric.clone(),
                    device_id: device_id.clone(),
                    points,
                });
            }
            out
        }
    });

    // Polling auto-entretenu tant que la page est montée.
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    // ── Tout est précalculé ici : les enfants rsx sont évalués
    // paresseusement, on ne creuse pas les Option dedans.
    // Métriques distinctes du catalogue (trié côté backend).
    let metrics: Vec<String> = cat
        .as_ref()
        .map(|c| {
            let mut names: Vec<String> = c.series.iter().map(|s| s.metric.clone()).collect();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_default();
    // Sélections EFFECTIVES, calculées — jamais de `set` au rendu : le
    // set Dioxus notifie sans comparer les valeurs, muter pendant le
    // rendu tant que le catalogue est vide bouclait à l'infini et
    // gelait toute l'app (retour user 2026-08-19 : « toutes les pages
    // bloquées »). Piège des selects contrôlés réglé par la même passe :
    // sans valeur effective, le select AFFICHE sa première option sans
    // qu'elle soit « sélectionnée » (bouton Ajouter restait désactivé,
    // aucun appel /series dans les logs serveur).
    let cur_metric = sel_metric();
    let eff_metric = if metrics.contains(&cur_metric) {
        cur_metric
    } else {
        metrics.first().cloned().unwrap_or_default()
    };
    // Devices de la métrique effective.
    let devices: Vec<String> = cat
        .as_ref()
        .map(|c| {
            c.series
                .iter()
                .filter(|s| s.metric == eff_metric)
                .map(|s| s.device_id.clone())
                .collect()
        })
        .unwrap_or_default();
    let cur_device = sel_device();
    let eff_device = if devices.contains(&cur_device) {
        cur_device
    } else {
        devices.first().cloned().unwrap_or_default()
    };
    let can_add = !eff_metric.is_empty()
        && !eff_device.is_empty()
        && active.read().len() < MAX_SERIES
        && !active
            .read()
            .iter()
            .any(|(m, d)| *m == eff_metric && *d == eff_device);

    // Chargement des points : flatten de la ressource.
    let loaded = match series.read().as_ref() {
        Some(list) => list.clone(),
        None => Vec::new(),
    };
    // Age of each series' last sample (catalog `last_seen`, real sample
    // time), shown next to its label: a flat line may just be stale data.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let age_of = |metric: &str, device: &str| -> Option<String> {
        let seen = cat
            .as_ref()?
            .series
            .iter()
            .find(|s| s.metric == metric && s.device_id == device)?
            .last_seen
            .as_deref()?;
        let ms = chrono::DateTime::parse_from_rfc3339(seen)
            .ok()?
            .timestamp_millis();
        Some(t!("vis-series-age", age: crate::pages::cameras::age_label(ms, now_ms)).to_string())
    };
    let series_label = |metric: &str, device: &str| match age_of(metric, device) {
        Some(age) => format!("{metric} · {device} · {age}"),
        None => format!("{metric} · {device}"),
    };
    let chips: Vec<(String, String, String)> = active
        .read()
        .iter()
        .map(|(m, d)| (m.clone(), d.clone(), series_label(m, d)))
        .collect();
    let any_failed = !loaded.is_empty() && loaded.iter().any(|s| s.points.is_none());
    let chart_input: Vec<ChartSeries> = loaded
        .iter()
        .enumerate()
        .map(|(index, s)| ChartSeries {
            color_index: index,
            label: series_label(&s.metric, &s.device_id),
            points: s.points.clone(),
        })
        .collect();
    let total_points: usize = loaded
        .iter()
        .filter_map(|s| s.points.as_ref().map(|p| p.len()))
        .sum();

    rsx! {
        div { class: "p-6",
            div { class: "mb-8 flex items-center justify-between",
                div {
                    h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-quick-charts")} }
                    p { class: "text-gray-600 mt-2", {t!("vis-subtitle")} }
                }
                div { class: "flex items-center gap-3",
                    span { class: "text-xs text-gray-400", {t!("dash-auto-refresh")} }
                    RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
                }
            }

            // Sélection : métrique × device × fenêtre + ajout
            div { class: "bg-white rounded-lg shadow-sm mb-8",
                div { class: "p-6 border-b border-gray-200",
                    h2 { class: "text-lg font-semibold text-gray-900", {t!("vis-series")} }
                }
                div { class: "p-6",
                    match cat.as_ref().filter(|c| c.available) {
                        None => rsx! {
                            p { class: "text-sm text-gray-500 bg-gray-50 border border-gray-200 rounded-lg p-4",
                                {t!("vis-unavailable")}
                            }
                        },
                        Some(c) if c.series.is_empty() => rsx! {
                            p { class: "text-gray-500 text-center py-8", {t!("vis-no-data")} }
                        },
                        Some(_) => rsx! {
                            div { class: "flex flex-wrap items-end gap-3",
                                div {
                                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", {t!("vis-metric")} }
                                    select {
                                        aria_label: t!("vis-metric"),
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| {
                                            sel_metric.set(event.value());
                                        },
                                        for metric in &metrics {
                                            option { value: "{metric}", selected: eff_metric == *metric, {metric.clone()} }
                                        }
                                    }
                                }
                                div {
                                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", {t!("vis-device")} }
                                    select {
                                        aria_label: t!("vis-device"),
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| sel_device.set(event.value()),
                                        for device in &devices {
                                            option { value: "{device}", selected: eff_device == *device, {device.clone()} }
                                        }
                                    }
                                }
                                div {
                                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", {t!("vis-window")} }
                                    div { class: "flex rounded-lg border border-gray-300 overflow-hidden",
                                        for (key, label, secs) in WINDOWS {
                                            button {
                                                key: "{key}",
                                                class: if window() == *secs { "px-3 py-2 text-sm bg-blue-600 text-white" } else { "px-3 py-2 text-sm bg-white text-gray-700 hover:bg-gray-50" },
                                                onclick: move |_| window.set(*secs),
                                                {t!(label)}
                                            }
                                        }
                                    }
                                }
                                button {
                                    class: if can_add { "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors" } else { "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg opacity-40 cursor-not-allowed" },
                                    disabled: !can_add,
                                    onclick: move |_| {
                                        if can_add {
                                            active.with_mut(|list| list.push((eff_metric.clone(), eff_device.clone())));
                                        }
                                    },
                                    icons::Plus { class: "h-4 w-4 mr-1" }
                                    {t!("vis-add")}
                                }
                            }

                            // Séries actives (chips)
                            if !active.read().is_empty() {
                                div { class: "flex flex-wrap gap-2 mt-4",
                                    for (index, (metric, device, label)) in chips.iter().enumerate() {
                                        div {
                                            key: "{metric}-{device}",
                                            class: "inline-flex items-center gap-2 px-3 py-1.5 bg-gray-50 border border-gray-200 rounded-full text-sm",
                                            span {
                                                class: "h-2.5 w-2.5 rounded-full",
                                                style: "background-color: {PALETTE[index % MAX_SERIES]}",
                                            }
                                            span { class: "text-gray-700", "{label}" }
                                            button {
                                                class: "text-gray-400 hover:text-red-500",
                                                onclick: move |_| {
                                                    active
                                                        .with_mut(|list| {
                                                            list.remove(index);
                                                        })
                                                },
                                                icons::X { class: "h-3.5 w-3.5" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }

            // Courbe
            div { class: "bg-white rounded-lg shadow-sm",
                div { class: "p-6 border-b border-gray-200 flex items-center justify-between",
                    h2 { class: "text-lg font-semibold text-gray-900", {t!("vis-chart")} }
                    if total_points > 0 {
                        span { class: "text-xs text-gray-400", "{total_points}" }
                    }
                }
                div { class: "p-6",
                    if active.read().is_empty() {
                        p { class: "text-gray-500 text-center py-12", {t!("vis-empty")} }
                    } else {
                        TimeSeriesChart { series: chart_input }
                        if any_failed {
                            p { class: "text-sm text-gray-500 bg-gray-50 border border-gray-200 rounded-lg p-4 mt-4",
                                {t!("vis-unavailable")}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Garde de session (même retour silencieux que les autres pages).
fn session_absent() -> bool {
    crate::state::session::user().is_none()
}
