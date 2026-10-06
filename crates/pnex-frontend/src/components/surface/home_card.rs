//! Home card bodies (D138): one composite card per [`HomeCard`] kind,
//! reading its sources by role from the live value map and driving its
//! role controls through [`ControlBody`] in compact mode (same gate,
//! confirmation and write path as a control widget).

use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::home::{HomeCard, HomeCardOptions};
use pnex_core::ui_control::{ControlRef, ControlSpec};
use pnex_core::{TelemetryPoint, Widget, WidgetOptions};

use super::control::ControlBody;
use super::SurfaceControls;
use crate::components::charts::format_value;
use crate::components::home_icons::HomeIconView;

/// Live values keyed by `SourceRef::series_key`.
pub type LiveValues = HashMap<String, Option<Vec<TelemetryPoint>>>;

/// Points of the source of `role`.
fn points_of(w: &Widget, role: &str, values: &LiveValues) -> Option<Vec<TelemetryPoint>> {
    let s = w.source_of(role)?;
    values.get(&s.series_key()).cloned().flatten()
}

/// Timestamp of the newest point across all the card's sources.
fn newest_ts(w: &Widget, values: &LiveValues) -> Option<f64> {
    w.source
        .iter()
        .filter_map(|s| values.get(&s.series_key()).cloned().flatten())
        .filter_map(|pts| pts.last().map(|p| p.ts))
        .reduce(f64::max)
}

/// Newest point of the source of `role`.
fn last_of(w: &Widget, role: &str, values: &LiveValues) -> Option<TelemetryPoint> {
    points_of(w, role, values).and_then(|p| p.last().cloned())
}

fn fmt(v: Option<f64>, decimals: u8) -> String {
    v.map(|v| format_value(v, decimals))
        .unwrap_or_else(|| "—".into())
}

/// Commanded value of the control of `role`. `try_consume_context`, not
/// the hook: this runs conditionally during render.
fn commanded(home: &HomeCardOptions, role: &str) -> Option<f64> {
    let id = home.control_of(role)?;
    let surface = try_consume_context::<SurfaceControls>()?;
    surface.value(&id).map(|v| v.v)
}

/// Synthetic control widget of one role, rendered by [`ControlBody`].
fn role_widget(w: &Widget, home: &HomeCardOptions, role: &str) -> Option<Widget> {
    let r = home.card.control_role(role)?;
    if !r.required && !home.specs.contains_key(role) {
        return None;
    }
    let spec: Option<ControlSpec> = home.spec_of(role);
    Some(Widget {
        id: format!("{}.{role}", w.id),
        widget_type: r.kind.widget_type().to_string(),
        title: String::new(),
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        source: vec![],
        options: WidgetOptions {
            control: home
                .control_of(role)
                .map(|control_id| ControlRef { control_id }),
            control_spec: spec,
            decimals: w.options.decimals,
            ..Default::default()
        },
    })
}

/// Localized card kind name (palette, inspector).
pub fn card_label(card: HomeCard) -> String {
    match card {
        HomeCard::Light => t!("hcard-light").to_string(),
        HomeCard::Thermostat => t!("hcard-thermostat").to_string(),
        HomeCard::Fan => t!("hcard-fan").to_string(),
        HomeCard::Cover => t!("hcard-cover").to_string(),
        HomeCard::Gate => t!("hcard-gate").to_string(),
        HomeCard::Lock => t!("hcard-lock").to_string(),
        HomeCard::Alarm => t!("hcard-alarm").to_string(),
        HomeCard::Scene => t!("hcard-scene").to_string(),
        HomeCard::Irrigation => t!("hcard-irrigation").to_string(),
        HomeCard::Binary => t!("hcard-binary").to_string(),
        HomeCard::ThermoHygro => t!("hcard-thermo_hygro").to_string(),
        HomeCard::AirQuality => t!("hcard-air_quality").to_string(),
        HomeCard::Power => t!("hcard-power").to_string(),
        HomeCard::Meter => t!("hcard-meter").to_string(),
        HomeCard::EnergyFlow => t!("hcard-energy_flow").to_string(),
        HomeCard::Appliance => t!("hcard-appliance").to_string(),
        HomeCard::Clock => t!("hcard-clock").to_string(),
        HomeCard::Weather => t!("hcard-weather").to_string(),
    }
}

