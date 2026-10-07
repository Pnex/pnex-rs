//! Live view of a mobile dashboard (D124, D139): page tabs, sections as
//! card grids, rooms (summary + "turn everything off") or chip rows,
//! cards hidden by their visibility rule, and a detail sheet with the
//! 24-hour history of the card sources. 2 columns on a phone, 3 to 4 on a
//! large screen.

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::home::HomeCard;
use pnex_core::{DashboardLayout, MobileSection, SectionStyle, TelemetryPoint, Widget};

use crate::components::charts::format_value;
use crate::components::confirm::ConfirmDialog;
use crate::components::dashboard_widget::WidgetBody;
use crate::components::home_icons::HomeIconView;
use crate::components::icons;
use crate::components::surface::home_card::{card_label, chip_summary};
use crate::components::surface::SurfaceControls;
use crate::state::toasts;

type Values = HashMap<String, Option<Vec<TelemetryPoint>>>;

/// Grid classes of a mobile card (D124): column span (1 = half, 2 = two
/// columns, default 2) and a height fitted to the widget type. Home cards
/// size to their content with a floor (O29: a fixed height left a light
/// card half empty); only the energy flow and the forecast keep a fixed
/// height. Charts keep theirs: their canvas fills the card.
pub fn mobile_card_classes(w: &Widget) -> &'static str {
    let half = w.options.span == Some(1);
    if let Some(home) = &w.options.home {
        return match (home.card, half) {
            (HomeCard::EnergyFlow | HomeCard::Weather, _) => "col-span-2 h-60",
            (HomeCard::Light | HomeCard::Thermostat | HomeCard::Cover | HomeCard::Alarm, true) => {
                "col-span-1 min-h-32"
            }
            (_, true) => "col-span-1 min-h-24",
            (_, false) => "col-span-2 min-h-20",
        };
    }
    match (w.widget_type.as_str(), half) {
        ("thermo_chart", _) => "col-span-2 h-80",
        ("gauge", true) => "col-span-1 h-40",
        ("gauge", false) => "col-span-2 h-48",
        ("line", true) => "col-span-1 h-36",
        ("line", false) => "col-span-2 h-40",
        ("slider" | "number" | "stepper" | "select" | "command", true) => "col-span-1 h-32",
        ("slider" | "number" | "stepper" | "select" | "command", false) => "col-span-2 h-32",
        ("text", true) => "col-span-1 min-h-20",
        ("text", false) => "col-span-2 min-h-20",
        (_, true) => "col-span-1 h-28",
        (_, false) => "col-span-2 h-28",
    }
}

/// Newest value of the first source of a widget.
fn first_value(w: &Widget, values: &Values) -> Option<f64> {
    let s = w.source.first()?;
    values
        .get(&s.series_key())
        .cloned()
        .flatten()
        .and_then(|p| p.last().map(|p| p.value))
}

/// Points of the first source and whether it is degraded (key present but
/// `None`: O2 unreachable for this series).
fn first_points(w: &Widget, values: &Values) -> (Option<Vec<TelemetryPoint>>, bool) {
    let Some(s) = w.source.first() else {
        return (None, false);
    };
    let points = values.get(&s.series_key()).cloned().flatten();
    let degraded = values.contains_key(&s.series_key()) && points.is_none();
    (points, degraded)
}

