//! Visualisation (2026-08-19) — sensor-by-sensor curves from
//! OpenObserve: catalogue of the org's series (`/api/v1/telemetry/catalog`)
//! then points per series over a preset window (`/api/v1/telemetry/series`).
//!
//! Home-made SVG chart (no lib): polyline + points, global Y scale
//! over all active series — quick and dirty, goal = validate the
//! collection mechanics end to end. Up to 6 overlaid series,
//! 15 s polling (see the data arrive live), degraded telemetry as a
//! notice (never a looping toast).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::TelemetryPoint;

use crate::api;
use crate::components::charts::{ChartSeries, TimeSeriesChart, PALETTE};
use crate::components::crud::layout::ListLayout;
use crate::components::icons;
use crate::components::ui::FIELD_LABEL;
use crate::state::org;
use crate::util::sleep;

/// Refresh rate (same choice as the dashboard: see the
/// data arrive without hammering O2).
const POLL_SECS: u64 = 15;

/// Max overlaid series (palette size).
const MAX_SERIES: usize = 6;

/// Offered preset windows (API key, i18n label, seconds) —
/// identical to `WINDOWS` on the backend side.
const WINDOWS: &[(&str, &str, i64)] = &[
    ("1h", "vis-window-1h", 3600),
    ("6h", "vis-window-6h", 21_600),
    ("24h", "vis-window-24h", 86_400),
];

/// A series active on the page: add key + loaded points.
#[derive(Clone)]
struct ActiveSeries {
    metric: String,
    device_id: String,
    /// `None` = failed or degraded request (global notice).
    points: Option<Vec<TelemetryPoint>>,
}

#[component]
pub fn Visualisation() -> Element {
    if session_absent() {
        return rsx! {};
    }

    // Current picker selection + active series + window.
    let mut reload = use_signal(|| 0u32);
    let mut window = use_signal(|| 86_400i64);
    let mut sel_metric = use_signal(String::new);
    let mut sel_device = use_signal(String::new);
    let mut active: Signal<Vec<(String, String)>> = use_signal(Vec::new);
    let mut polling = use_signal(|| false);

    // Catalogue: re-run on org change or on reload (polling +
    // button). Errors swallowed → None (the page stays usable).
    let catalog = use_resource(move || async move {
        let _ = reload();
        org::current()?;
        api::telemetry::catalog().await.ok()
    });
    let cat = match catalog.read().as_ref() {
        Some(inner) => inner.clone(),
        None => None,
    };

    // Points: one request per active series (≤ 6), sequential —
    // trivial load at the polling rate. The signals are read in the
    // SYNCHRONOUS part of the closure (devices.rs pattern): the re-run
    // subscription is guaranteed on active/window/reload.
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

    // Self-sustaining polling while the page is mounted.
    if !polling() {
        polling.set(true);
        spawn(async move {
            sleep(Duration::from_secs(POLL_SECS)).await;
            polling.set(false);
            reload.with_mut(|r| *r += 1);
        });
    }

    // ── Everything is precomputed here: rsx children are evaluated
    // lazily, we do not dig into the Options inside.
    // Distinct metrics of the catalogue (sorted backend side).
    let metrics: Vec<String> = cat
        .as_ref()
        .map(|c| {
            let mut names: Vec<String> = c.series.iter().map(|s| s.metric.clone()).collect();
            names.sort();
            names.dedup();
            names
        })
        .unwrap_or_default();
    // EFFECTIVE selections, computed — never a `set` during render: the
    // Dioxus set notifies without comparing values, mutating during
    // render while the catalogue was empty looped forever and
    // froze the whole app (user feedback 2026-08-19: "all pages
    // frozen"). Controlled-select pitfall fixed by the same pass:
    // without an effective value, the select SHOWS its first option without
    // it being "selected" (Add button stayed disabled,
    // no /series call in the server logs).
    let cur_metric = sel_metric();
    let eff_metric = if metrics.contains(&cur_metric) {
        cur_metric
    } else {
        metrics.first().cloned().unwrap_or_default()
    };
    // Devices of the effective metric.
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

    // Loading the points: flatten of the resource.
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
        // Canonical list header: same refresh spot as every other page.
        ListLayout {
            title: t!("nav-quick-charts").to_string(),
            subtitle: Some(t!("vis-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: false,
            actions: rsx! {
                span { class: "hidden text-xs text-gray-500 sm:inline", {t!("dash-auto-refresh")} }
            },

            // Selection: metric × device × window + add
            div { class: "bg-white rounded-lg border border-line mb-8",
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
                                    label {
                                        r#for: "visualisation-field-1",
                                        class: FIELD_LABEL,
                                        {t!("vis-metric")}
                                    }
                                    select {
                                        id: "visualisation-field-1",
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
                                    label {
                                        r#for: "visualisation-field-2",
                                        class: FIELD_LABEL,
                                        {t!("vis-device")}
                                    }
                                    select {
                                        id: "visualisation-field-2",
                                        aria_label: t!("vis-device"),
                                        class: "px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                                        onchange: move |event| sel_device.set(event.value()),
                                        for device in &devices {
                                            option { value: "{device}", selected: eff_device == *device, {device.clone()} }
                                        }
                                    }
                                }
                                div {
                                    p {
                                        id: "visualisation-window-label",
                                        class: "block text-xs font-medium text-gray-500 mb-1",
                                        {t!("vis-window")}
                                    }
                                    div {
                                        class: "flex rounded-lg border border-gray-300 overflow-hidden",
                                        role: "group",
                                        aria_labelledby: "visualisation-window-label",
                                        for (key, label, secs) in WINDOWS {
                                            button {
                                                key: "{key}",
                                                class: if window() == *secs { "px-3 py-2 text-sm bg-signal-600 text-white" } else { "px-3 py-2 text-sm bg-white text-gray-700 hover:bg-gray-50" },
                                                onclick: move |_| window.set(*secs),
                                                {t!(label)}
                                            }
                                        }
                                    }
                                }
                                button {
                                    class: if can_add { "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-signal-600 rounded-lg hover:bg-signal-700 transition-colors" } else { "inline-flex items-center px-4 py-2 text-sm font-medium text-white bg-signal-600 rounded-lg opacity-40 cursor-not-allowed" },
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

                            // Active series (chips)
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
                                                class: "text-gray-500 hover:text-red-500",
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

            // Curve
            div { class: "bg-white rounded-lg border border-line",
                div { class: "p-6 border-b border-gray-200 flex items-center justify-between",
                    h2 { class: "text-lg font-semibold text-gray-900", {t!("vis-chart")} }
                    if total_points > 0 {
                        span { class: "text-xs text-gray-500", "{total_points}" }
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

/// Session guard (same silent return as the other pages).
fn session_absent() -> bool {
    crate::state::session::user().is_none()
}