/// Localized role name (inspector, provisioned control labels).
pub fn role_label(role: &str) -> String {
    dioxus_i18n::prelude::i18n()
        .try_translate(&format!("hrole-{role}"))
        .unwrap_or_else(|_| role.to_string())
}

/// On / off wording of a binary sensor variant.
fn binary_text(variant: &str, on: bool) -> String {
    match (variant, on) {
        ("door" | "window", true) => t!("hbin-open").to_string(),
        ("door" | "window", false) => t!("hbin-closed").to_string(),
        ("motion", true) => t!("hbin-motion").to_string(),
        ("motion", false) => t!("hbin-calm").to_string(),
        ("presence", true) => t!("hbin-present").to_string(),
        ("presence", false) => t!("hbin-absent").to_string(),
        ("smoke", true) => t!("hbin-smoke").to_string(),
        ("leak", true) => t!("hbin-leak").to_string(),
        ("leak", false) => t!("hbin-dry").to_string(),
        ("co", true) => t!("hbin-co").to_string(),
        ("smoke" | "co", false) => t!("hbin-ok").to_string(),
        (_, true) => t!("controls-on").to_string(),
        (_, false) => t!("controls-off").to_string(),
    }
}

/// Is an "on" binary state an alarm (red) rather than just active (amber)?
fn binary_alarm(variant: &str) -> bool {
    matches!(variant, "smoke" | "leak" | "co")
}