#[component]
pub fn LiveStack(layout: DashboardLayout, values: Values) -> Element {
    let mut page: Signal<Option<String>> = use_signal(|| None);
    let mut detail: Signal<Option<String>> = use_signal(|| None);
    let current = page()
        .filter(|p| layout.pages.iter().any(|x| &x.id == p))
        .or_else(|| layout.pages.first().map(|p| p.id.clone()));
    let sections: Vec<(MobileSection, Vec<Widget>)> = layout
        .sections
        .iter()
        .filter(|s| layout.page_of(s) == current.as_deref())
        .map(|sec| {
            let cards = sec
                .items
                .iter()
                .filter_map(|id| layout.widgets.iter().find(|w| &w.id == id).cloned())
                .filter(|w| pnex_core::card_visible(&w.options, first_value(w, &values)))
                .collect();
            (sec.clone(), cards)
        })
        .collect();
    let opened = detail().and_then(|id| layout.widgets.iter().find(|w| w.id == id).cloned());
    let tab_on = "flex shrink-0 items-center gap-1.5 rounded-full bg-gray-900 px-3 py-1.5 text-sm font-medium text-white";
    let tab_off = "flex shrink-0 items-center gap-1.5 rounded-full bg-white px-3 py-1.5 text-sm font-medium text-gray-600 shadow-sm";
    rsx! {
        div { class: "mx-auto w-full max-w-md space-y-5 md:max-w-3xl xl:max-w-6xl",
            if layout.pages.len() > 1 {
                nav { class: "flex gap-2 overflow-x-auto pb-1",
                    for p in layout.pages.iter().cloned() {
                        button {
                            key: "{p.id}",
                            r#type: "button",
                            class: if current.as_ref() == Some(&p.id) { tab_on } else { tab_off },
                            onclick: move |_| page.set(Some(p.id.clone())),
                            if let Some(icon) = p.icon.clone() {
                                HomeIconView { id: icon, class: "h-4 w-4" }
                            }
                            "{p.title}"
                        }
                    }
                }
            }
            for (sec, cards) in sections {
                LiveSection {
                    key: "{sec.id}",
                    section: sec.clone(),
                    cards,
                    values: values.clone(),
                    on_open: move |id: String| detail.set(Some(id)),
                }
            }
        }
        if let Some(w) = opened {
            DetailSheet {
                widget: w,
                values: values.clone(),
                on_close: move |_| detail.set(None),
            }
        }
    }
}

