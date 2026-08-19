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

use chrono::Local;
use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TelemetryPoint;

use crate::api;
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

/// Palette des séries (couleurs littérales — jamais de classe
/// construite dynamiquement pour le scanner Tailwind ; ce sont des
/// attributs SVG `stroke`, pas des classes).
const PALETTE: [&str; MAX_SERIES] = [
    "#0d9488", "#4f46e5", "#db2777", "#f59e0b", "#0284c7", "#65a30d",
];

/// Géométrie du chart (viewBox) — padding pour les labels Y.
const CHART_W: f64 = 800.0;
const CHART_H: f64 = 280.0;
const PAD_L: f64 = 48.0;
const PAD_R: f64 = 16.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 24.0;

/// Une série active côté page : clé d'ajout + points chargés.
#[derive(Clone)]
struct ActiveSeries {
    metric: String,
    device_id: String,
    /// `None` = requête en échec ou dégradée (encart global).
    points: Option<Vec<TelemetryPoint>>,
}

/// Formatage heure locale d'un timestamp epoch secondes (axe X).
fn time_label(ts: f64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".into())
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
    // Devices de la métrique sélectionnée.
    let devices: Vec<String> = cat
        .as_ref()
        .map(|c| {
            c.series
                .iter()
                .filter(|s| s.metric == sel_metric())
                .map(|s| s.device_id.clone())
                .collect()
        })
        .unwrap_or_default();
    // Sélections par défaut, piège des selects contrôlés : sans valeur
    // portée par le signal, le select AFFICHE sa première option sans
    // qu'elle soit « sélectionnée » — le bouton Ajouter restait grisé
    // alors que l'utilisateur voyait un capteur (retour user 2026-08-19 :
    // « rien ne s'affiche sur les graphes », aucun appel /series dans les
    // logs). On ancre métrique ET device sur le premier élément valide.
    if !metrics.contains(&sel_metric()) {
        sel_metric.set(metrics.first().cloned().unwrap_or_default());
    }
    if !devices.contains(&sel_device()) {
        sel_device.set(devices.first().cloned().unwrap_or_default());
    }
    let can_add = !sel_metric().is_empty()
        && !sel_device().is_empty()
        && active.read().len() < MAX_SERIES
        && !active
            .read()
            .iter()
            .any(|(m, d)| *m == sel_metric() && *d == sel_device());

    // Chargement des points : flatten de la ressource.
    let loaded = match series.read().as_ref() {
        Some(list) => list.clone(),
        None => Vec::new(),
    };
    let any_failed = !loaded.is_empty() && loaded.iter().any(|s| s.points.is_none());
    let chart_input: Vec<(usize, &ActiveSeries)> = loaded.iter().enumerate().collect();
    let total_points: usize = loaded
        .iter()
        .filter_map(|s| s.points.as_ref().map(|p| p.len()))
        .sum();

    rsx! {
        div { class: "p-6",
            div { class: "mb-8 flex items-center justify-between",
                div {
                    h1 { class: "text-3xl font-bold text-gray-900", {t!("nav-visualisation")} }
                    p { class: "text-gray-600 mt-2", {t!("vis-subtitle")} }
                }
                div { class: "flex items-center gap-3",
                    span { class: "text-xs text-gray-400", {t!("dash-auto-refresh")} }
                    button {
                        class: "inline-flex items-center px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                        onclick: move |_| reload.with_mut(|r| *r += 1),
                        icons::RefreshCw { class: "h-4 w-4 mr-2" }
                        {t!("common-retry")}
                    }
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
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| {
                                            sel_metric.set(event.value());
                                            sel_device.set(String::new());
                                        },
                                        for metric in &metrics {
                                            option { value: "{metric}", selected: sel_metric() == *metric, {metric.clone()} }
                                        }
                                    }
                                }
                                div {
                                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", {t!("vis-device")} }
                                    select {
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| sel_device.set(event.value()),
                                        for device in &devices {
                                            option { value: "{device}", selected: sel_device() == *device, {device.clone()} }
                                        }
                                    }
                                }
                                div {
                                    label { class: "block text-xs font-medium text-gray-500 uppercase mb-1", {t!("vis-window")} }
                                    div { class: "flex rounded-lg border border-gray-300 overflow-hidden",
                                        for (key, label, secs) in WINDOWS {
                                            button {
                                                key: "{key}",
                                                class: if window() == *secs {
                                                    "px-3 py-2 text-sm bg-blue-600 text-white"
                                                } else {
                                                    "px-3 py-2 text-sm bg-white text-gray-700 hover:bg-gray-50"
                                                },
                                                onclick: move |_| window.set(*secs),
                                                {t!(label)}
                                            }
                                        }
                                    }
                                }
                                button {
                                    class: if can_add {
                                        "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg hover:bg-blue-700 transition-colors"
                                    } else {
                                        "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-lg opacity-40 cursor-not-allowed"
                                    },
                                    disabled: !can_add,
                                    onclick: move |_| {
                                        let (m, d) = (sel_metric(), sel_device());
                                        if can_add {
                                            active.with_mut(|list| list.push((m, d)));
                                        }
                                    },
                                    icons::Plus { class: "h-4 w-4 mr-1" }
                                    {t!("vis-add")}
                                }
                            }

                            // Séries actives (chips)
                            if !active.read().is_empty() {
                                div { class: "flex flex-wrap gap-2 mt-4",
                                    for (index, (metric, device)) in active.read().iter().enumerate() {
                                        div { key: "{metric}-{device}",
                                            class: "inline-flex items-center gap-2 px-3 py-1.5 bg-gray-50 border border-gray-200 rounded-full text-sm",
                                            span { class: "h-2.5 w-2.5 rounded-full", style: "background-color: {PALETTE[index % MAX_SERIES]}" }
                                            span { class: "text-gray-700", "{metric} · {device}" }
                                            button {
                                                class: "text-gray-400 hover:text-red-500",
                                                onclick: move |_| active.with_mut(|list| { list.remove(index); }),
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
                        {chart(chart_input)}
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

/// Chart SVG : une polyline + ses cercles par série, échelle Y globale.
/// Toute la géométrie est calculée ICI (rsx à évaluation paresseuse).
fn chart(series: Vec<(usize, &ActiveSeries)>) -> Element {
    // Fenêtre temporelle globale + amplitude Y globale.
    let mut t_min = f64::MAX;
    let mut t_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for (_, s) in &series {
        if let Some(points) = &s.points {
            for p in points {
                t_min = t_min.min(p.ts);
                t_max = t_max.max(p.ts);
                v_min = v_min.min(p.value);
                v_max = v_max.max(p.value);
            }
        }
    }
    // Aucun point nulle part : état vide.
    if t_min > t_max {
        return rsx! {
            p { class: "text-gray-500 text-center py-12", {t!("vis-no-points")} }
        };
    }
    let t_span = (t_max - t_min).max(1.0);
    // Amplitude nulle (série plate) : on dessine au centre.
    let v_span = (v_max - v_min).max(1e-9);
    let x = |ts: f64| PAD_L + (ts - t_min) / t_span * (CHART_W - PAD_L - PAD_R);
    let y = |v: f64| PAD_T + (1.0 - (v - v_min) / v_span) * (CHART_H - PAD_T - PAD_B);

    // Une polyline (points "x,y x,y …") + cercles par point, précalculés.
    #[allow(clippy::type_complexity)]
    let paths: Vec<(usize, String, Vec<(f64, f64)>)> = series
        .iter()
        .filter_map(|(index, s)| {
            let points = s.points.as_ref()?;
            if points.is_empty() {
                return None;
            }
            let coords: Vec<(f64, f64)> = points
                .iter()
                .map(|p| {
                    // Un seul point (ou plusieurs au même ts) : x centré
                    // pour rester visible.
                    let px = if t_span <= 1.0 {
                        (PAD_L + CHART_W - PAD_R) / 2.0
                    } else {
                        x(p.ts)
                    };
                    (px, y(p.value))
                })
                .collect();
            let path = coords
                .iter()
                .map(|(px, py)| format!("{px:.1},{py:.1}"))
                .collect::<Vec<_>>()
                .join(" ");
            Some((*index, path, coords))
        })
        .collect();

    // Grille : 4 lignes horizontales + labels min/max.
    let grid_ys: Vec<f64> = (0..=3)
        .map(|i| PAD_T + (CHART_H - PAD_T - PAD_B) * (i as f64 / 3.0))
        .collect();
    let y_max_label = format!("{v_max:.1}");
    let y_min_label = format!("{v_min:.1}");
    let t_start_label = time_label(t_min);
    let t_end_label = time_label(t_max);

    rsx! {
        // Légende (couleurs de la palette, même ordre que les courbes)
        div { class: "flex flex-wrap gap-4 mb-4",
            for (index, s) in &series {
                div { key: "{s.metric}-{s.device_id}", class: "flex items-center gap-2 text-sm text-gray-700",
                    span { class: "h-2.5 w-2.5 rounded-full", style: "background-color: {PALETTE[*index % MAX_SERIES]}" }
                    "{s.metric} · {s.device_id}"
                }
            }
        }
        svg {
            view_box: "0 0 {CHART_W} {CHART_H}",
            class: "w-full h-auto",
            xmlns: "http://www.w3.org/2000/svg",
            // Grille
            for gy in &grid_ys {
                line { x1: "{PAD_L}", y1: "{gy}", x2: "{CHART_W - PAD_R}", y2: "{gy}",
                    stroke: "#e5e7eb", "stroke-width": "1" }
            }
            // Labels Y (max en haut, min en bas)
            text { x: "{PAD_L - 6.0}", y: "{PAD_T + 4.0}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {y_max_label} }
            text { x: "{PAD_L - 6.0}", y: "{CHART_H - PAD_B}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {y_min_label} }
            // Labels X (début / fin de fenêtre, heure locale)
            text { x: "{PAD_L}", y: "{CHART_H - 6.0}",
                class: "fill-gray-400", style: "font-size: 11px", {t_start_label} }
            text { x: "{CHART_W - PAD_R}", y: "{CHART_H - 6.0}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {t_end_label} }
            // Séries
            for (index, path, coords) in &paths {
                polyline {
                    points: "{path}",
                    fill: "none",
                    stroke: "{PALETTE[*index % MAX_SERIES]}",
                    "stroke-width": "2",
                    "stroke-linejoin": "round",
                    "stroke-linecap": "round"
                }
                for (px, py) in coords {
                    circle { cx: "{px}", cy: "{py}", r: "2.5",
                        fill: "{PALETTE[*index % MAX_SERIES]}" }
                }
            }
        }
    }
}