#[component]
pub fn HomeCardBody(widget: Widget, values: Option<LiveValues>) -> Element {
    let Some(home) = widget.options.home.clone() else {
        return rsx! {};
    };
    let values = values.unwrap_or_default();
    // Stale value (D135): the newest point of the card's sources is older
    // than "Stale after" — the card greys out like the classic widgets.
    let stale = newest_ts(&widget, &values)
        .is_some_and(|ts| pnex_core::is_stale(&widget.options, ts, crate::util::now_secs()));
    let w = widget.clone();
    let title = if w.title.trim().is_empty() {
        card_label(home.card)
    } else {
        w.title.clone()
    };
    let icon = w
        .options
        .icon
        .clone()
        .unwrap_or_else(|| home.card.default_icon(home.variant.as_deref()).to_string());
    let body = match home.card {
        HomeCard::Light => rsx! {
            LightCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Thermostat => rsx! {
            ThermostatCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Fan | HomeCard::Irrigation => rsx! {
            SwitchedCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Cover => rsx! {
            CoverCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Gate | HomeCard::Lock | HomeCard::Alarm => rsx! {
            GuardedCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Scene => rsx! {
            SceneCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
            }
        },
        HomeCard::Binary => rsx! {
            BinaryCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::ThermoHygro | HomeCard::AirQuality | HomeCard::Power | HomeCard::Meter => rsx! {
            ReadingCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::EnergyFlow => rsx! {
            EnergyFlowCard { widget: w.clone(), title, values }
        },
        HomeCard::Appliance => rsx! {
            ApplianceCard {
                widget: w.clone(),
                home: home.clone(),
                title,
                icon,
                values,
            }
        },
        HomeCard::Clock => rsx! {
            ClockCard {}
        },
        HomeCard::Weather => rsx! {
            WeatherCard { widget: w.clone(), title, values }
        },
    };
    // A required source still to pick (templates, D141).
    let to_configure = home
        .card
        .source_roles()
        .iter()
        .any(|r| r.required && widget.source_of(r.role).is_none_or(|s| s.is_unset()));
    rsx! {
        div {
            class: if stale { "flex h-full w-full flex-col gap-2 overflow-hidden rounded-lg p-3 opacity-50 grayscale" } else { "flex h-full w-full flex-col gap-2 overflow-hidden rounded-lg p-3" },
            title: if stale { t!("db-stale").to_string() } else { String::new() },
            {body}
            if to_configure {
                p { class: "mt-auto text-[11px] font-medium text-amber-700",
                    {t!("hcard-to-configure")}
                }
            }
        }
    }
}

/// Card header: tinted icon, title, state text, optional right element.
#[component]
fn Header(
    icon: String,
    title: String,
    state: String,
    /// `#rrggbb` tint of the icon and its halo (`None` = gray).
    tint: Option<String>,
    #[props(default)] right: Option<Element>,
) -> Element {
    let halo = match &tint {
        Some(c) => format!("background-color: {c}22; color: {c}"),
        None => "background-color: #f3f4f6; color: #6b7280".to_string(),
    };
    rsx! {
        div { class: "flex items-center gap-3",
            span {
                class: "flex h-10 w-10 shrink-0 items-center justify-center rounded-full",
                style: "{halo}",
                HomeIconView { id: icon, class: "h-5 w-5" }
            }
            div { class: "min-w-0 flex-1",
                p { class: "truncate text-sm font-medium text-gray-900", "{title}" }
                p { class: "truncate text-xs text-gray-500", "{state}" }
            }
            if let Some(r) = right {
                div { class: "shrink-0", {r} }
            }
        }
    }
}

/// Compact control of a role (nothing when the role is inactive).
#[component]
fn Role(widget: Widget, home: HomeCardOptions, role: String) -> Element {
    match role_widget(&widget, &home, &role) {
        Some(rw) => rsx! {
            ControlBody { widget: rw, state: None, compact: true }
        },
        None => rsx! {},
    }
}

const AMBER: &str = "#f59e0b";
const TEAL: &str = "#0d9488";
const RED: &str = "#dc2626";
const BLUE: &str = "#2563eb";

#[component]
fn LightCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let power = home.spec_of("power").map(|s| s.on_value()).unwrap_or(1.0);
    let actual = last_of(&widget, "state", &values).map(|p| p.value);
    let on = actual.or_else(|| commanded(&home, "power")) == Some(power);
    let level = commanded(&home, "level");
    let state = match (on, level) {
        (true, Some(l)) => format!("{} · {} %", t!("controls-on"), format_value(l, 0)),
        (true, None) => t!("controls-on").to_string(),
        (false, _) => t!("controls-off").to_string(),
    };
    let right = rsx! {
        Role { widget: widget.clone(), home: home.clone(), role: "power" }
    };
    rsx! {
        Header {
            icon,
            title,
            state,
            tint: on.then(|| AMBER.to_string()),
            right,
        }
        Role { widget: widget.clone(), home: home.clone(), role: "level" }
        Role { widget: widget.clone(), home: home.clone(), role: "color" }
    }
}

#[component]
fn ThermostatCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let decimals = widget.options.decimals.unwrap_or(1);
    let current = last_of(&widget, "current", &values).map(|p| p.value);
    let humidity = last_of(&widget, "humidity", &values).map(|p| p.value);
    let unit = home
        .spec_of("setpoint")
        .and_then(|s| s.unit)
        .unwrap_or_else(|| "°C".into());
    let mut state = match current {
        Some(v) => {
            t!("hcard-current", value : format!("{} {unit}", format_value(v, decimals))).to_string()
        }
        None => String::new(),
    };
    if let Some(h) = humidity {
        state = format!("{state} · {} %", format_value(h, 0));
    }
    let heating = match (current, commanded(&home, "setpoint")) {
        (Some(c), Some(sp)) if sp > c + 0.2 => Some(AMBER.to_string()),
        (Some(c), Some(sp)) if sp < c - 0.2 => Some(BLUE.to_string()),
        (Some(_), Some(_)) => Some(TEAL.to_string()),
        _ => None,
    };
    rsx! {
        Header {
            icon,
            title,
            state,
            tint: heating,
        }
        Role { widget: widget.clone(), home: home.clone(), role: "setpoint" }
        Role { widget: widget.clone(), home: home.clone(), role: "mode" }
    }
}

/// Fan and irrigation: a power switch plus an optional second control.
#[component]
fn SwitchedCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let power = home.spec_of("power").map(|s| s.on_value()).unwrap_or(1.0);
    let actual = last_of(&widget, "state", &values).map(|p| p.value);
    let on = actual.or_else(|| commanded(&home, "power")) == Some(power);
    let mut state = if on {
        t!("controls-on").to_string()
    } else {
        t!("controls-off").to_string()
    };
    if let Some(m) = last_of(&widget, "moisture", &values) {
        state = format!("{state} · {} %", format_value(m.value, 0));
    }
    let second = if home.card == HomeCard::Fan {
        "speed"
    } else {
        "duration"
    };
    let right = rsx! {
        Role { widget: widget.clone(), home: home.clone(), role: "power" }
    };
    rsx! {
        Header {
            icon,
            title,
            state,
            tint: on.then(|| TEAL.to_string()),
            right,
        }
        Role { widget: widget.clone(), home: home.clone(), role: second }
    }
}

#[component]
fn CoverCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let position = last_of(&widget, "position", &values)
        .map(|p| p.value)
        .or_else(|| commanded(&home, "position"));
    let state = match position {
        Some(p) if p <= 0.5 => t!("hbin-closed").to_string(),
        Some(p) if p >= 99.5 => t!("hbin-open").to_string(),
        Some(p) => format!("{} %", format_value(p, 0)),
        None => String::new(),
    };
    let tint = position.filter(|p| *p > 0.5).map(|_| TEAL.to_string());
    rsx! {
        Header {
            icon,
            title,
            state,
            tint,
        }
        Role { widget: widget.clone(), home: home.clone(), role: "command" }
        Role { widget: widget.clone(), home: home.clone(), role: "position" }
    }
}

/// Gate, lock, alarm: the state source (else the last command) named by
/// the option of the same value, commands confirmed by default.
#[component]
fn GuardedCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let role = if home.card == HomeCard::Alarm {
        "mode"
    } else {
        "command"
    };
    let spec = home.spec_of(role);
    let value = last_of(&widget, "state", &values)
        .map(|p| p.value)
        .or_else(|| commanded(&home, role));
    let option = value.and_then(|v| spec.as_ref().and_then(|s| s.option_of(v).cloned()));
    let state = option.as_ref().map(super::option_label).unwrap_or_default();
    let key = option.as_ref().map(|o| o.key.as_str()).unwrap_or("");
    let tint = match key {
        "lock" | "close" => Some(TEAL.to_string()),
        "unlock" | "open" => Some(AMBER.to_string()),
        "arm_home" | "arm_away" | "arm_night" => Some(RED.to_string()),
        "disarm" => Some(TEAL.to_string()),
        _ => None,
    };
    let icon = option.as_ref().and_then(|o| o.icon.clone()).unwrap_or(icon);
    rsx! {
        Header {
            icon,
            title,
            state,
            tint,
        }
        Role { widget: widget.clone(), home: home.clone(), role }
    }
}

#[component]
fn SceneCard(widget: Widget, home: HomeCardOptions, title: String, icon: String) -> Element {
    rsx! {
        Header {
            icon,
            title,
            state: String::new(),
            tint: Some(TEAL.to_string()),
        }
        Role { widget: widget.clone(), home: home.clone(), role: "trigger" }
    }
}

#[component]
fn BinaryCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let variant = home.variant.clone().unwrap_or_else(|| "generic".into());
    let point = last_of(&widget, "state", &values);
    let on = point.as_ref().map(|p| home.is_on(p.value));
    let state = match on {
        Some(on) => binary_text(&variant, on),
        None => "—".into(),
    };
    let tint = match on {
        Some(true) if binary_alarm(&variant) => Some(RED.to_string()),
        Some(true) => Some(AMBER.to_string()),
        Some(false) if binary_alarm(&variant) => Some(TEAL.to_string()),
        _ => None,
    };
    rsx! {
        Header {
            icon,
            title,
            state,
            tint,
        }
    }
}

