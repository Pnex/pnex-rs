//! Rendu du **corps** d'un widget SCADA (jauge, valeur, mini-courbe,
//! indicateur, texte) — partagé par la vue LIVE de `pages/dashboards.rs`
//! et l'aperçu de l'éditeur. Le cadre, la sélection et les poignées de
//! resize sont dessinés par l'appelant, jamais ici : le même composant
//! sert aux deux modes (D34 : en édition, la dernière valeur est figée —
//! c'est l'appelant qui cesse de la rafraîchir, le rendu est identique).
//!
//! Toute la géométrie vit dans `components/charts` (pure, testée).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{TelemetryPoint, Widget};
use std::collections::HashMap;

use crate::components::charts::thermo::ThermoChart;
use crate::components::home_icons::HomeIconView;
use crate::components::symbols;

use crate::components::charts::{
    clamp_ratio, format_value, gauge_arc_geometry, gauge_arc_path, gauge_point, threshold_color,
};

/// Couleur par défaut de l'arc / de la LED (littéral — scanner Tailwind).
const ACCENT: &str = "#0d9488";
const GRAY: &str = "#d1d5db";
const TEXT_MUTED: &str = "#6b7280";

#[component]
pub fn WidgetBody(
    widget: Widget,
    points: Option<Vec<TelemetryPoint>>,
    /// Map live complète `metric|device` — consommée par le widget
    /// `thermo_chart` (multi-sources) ; `None` ailleurs.
    values: Option<HashMap<String, Option<Vec<TelemetryPoint>>>>,
) -> Element {
    let w = &widget;
    let decimals = w.options.decimals.unwrap_or(1);
    let last = points.as_ref().and_then(|p| p.last().cloned());
    // Le mini chart `line` ne porte l'unité nulle part ailleurs :
    // l'en-tête existe dès que le titre OU l'unité est renseigné.
    // Only `line` shows the unit in the header: stat, gauge and indicator
    // already render it next to the value (no duplicate).
    let header_unit = w
        .options
        .unit
        .clone()
        .filter(|u| w.widget_type == "line" && !u.trim().is_empty());

    // Control cards draw their own header (control label, "no effect"
    // badge) and read the state source as the actual value (D125).
    if pnex_core::CONTROL_WIDGET_TYPES.contains(&w.widget_type.as_str()) {
        return rsx! {
            crate::components::surface::ControlBody { widget: widget.clone(), state: last }
        };
    }

    // Home cards draw their whole card (D138).
    if w.widget_type == pnex_core::home::HOME_WIDGET_TYPE {
        return rsx! {
            crate::components::surface::home_card::HomeCardBody { widget: widget.clone(), values }
        };
    }

    // Symbols draw their own caption (under the drawing, not a card header).
    if w.widget_type == "symbol" {
        return rsx! {
            symbol_body { widget: widget.clone(), last, decimals }
        };
    }

    // Stale value (D135): the card greys out and says so.
    let stale = last
        .as_ref()
        .is_some_and(|p| pnex_core::is_stale(&w.options, p.ts, crate::util::now_secs()));
    rsx! {
        div {
            class: if stale { "flex h-full w-full flex-col overflow-hidden rounded-lg opacity-50 grayscale" } else { "flex h-full w-full flex-col overflow-hidden rounded-lg" },
            title: if stale { t!("db-stale").to_string() } else { String::new() },
            // En-tête : titre + unité.
            if !w.title.trim().is_empty() || header_unit.is_some() {
                div { class: "flex items-baseline justify-between px-3 pt-2",
                    span { class: "truncate text-xs font-medium text-gray-500", "{w.title}" }
                    if let Some(unit) = &header_unit {
                        span { class: "ml-2 shrink-0 text-[10px] text-gray-400", "{unit}" }
                    }
                }
            }
            // Corps par type.
            match w.widget_type.as_str() {
                "gauge" => rsx! {
                    gauge_body { widget: widget.clone(), last, decimals }
                },
                "stat" => rsx! {
                    stat_body { widget: widget.clone(), last, decimals }
                },
                "line" => rsx! {
                    line_body { widget: widget.clone(), points }
                },
                "indicator" => rsx! {
                    indicator_body { widget: widget.clone(), last, decimals }
                },
                "text" => rsx! {
                    text_body { widget: widget.clone() }
                },
                "thermo_chart" => rsx! {
                    ThermoChart { widget: widget.clone(), values }
                },
                _ => rsx! {
                    p { class: "p-2 text-xs text-gray-400", "{w.widget_type}" }
                },
            }
        }
    }
}