#[component]
fn LiveSection(
    section: MobileSection,
    cards: Vec<Widget>,
    values: Values,
    on_open: EventHandler<String>,
) -> Element {
    if cards.is_empty() && section.style == SectionStyle::Chips {
        return rsx! {};
    }
    let title = section.title.clone();
    let icon = section.icon.clone();
    rsx! {
        section { class: "space-y-2",
            if section.style == SectionStyle::Room {
                RoomHeader {
                    title: title.clone(),
                    icon: icon.clone(),
                    cards: cards.clone(),
                    values: values.clone(),
                }
            } else if !title.trim().is_empty() {
                h2 { class: "flex items-center gap-1.5 px-1 text-xs font-semibold uppercase tracking-wide text-gray-500",
                    if let Some(i) = icon.clone() {
                        HomeIconView { id: i, class: "h-4 w-4" }
                    }
                    "{title}"
                }
            }
            if section.style == SectionStyle::Chips {
                div { class: "flex gap-2 overflow-x-auto pb-1",
                    for w in cards {
                        Chip {
                            key: "{w.id}",
                            widget: w.clone(),
                            values: values.clone(),
                            on_open,
                        }
                    }
                }
            } else {
                div { class: "grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-6",
                    for w in cards {
                        StackCard {
                            key: "{w.id}",
                            w: w.clone(),
                            values: values.clone(),
                            on_open,
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StackCard(w: Widget, values: Values, on_open: EventHandler<String>) -> Element {
    let (points, degraded) = first_points(&w, &values);
    let classes = mobile_card_classes(&w);
    let fade = if degraded { "opacity-50" } else { "" };
    let id = w.id.clone();
    rsx! {
        div { class: "{classes} {fade} group relative overflow-hidden rounded-2xl border border-gray-200 bg-white shadow-sm",
            WidgetBody { widget: w.clone(), points, values: Some(values.clone()) }
            button {
                r#type: "button",
                class: "absolute right-1.5 top-1.5 rounded-full p-1 text-gray-300 hover:bg-gray-100 hover:text-gray-600",
                title: t!("db-detail-open").to_string(),
                aria_label: t!("db-detail-open"),
                onclick: move |_| on_open.call(id.clone()),
                icons::Maximize { class: "h-3.5 w-3.5" }
            }
        }
    }
}

/// Compact pill: icon + short state of a card (header summary).
#[component]
fn Chip(widget: Widget, values: Values, on_open: EventHandler<String>) -> Element {
    let (icon, text, tint) = chip_summary(&widget, &values);
    let style = tint
        .map(|c| format!("color: {c}"))
        .unwrap_or_else(|| "color: #4b5563".to_string());
    let id = widget.id.clone();
    rsx! {
        button {
            r#type: "button",
            class: "flex shrink-0 items-center gap-1.5 rounded-full border border-gray-200 bg-white px-3 py-1.5 text-sm shadow-sm",
            onclick: move |_| on_open.call(id.clone()),
            span { style: "{style}",
                HomeIconView { id: icon, class: "h-4 w-4" }
            }
            span { class: "font-medium text-gray-800", "{text}" }
        }
    }
}

/// Room summary of a section: mean temperature, lights on, total power.
struct RoomSummary {
    temperature: Option<f64>,
    lights_on: usize,
    power: Option<f64>,
    /// `(control id, off value)` of every switchable thing of the room.
    switch_offs: Vec<(uuid::Uuid, f64)>,
}

fn room_summary(
    cards: &[Widget],
    values: &Values,
    surface: Option<SurfaceControls>,
) -> RoomSummary {
    let last = |w: &Widget, role: &str| {
        w.source_of(role)
            .and_then(|s| values.get(&s.series_key()).cloned().flatten())
            .and_then(|p| p.last().map(|p| p.value))
    };
    let mut temps = Vec::new();
    let mut power = None::<f64>;
    let mut lights_on = 0;
    let mut switch_offs = Vec::new();
    for w in cards {
        let Some(home) = &w.options.home else {
            continue;
        };
        match home.card {
            HomeCard::ThermoHygro => temps.extend(last(w, "temperature")),
            HomeCard::Thermostat => temps.extend(last(w, "current")),
            HomeCard::Power => {
                if let Some(p) = last(w, "power") {
                    power = Some(power.unwrap_or(0.0) + p);
                }
            }
            _ => {}
        }
        let switchable = matches!(
            home.card,
            HomeCard::Light | HomeCard::Fan | HomeCard::Irrigation | HomeCard::Appliance
        );
        if let (true, Some(id), Some(spec)) =
            (switchable, home.control_of("power"), home.spec_of("power"))
        {
            switch_offs.push((id, spec.off_value()));
            let commanded = surface.and_then(|s| s.value(&id)).map(|v| v.v);
            let on = last(w, "state").or(commanded) == Some(spec.on_value());
            if on && home.card == HomeCard::Light {
                lights_on += 1;
            }
        }
    }
    RoomSummary {
        temperature: (!temps.is_empty()).then(|| temps.iter().sum::<f64>() / temps.len() as f64),
        lights_on,
        power,
        switch_offs,
    }
}

#[component]
fn RoomHeader(title: String, icon: Option<String>, cards: Vec<Widget>, values: Values) -> Element {
    let surface = try_use_context::<SurfaceControls>();
    let mut confirming = use_signal(|| false);
    let summary = room_summary(&cards, &values, surface);
    let mut facts = Vec::new();
    if let Some(t) = summary.temperature {
        facts.push(format!("{} °C", format_value(t, 1)));
    }
    if summary.lights_on > 0 {
        facts.push(t!("db-room-lights-on", count : summary.lights_on).to_string());
    }
    if let Some(p) = summary.power {
        facts.push(format!("{} W", format_value(p, 0)));
    }
    let facts = facts.join(" · ");
    let can_off = surface.is_some_and(|s| s.interactive) && !summary.switch_offs.is_empty();
    let offs = summary.switch_offs.clone();
    rsx! {
        div { class: "flex items-center gap-3 px-1",
            span { class: "flex h-9 w-9 items-center justify-center rounded-full bg-white text-gray-700 shadow-sm",
                HomeIconView {
                    id: icon.unwrap_or_else(|| "home-house".to_string()),
                    class: "h-5 w-5",
                }
            }
            div { class: "min-w-0 flex-1",
                p { class: "truncate text-sm font-semibold text-gray-900", "{title}" }
                if !facts.is_empty() {
                    p { class: "truncate text-xs text-gray-500", "{facts}" }
                }
            }
            if can_off {
                button {
                    r#type: "button",
                    class: "flex items-center gap-1 rounded-full bg-white px-3 py-1 text-xs font-medium text-gray-700 shadow-sm hover:bg-gray-50",
                    onclick: move |_| confirming.set(true),
                    HomeIconView { id: "home-power", class: "h-3.5 w-3.5" }
                    {t!("db-room-all-off")}
                }
            }
        }
        if confirming() {
            ConfirmDialog {
                title: t!("db-room-all-off").to_string(),
                message: t!("db-room-all-off-confirm", room : title.clone()).to_string(),
                confirm_label: t!("db-room-all-off").to_string(),
                on_confirm: move |_| {
                    confirming.set(false);
                    if let Some(surface) = surface {
                        all_off(surface, offs.clone());
                    }
                },
                on_cancel: move |_| confirming.set(false),
            }
        }
    }
}

/// Writes the off value of every switchable control of a room (each write
/// goes through the control gate, rate limit and flows as usual).
fn all_off(surface: SurfaceControls, offs: Vec<(uuid::Uuid, f64)>) {
    let via = surface.via.cloned();
    spawn(async move {
        for (id, off) in offs {
            match crate::api::controls::write_value(id, off, Some(via.clone())).await {
                Ok(value) => surface.record(id, value),
                Err(e) => toasts::error(e),
            }
        }
    });
}

/// Bottom sheet of a card: the card itself at full width plus the 24-hour
/// curve of each telemetry source.
#[component]
fn DetailSheet(widget: Widget, values: Values, on_close: EventHandler<()>) -> Element {
    let specs: Vec<pnex_core::SeriesSpec> = widget
        .source
        .iter()
        .filter(|s| s.memory.is_none() && !s.device_id.is_empty())
        .map(|s| pnex_core::SeriesSpec {
            metric: s.metric.clone(),
            device_id: s.device_id.clone(),
            window: "24h".into(),
        })
        .collect();
    let history = use_resource(move || {
        let specs = specs.clone();
        async move {
            if specs.is_empty() {
                return Vec::new();
            }
            crate::api::dashboards::series_batch(pnex_core::SeriesBatchRequest { specs })
                .await
                .map(|r| r.results)
                .unwrap_or_default()
        }
    });
    let curves = history.read().clone().unwrap_or_default();
    let title = if widget.title.trim().is_empty() {
        widget
            .options
            .home
            .as_ref()
            .map(|h| card_label(h.card))
            .unwrap_or_default()
    } else {
        widget.title.clone()
    };
    let (points, _) = first_points(&widget, &values);
    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-end justify-center bg-black/40 md:items-center",
            onclick: move |_| on_close.call(()),
            div {
                class: "max-h-[90vh] w-full max-w-lg overflow-y-auto rounded-t-2xl bg-gray-50 p-4 md:rounded-2xl",
                role: "dialog",
                aria_modal: "true",
                aria_label: "{title}",
                onclick: move |e| e.stop_propagation(),
                div { class: "mb-3 flex items-center justify-between",
                    h2 { class: "text-base font-semibold text-gray-900", "{title}" }
                    button {
                        r#type: "button",
                        class: "rounded-full p-1 text-gray-400 hover:bg-gray-200",
                        aria_label: t!("common-close"),
                        onclick: move |_| on_close.call(()),
                        icons::X { class: "h-5 w-5" }
                    }
                }
                div { class: "min-h-48 overflow-hidden rounded-2xl border border-gray-200 bg-white",
                    WidgetBody {
                        widget: widget.clone(),
                        points,
                        values: Some(values.clone()),
                    }
                }
                for c in curves {
                    HistoryCurve {
                        key: "{c.metric}|{c.device_id}",
                        label: format!("{} · {}", c.metric, c.device_id),
                        points: c.points.clone(),
                    }
                }
            }
        }
    }
}

/// 24-hour curve with its min / max (detail sheet).
#[component]
fn HistoryCurve(label: String, points: Vec<TelemetryPoint>) -> Element {
    if points.len() < 2 {
        return rsx! {};
    }
    let (mut t0, mut t1, mut v0, mut v1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in &points {
        t0 = t0.min(p.ts);
        t1 = t1.max(p.ts);
        v0 = v0.min(p.value);
        v1 = v1.max(p.value);
    }
    let (ts, vs) = ((t1 - t0).max(1.0), (v1 - v0).max(1e-9));
    let path = points
        .iter()
        .map(|p| {
            let x = (p.ts - t0) / ts * 300.0;
            let y = 76.0 - (p.value - v0) / vs * 72.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let (min, max) = (format_value(v0, 1), format_value(v1, 1));
    rsx! {
        div { class: "mt-3 rounded-2xl border border-gray-200 bg-white p-3",
            div { class: "mb-1 flex items-baseline justify-between text-xs text-gray-500",
                span { class: "truncate", "{label}" }
                span { {t!("db-detail-range", min : min, max : max)} }
            }
            svg {
                class: "h-20 w-full",
                view_box: "0 0 300 80",
                "preserveAspectRatio": "none",
                polyline {
                    points: "{path}",
                    fill: "none",
                    stroke: "#0d9488",
                    "stroke-width": "2",
                    "vector-effect": "non-scaling-stroke",
                }
            }
            p { class: "text-[10px] text-gray-400", {t!("db-detail-24h")} }
        }
    }
}