/// Air quality zone colour of a CO₂ concentration (ppm).
fn co2_color(ppm: f64) -> &'static str {
    if ppm < 800.0 {
        "#16a34a"
    } else if ppm < 1_200.0 {
        AMBER
    } else {
        RED
    }
}

/// Thermo-hygro, air quality, power, meter: one big value plus details.
#[component]
fn ReadingCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let decimals = widget.options.decimals.unwrap_or(1);
    let unit_or = |d: &str| widget.options.unit.clone().unwrap_or_else(|| d.to_string());
    let (big, unit, details, color): (String, String, Vec<String>, Option<&str>) = match home.card {
        HomeCard::ThermoHygro => {
            let t = last_of(&widget, "temperature", &values).map(|p| p.value);
            let h = last_of(&widget, "humidity", &values).map(|p| p.value);
            let details = h
                .map(|h| vec![t!("hcard-humidity", value : format_value(h, 0)).to_string()])
                .unwrap_or_default();
            (fmt(t, decimals), unit_or("°C"), details, None)
        }
        HomeCard::AirQuality => {
            let co2 = last_of(&widget, "co2", &values).map(|p| p.value);
            let mut details = Vec::new();
            if let Some(v) = last_of(&widget, "voc", &values) {
                details.push(t!("hcard-voc", value : format_value(v.value, 0)).to_string());
            }
            if let Some(v) = last_of(&widget, "pm25", &values) {
                details.push(t!("hcard-pm25", value : format_value(v.value, 0)).to_string());
            }
            (fmt(co2, 0), "ppm".into(), details, co2.map(co2_color))
        }
        HomeCard::Power => {
            let p = last_of(&widget, "power", &values).map(|p| p.value);
            (fmt(p, 0), unit_or("W"), vec![], None)
        }
        _ => {
            // Meter: consumption over the source window (last - first of
            // a cumulative counter).
            let pts = points_of(&widget, "energy", &values).unwrap_or_default();
            let delta = match (pts.first(), pts.last()) {
                (Some(a), Some(b)) => Some((b.value - a.value).max(0.0)),
                _ => None,
            };
            let window = widget
                .source_of("energy")
                .map(|s| s.window.clone())
                .unwrap_or_default();
            (
                fmt(delta, decimals),
                unit_or("kWh"),
                vec![t!("hcard-meter-window", window : window).to_string()],
                None,
            )
        }
    };
    let style = color.map(|c| format!("color: {c}")).unwrap_or_default();
    let spark = if home.card == HomeCard::Power {
        points_of(&widget, "power", &values)
    } else {
        None
    };
    rsx! {
        div { class: "flex items-center gap-2 text-gray-500",
            HomeIconView { id: icon, class: "h-4 w-4" }
            span { class: "truncate text-xs font-medium", "{title}" }
        }
        div { class: "flex items-baseline gap-1",
            span {
                class: "text-3xl font-semibold leading-none text-gray-900",
                style: "{style}",
                "{big}"
            }
            span { class: "text-xs text-gray-400", "{unit}" }
        }
        for d in details {
            p { key: "{d}", class: "text-xs text-gray-500", "{d}" }
        }
        if let Some(points) = spark {
            Sparkline { points }
        }
    }
}