#[component]
fn gauge_body(widget: Widget, last: Option<TelemetryPoint>, decimals: u8) -> Element {
    let w = &widget;
    let min = w.options.min.unwrap_or(0.0);
    let max = w.options.max.unwrap_or(100.0);
    let (cx, cy, r) = gauge_arc_geometry(200.0, 140.0);
    let background = gauge_arc_path(cx, cy, r, 0.0, 1.0);
    let ratio = last
        .as_ref()
        .map(|p| clamp_ratio(p.value, min, max))
        .unwrap_or(0.0);
    let value_color = match &last {
        Some(p) => threshold_color(&w.options.thresholds, p.value, ACCENT),
        None => GRAY.to_string(),
    };
    let arc = if ratio > 0.001 {
        gauge_arc_path(cx, cy, r, 0.0, ratio)
    } else {
        String::new()
    };
    let needle = last.is_some().then(|| {
        let (nx, ny) = gauge_point(cx, cy, r - 4.0, ratio);
        (nx, ny)
    });
    let label = last
        .as_ref()
        .map(|p| format_value(p.value, decimals))
        .unwrap_or_else(|| "—".into());

    rsx! {
        div { class: "flex flex-1 items-center justify-center",
            svg {
                view_box: "0 0 200 140",
                class: "h-full w-full",
                xmlns: "http://www.w3.org/2000/svg",
                path {
                    d: "{background}",
                    fill: "none",
                    stroke: "{GRAY}",
                    "stroke-width": "10",
                    "stroke-linecap": "round",
                }
                if !arc.is_empty() {
                    path {
                        d: "{arc}",
                        fill: "none",
                        stroke: "{value_color}",
                        "stroke-width": "10",
                        "stroke-linecap": "round",
                    }
                }
                if let Some((nx, ny)) = needle {
                    line {
                        x1: "{cx}",
                        y1: "{cy}",
                        x2: "{nx}",
                        y2: "{ny}",
                        stroke: "{TEXT_MUTED}",
                        "stroke-width": "2",
                    }
                    circle {
                        cx: "{cx}",
                        cy: "{cy}",
                        r: "3",
                        fill: "{TEXT_MUTED}",
                    }
                }
                text {
                    x: "100",
                    y: "128",
                    "text-anchor": "middle",
                    style: "font-size: 20px; font-weight: 600",
                    fill: "#111827",
                    "{label}"
                    // Unité portée par la valeur (le corps, pas seulement
                    // l'en-tête : sans titre, l'en-tête n'existe pas).
                    if let Some(unit) = &w.options.unit {
                        tspan {
                            style: "font-size: 11px; font-weight: 400",
                            fill: "{TEXT_MUTED}",
                            " {unit}"
                        }
                    }
                }
            }
        }
    }
}

/// Appearance of a value after the state rules (D135): colour, icon, and
/// the text shown (a rule label replaces the number and its unit).
struct Look {
    color: String,
    icon: Option<String>,
    text: String,
    unit: Option<String>,
}

fn look(w: &Widget, last: Option<&TelemetryPoint>, decimals: u8, fallback: &str) -> Look {
    let Some(p) = last else {
        return Look {
            color: TEXT_MUTED.to_string(),
            icon: w.options.icon.clone(),
            text: "—".into(),
            unit: w.options.unit.clone(),
        };
    };
    let base = threshold_color(&w.options.thresholds, p.value, fallback);
    let rule = pnex_core::resolve_state(&w.options.states, p.value);
    let labelled = rule.and_then(|r| r.label.clone());
    Look {
        color: rule.and_then(|r| r.color.clone()).unwrap_or(base),
        icon: rule
            .and_then(|r| r.icon.clone())
            .or_else(|| w.options.icon.clone()),
        unit: if labelled.is_some() {
            None
        } else {
            w.options.unit.clone()
        },
        text: labelled.unwrap_or_else(|| format_value(p.value, decimals)),
    }
}

#[component]
fn stat_body(widget: Widget, last: Option<TelemetryPoint>, decimals: u8) -> Element {
    let l = look(&widget, last.as_ref(), decimals, ACCENT);
    rsx! {
        div {
            class: "flex flex-1 flex-col items-center justify-center gap-1 px-2",
            style: "color: {l.color}",
            if let Some(icon) = l.icon {
                HomeIconView { id: icon, class: "h-7 w-7" }
            }
            span { class: "text-3xl font-semibold leading-none", "{l.text}" }
            if let Some(unit) = &l.unit {
                span { class: "text-xs text-gray-400", "{unit}" }
            }
        }
    }
}