/// Axis-free mini curve (power card).
#[component]
fn Sparkline(points: Vec<TelemetryPoint>) -> Element {
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
            let x = (p.ts - t0) / ts * 100.0;
            let y = 28.0 - (p.value - v0) / vs * 26.0;
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    rsx! {
        svg {
            class: "h-8 w-full",
            view_box: "0 0 100 30",
            "preserveAspectRatio": "none",
            polyline {
                points: "{path}",
                fill: "none",
                stroke: "{TEAL}",
                "stroke-width": "1.5",
                "vector-effect": "non-scaling-stroke",
            }
        }
    }
}

/// Grid / solar / battery / home power flows (W): positive grid = import,
/// positive battery = discharge. Home = given, else grid + solar + battery.
#[component]
fn EnergyFlowCard(widget: Widget, title: String, values: LiveValues) -> Element {
    let v = |role: &str| last_of(&widget, role, &values).map(|p| p.value);
    let (grid, solar, battery, soc) = (v("grid"), v("solar"), v("battery"), v("battery_soc"));
    let home =
        v("home").or_else(|| grid.map(|g| g + solar.unwrap_or(0.0) + battery.unwrap_or(0.0)));
    let w = |x: Option<f64>| fmt(x.map(f64::abs), 0);
    let grid_text = match grid {
        Some(g) if g < 0.0 => t!("hflow-export").to_string(),
        Some(_) => t!("hflow-import").to_string(),
        None => String::new(),
    };
    let battery_text = match (battery, soc) {
        (Some(b), Some(s)) if b < 0.0 => {
            format!("{} · {} %", t!("hflow-charging"), format_value(s, 0))
        }
        (Some(_), Some(s)) => format!("{} · {} %", t!("hflow-discharging"), format_value(s, 0)),
        (_, Some(s)) => format!("{} %", format_value(s, 0)),
        _ => String::new(),
    };
    let has_solar = widget.source_of("solar").is_some();
    let has_battery =
        widget.source_of("battery").is_some() || widget.source_of("battery_soc").is_some();
    rsx! {
        p { class: "truncate text-xs font-medium text-gray-500", "{title}" }
        div { class: "grid flex-1 grid-cols-2 gap-2",
            FlowNode {
                icon: "home-grid",
                label: t!("hflow-grid").to_string(),
                value: w(grid),
                note: grid_text,
            }
            FlowNode {
                icon: "home-house",
                label: t!("hflow-home").to_string(),
                value: w(home),
                note: String::new(),
            }
            if has_solar {
                FlowNode {
                    icon: "home-solar",
                    label: t!("hflow-solar").to_string(),
                    value: w(solar),
                    note: String::new(),
                }
            }
            if has_battery {
                FlowNode {
                    icon: "home-battery",
                    label: t!("hflow-battery").to_string(),
                    value: w(battery),
                    note: battery_text,
                }
            }
        }
    }
}

#[component]
fn FlowNode(icon: String, label: String, value: String, note: String) -> Element {
    rsx! {
        div { class: "flex flex-col items-center justify-center rounded-lg bg-gray-50 p-2 text-center",
            HomeIconView { id: icon, class: "h-5 w-5 text-gray-600" }
            span { class: "text-[11px] text-gray-500", "{label}" }
            span { class: "text-sm font-semibold text-gray-900", "{value} W" }
            if !note.is_empty() {
                span { class: "text-[10px] text-gray-500", "{note}" }
            }
        }
    }
}

/// Washer, dishwasher…: status through the state rules (D135), progress
/// bar and remaining minutes, optional power switch.
#[component]
fn ApplianceCard(
    widget: Widget,
    home: HomeCardOptions,
    title: String,
    icon: String,
    values: LiveValues,
) -> Element {
    let status = last_of(&widget, "status", &values).map(|p| p.value);
    let rule = status.and_then(|v| pnex_core::resolve_state(&widget.options.states, v));
    let state = match (rule.and_then(|r| r.label.clone()), status) {
        (Some(l), _) => l,
        (None, Some(v)) => format_value(v, 0),
        (None, None) => "—".into(),
    };
    let tint = rule.and_then(|r| r.color.clone());
    let progress = last_of(&widget, "progress", &values).map(|p| p.value.clamp(0.0, 100.0));
    let remaining = last_of(&widget, "remaining", &values).map(|p| p.value);
    let right = rsx! {
        Role { widget: widget.clone(), home: home.clone(), role: "power" }
    };
    rsx! {
        Header {
            icon,
            title,
            state,
            tint,
            right,
        }
        if let Some(p) = progress {
            div { class: "h-2 w-full overflow-hidden rounded-full bg-gray-100",
                div {
                    class: "h-full rounded-full bg-teal-600",
                    style: "width: {p}%",
                }
            }
        }
        if let Some(m) = remaining {
            p { class: "text-xs text-gray-500",
                {t!("hcard-remaining", minutes : format_value(m, 0))}
            }
        }
    }
}

#[component]
fn ClockCard() -> Element {
    let mut now = use_signal(crate::util::now_label_secs);
    use_future(move || async move {
        loop {
            crate::util::sleep(std::time::Duration::from_secs(1)).await;
            now.set(crate::util::now_label_secs());
        }
    });
    let label = now();
    let (date, time) = label.split_once(' ').unwrap_or(("", label.as_str()));
    let time = time.get(..5).unwrap_or(time).to_string();
    let date = date.to_string();
    rsx! {
        div { class: "flex flex-1 flex-col items-center justify-center",
            span { class: "text-4xl font-semibold tabular-nums text-gray-900", "{time}" }
            span { class: "text-xs text-gray-500", "{date}" }
        }
    }
}