#[component]
fn line_body(widget: Widget, points: Option<Vec<TelemetryPoint>>) -> Element {
    let _ = &widget;
    // Fenêtre temporelle + amplitude Y bornées au widget (pas d'échelle
    // globale : un mini-chart vit dans sa boîte).
    let mut t_min = f64::MAX;
    let mut t_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    if let Some(pts) = &points {
        for p in pts {
            t_min = t_min.min(p.ts);
            t_max = t_max.max(p.ts);
            v_min = v_min.min(p.value);
            v_max = v_max.max(p.value);
        }
    }
    if t_min > t_max {
        return rsx! {
            div { class: "flex flex-1 items-center justify-center",
                span { class: "text-xs text-gray-400", {t!("db-no-data")} }
            }
        };
    }
    let t_span = (t_max - t_min).max(1.0);
    let v_span = (v_max - v_min).max(1e-9);
    let w = 200.0;
    let h = 96.0;
    let pad = 8.0;
    let path = points
        .map(|pts| {
            pts.iter()
                .map(|p| {
                    let px = pad + (p.ts - t_min) / t_span * (w - 2.0 * pad);
                    let py = pad + (1.0 - (p.value - v_min) / v_span) * (h - 2.0 * pad);
                    format!("{px:.1},{py:.1}")
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    rsx! {
        div { class: "flex flex-1 items-center justify-center",
            svg {
                view_box: "0 0 {w} {h}",
                class: "h-full w-full",
                xmlns: "http://www.w3.org/2000/svg",
                polyline {
                    points: "{path}",
                    fill: "none",
                    stroke: "{ACCENT}",
                    "stroke-width": "2",
                    "stroke-linejoin": "round",
                    "stroke-linecap": "round",
                }
            }
        }
    }
}

#[component]
fn indicator_body(widget: Widget, last: Option<TelemetryPoint>, decimals: u8) -> Element {
    let w = &widget;
    // LED : verte si la valeur franchit le premier seuil, rouge sinon —
    // sans seuil ni donnée : grise.
    let led = match &last {
        None => GRAY.to_string(),
        Some(p) => {
            let over = w
                .options
                .thresholds
                .first()
                .map(|t| p.value >= t.value)
                .unwrap_or(true);
            if over { "#16a34a" } else { "#dc2626" }.to_string()
        }
    };
    // A matching state rule overrides the LED colour, text and icon (D135).
    let l = look(w, last.as_ref(), decimals, &led);
    let color = if last.is_some() && !w.options.states.is_empty() {
        l.color.clone()
    } else {
        led
    };
    rsx! {
        div { class: "flex flex-1 items-center justify-center gap-3 px-2",
            if let Some(icon) = l.icon {
                span { style: "color: {color}",
                    HomeIconView { id: icon, class: "h-6 w-6" }
                }
            } else {
                span {
                    class: "h-5 w-5 rounded-full",
                    style: "background-color: {color}; box-shadow: 0 0 8px {color}",
                }
            }
            span { class: "text-lg font-medium text-gray-800", "{l.text}" }
            if let Some(unit) = &l.unit {
                span { class: "text-xs text-gray-400", "{unit}" }
            }
        }
    }
}

/// Process / flowchart symbol: the drawing fills the box; with a source,
/// the thresholds colour the fill live and the value can be shown under it.
#[component]
fn symbol_body(widget: Widget, last: Option<TelemetryPoint>, decimals: u8) -> Element {
    let w = &widget;
    let opts = w.options.symbol.clone().unwrap_or_default();
    let base_fill = opts
        .fill
        .clone()
        .unwrap_or_else(|| symbols::DEFAULT_FILL.to_string());
    let fill = match &last {
        Some(p) if !w.options.thresholds.is_empty() => {
            threshold_color(&w.options.thresholds, p.value, &base_fill)
        }
        _ => base_fill,
    };
    let value = (opts.show_value && !w.source.is_empty()).then(|| {
        let v = last
            .as_ref()
            .map(|p| format_value(p.value, decimals))
            .unwrap_or_else(|| "—".into());
        match &w.options.unit {
            Some(u) if !u.trim().is_empty() => format!("{v} {u}"),
            _ => v,
        }
    });
    let has_title = !w.title.trim().is_empty();
    rsx! {
        div { class: "flex h-full w-full flex-col items-center overflow-visible",
            div { class: "min-h-0 w-full flex-1",
                symbols::SymbolView {
                    shape: opts.shape.clone(),
                    stroke: opts.stroke.clone(),
                    fill: Some(fill),
                    rotation: opts.rotation,
                    flip: opts.flip,
                    stretch: opts.stretch,
                }
            }
            if has_title || value.is_some() {
                div { class: "flex max-w-full items-baseline gap-1 px-1 leading-tight",
                    if has_title {
                        span { class: "truncate text-[11px] font-medium text-gray-700",
                            "{w.title}"
                        }
                    }
                    if let Some(v) = value {
                        span { class: "shrink-0 text-[11px] font-semibold text-teal-700",
                            "{v}"
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn text_body(widget: Widget) -> Element {
    let w = &widget;
    let content = w.options.text.clone().unwrap_or_default();
    rsx! {
        div { class: "flex flex-1 items-center justify-center px-3 py-2",
            p {
                class: "w-full text-center text-sm text-gray-700",
                style: "white-space: pre-wrap",
                "{content}"
            }
        }
    }
}