/// Localized name of a normalized weather condition.
pub fn condition_label(c: pnex_core::weather::WeatherCondition) -> String {
    use pnex_core::weather::WeatherCondition::*;
    match c {
        Clear => t!("wcond-clear").to_string(),
        PartlyCloudy => t!("wcond-partly-cloudy").to_string(),
        Cloudy => t!("wcond-cloudy").to_string(),
        Fog => t!("wcond-fog").to_string(),
        Drizzle => t!("wcond-drizzle").to_string(),
        Rain => t!("wcond-rain").to_string(),
        HeavyRain => t!("wcond-heavy-rain").to_string(),
        Sleet => t!("wcond-sleet").to_string(),
        Snow => t!("wcond-snow").to_string(),
        Thunderstorm => t!("wcond-thunderstorm").to_string(),
    }
}

/// One forecast day of the weather card.
#[derive(Clone, PartialEq)]
struct DayCell {
    label: String,
    icon: String,
    max: String,
    min: String,
}

/// Weather card (D140): fields written to org memory by the `weather`
/// flow node, current conditions plus up to five forecast days.
#[component]
fn WeatherCard(widget: Widget, title: String, values: LiveValues) -> Element {
    use pnex_core::weather::WeatherCondition;
    let v = |role: &str| last_of(&widget, role, &values).map(|p| p.value);
    let is_day = v("is_day").is_none_or(|d| d >= 0.5);
    let cond = v("condition_code").and_then(WeatherCondition::from_code);
    let icon = cond
        .map(|c| c.icon(is_day))
        .unwrap_or("home-partly-cloudy")
        .to_string();
    let condition = cond.map(condition_label).unwrap_or_default();
    let temp = fmt(v("temperature"), 0);
    let mut details = Vec::new();
    if let Some(f) = v("feels_like") {
        details.push(t!("hweather-feels", value : format_value(f, 0)).to_string());
    }
    if let Some(h) = v("humidity") {
        details.push(t!("hcard-humidity", value : format_value(h, 0)).to_string());
    }
    if let Some(w) = v("wind_speed") {
        details.push(t!("hweather-wind", value : format_value(w, 0)).to_string());
    }
    let days: Vec<DayCell> = (0..pnex_core::home::WEATHER_CARD_DAYS)
        .filter_map(|d| {
            let max = v(&format!("d{d}_t_max"))?;
            let min = v(&format!("d{d}_t_min"));
            let c = v(&format!("d{d}_condition_code")).and_then(WeatherCondition::from_code);
            let label = match d {
                0 => t!("hweather-today").to_string(),
                1 => t!("hweather-tomorrow").to_string(),
                n => t!("hweather-day-n", n : n).to_string(),
            };
            Some(DayCell {
                label,
                icon: c.map(|c| c.icon(true)).unwrap_or("home-cloud").to_string(),
                max: format!("{}°", format_value(max, 0)),
                min: min
                    .map(|m| format!("{}°", format_value(m, 0)))
                    .unwrap_or_default(),
            })
        })
        .collect();
    let details = details.join(" · ");
    rsx! {
        p { class: "truncate text-xs font-medium text-gray-500", "{title}" }
        div { class: "flex items-center gap-3",
            span { class: "text-sky-600",
                HomeIconView { id: icon, class: "h-12 w-12" }
            }
            div { class: "min-w-0",
                p { class: "text-3xl font-semibold leading-none text-gray-900", "{temp}°" }
                p { class: "truncate text-sm text-gray-600", "{condition}" }
            }
        }
        if !details.is_empty() {
            p { class: "text-xs text-gray-500", "{details}" }
        }
        if !days.is_empty() {
            div { class: "mt-auto flex justify-between gap-1 border-t border-gray-100 pt-2",
                for day in days {
                    div {
                        key: "{day.label}",
                        class: "flex flex-col items-center text-[11px] text-gray-600",
                        span { "{day.label}" }
                        HomeIconView { id: day.icon, class: "h-5 w-5 text-sky-600" }
                        span { class: "font-medium text-gray-900", "{day.max}" }
                        span { class: "text-gray-400", "{day.min}" }
                    }
                }
            }
        }
    }
}

/// Short summary of a card for a chip row (D139): icon, text, tint.
pub fn chip_summary(w: &Widget, values: &LiveValues) -> (String, String, Option<String>) {
    let title = w.title.trim().to_string();
    let Some(home) = &w.options.home else {
        // Plain widget: title + newest value of its first source.
        let v = w
            .source
            .first()
            .and_then(|s| values.get(&s.series_key()).cloned().flatten())
            .and_then(|p| p.last().map(|p| p.value));
        let rule = v.and_then(|v| pnex_core::resolve_state(&w.options.states, v));
        let text = match (rule.and_then(|r| r.label.clone()), v) {
            (Some(l), _) => l,
            (None, Some(v)) => {
                let unit = w.options.unit.clone().unwrap_or_default();
                format!(
                    "{} {unit}",
                    format_value(v, w.options.decimals.unwrap_or(1))
                )
                .trim()
                .to_string()
            }
            (None, None) => "—".into(),
        };
        let text = if title.is_empty() {
            text
        } else {
            format!("{title} {text}")
        };
        let icon = rule
            .and_then(|r| r.icon.clone())
            .or_else(|| w.options.icon.clone())
            .unwrap_or_else(|| "home-check".into());
        return (icon, text, rule.and_then(|r| r.color.clone()));
    };
    let icon = w
        .options
        .icon
        .clone()
        .unwrap_or_else(|| home.card.default_icon(home.variant.as_deref()).to_string());
    let v = |role: &str| last_of(w, role, values).map(|p| p.value);
    let named = |text: String| {
        if title.is_empty() {
            text
        } else {
            format!("{title} · {text}")
        }
    };
    match home.card {
        HomeCard::Binary => {
            let variant = home.variant.clone().unwrap_or_else(|| "generic".into());
            match v("state").map(|x| home.is_on(x)) {
                Some(on) => {
                    let tint = match (on, binary_alarm(&variant)) {
                        (true, true) => Some(RED.to_string()),
                        (true, false) => Some(AMBER.to_string()),
                        _ => None,
                    };
                    (icon, named(binary_text(&variant, on)), tint)
                }
                None => (icon, named("—".into()), None),
            }
        }
        HomeCard::ThermoHygro => (icon, named(format!("{}°", fmt(v("temperature"), 1))), None),
        HomeCard::Power => (icon, named(format!("{} W", fmt(v("power"), 0))), None),
        HomeCard::AirQuality => {
            let co2 = v("co2");
            (
                icon,
                named(format!("{} ppm", fmt(co2, 0))),
                co2.map(|c| co2_color(c).to_string()),
            )
        }
        HomeCard::Weather => {
            let cond =
                v("condition_code").and_then(pnex_core::weather::WeatherCondition::from_code);
            let day = v("is_day").is_none_or(|d| d >= 0.5);
            let icon = cond.map(|c| c.icon(day).to_string()).unwrap_or(icon);
            (
                icon,
                named(format!("{}°", fmt(v("temperature"), 0))),
                Some(BLUE.to_string()),
            )
        }
        HomeCard::Light | HomeCard::Fan | HomeCard::Irrigation => {
            let on_value = home.spec_of("power").map(|s| s.on_value()).unwrap_or(1.0);
            let on = v("state").or_else(|| commanded(home, "power")) == Some(on_value);
            let text = if on {
                t!("controls-on").to_string()
            } else {
                t!("controls-off").to_string()
            };
            (icon, named(text), on.then(|| AMBER.to_string()))
        }
        HomeCard::Gate | HomeCard::Lock | HomeCard::Alarm => {
            let role = if home.card == HomeCard::Alarm {
                "mode"
            } else {
                "command"
            };
            let spec = home.spec_of(role);
            let option = v("state")
                .or_else(|| commanded(home, role))
                .and_then(|x| spec.as_ref().and_then(|s| s.option_of(x).cloned()));
            let text = option
                .as_ref()
                .map(super::option_label)
                .unwrap_or_else(|| "—".into());
            let icon = option.and_then(|o| o.icon).unwrap_or(icon);
            (icon, named(text), None)
        }
        _ => (
            icon,
            if title.is_empty() {
                card_label(home.card)
            } else {
                title
            },
            None,
        ),
    }
}
