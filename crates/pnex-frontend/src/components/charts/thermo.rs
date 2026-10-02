//! Diagramme thermodynamique (widget `thermo_chart`) — projection pure
//! (log/lin par axe, polylines SI → viewBox SVG) + composant partagé
//! live/éditeur. Doctrine D29/D30 : SVG fait main, zéro lib, géométrie
//! pure testée ; la couche statique (dôme + iso-lignes) est re-rendue via
//! `use_resource` (clé = options thermo), la couche dynamique (points du
//! cycle) est re-résolue à chaque tick de polling.

use dioxus::prelude::*;
use dioxus_i18n::t;
use std::collections::HashMap;

use crate::api;
use pnex_core::{TelemetryPoint, ThermoChartOptions, ThermoPressureUnit, Widget};

/// Projection linéaire d'une valeur sur la plage [min, max] → [0, 1].
fn lin_project(v: f64, min: f64, max: f64) -> f64 {
    ((v - min) / (max - min).max(1e-12)).clamp(0.0, 1.0)
}

/// Projection log10 d'une valeur (axe p-h en y). Précondition v > 0.
fn log_project(v: f64, min: f64, max: f64) -> f64 {
    let (lo, hi) = (min.max(1e-12).log10(), max.max(1e-12).log10());
    ((v.max(1e-12).log10() - lo) / (hi - lo).max(1e-12)).clamp(0.0, 1.0)
}

/// Projette un point SI en coordonnées viewBox.
fn project(
    x: f64,
    y: f64,
    xm: &api::thermo::AxisMeta,
    ym: &api::thermo::AxisMeta,
    w: f64,
    h: f64,
    pad: f64,
) -> (f64, f64) {
    let u = if xm.log {
        log_project(x, xm.min, xm.max)
    } else {
        lin_project(x, xm.min, xm.max)
    };
    let v = if ym.log {
        log_project(y, ym.min, ym.max)
    } else {
        lin_project(y, ym.min, ym.max)
    };
    (pad + u * (w - 2.0 * pad), h - pad - v * (h - 2.0 * pad))
}

/// Polylines SI → attribut `points` SVG (une chaîne par polyline).
fn polyline_attr(
    points: &[[f64; 2]],
    xm: &api::thermo::AxisMeta,
    ym: &api::thermo::AxisMeta,
    w: f64,
    h: f64,
    pad: f64,
) -> String {
    points
        .iter()
        .map(|p| {
            let (x, y) = project(p[0], p[1], xm, ym, w, h, pad);
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Last numeric value of a live-values map entry (telemetry or memory key).
fn last_value_of(
    values: &HashMap<String, Option<Vec<TelemetryPoint>>>,
    series_key: &str,
) -> Option<f64> {
    values.get(series_key)?.as_ref()?.last().map(|p| p.value)
}

/// Clé du rendu statique (options → chaîne stable pour le garde-fou).
fn diagram_key(t: &ThermoChartOptions) -> String {
    format!(
        "{}|{}|{}|{:?}",
        t.diagram,
        t.fluid,
        serde_json::to_string(&t.isolines).unwrap_or_default(),
        t.pressure
    )
}

/// Couleurs littérales (scanner Tailwind + attributs SVG).
const CURVE: &str = "#94a3b8";
const DOME: &str = "#475569";
const CYCLE: &str = "#dc2626";
const FRAME: &str = "#9ca3af";
const TEXT_MUTED: &str = "#6b7280";

#[component]
pub fn ThermoChart(
    widget: Widget,
    values: Option<HashMap<String, Option<Vec<TelemetryPoint>>>>,
) -> Element {
    let Some(thermo) = widget.options.thermo.clone() else {
        return rsx! {
            div { class: "flex h-full w-full items-center justify-center",
                span { class: "text-xs text-gray-400", {t!("db-thermo-no-options")} }
            }
        };
    };
    let w = 400.0f64;
    let h = 300.0f64;
    let pad = 40.0f64;

    // ── Couche statique : dôme + iso-lignes (garde sur la clé d'options) ──
    let mut diagram_sig = use_signal(|| Option::<api::thermo::DiagramResult>::None);
    let mut diagram_requested = use_signal(String::new);
    {
        let key = diagram_key(&thermo);
        if diagram_requested.read().clone() != key {
            diagram_requested.set(key);
            let req_thermo = thermo.clone();
            spawn(async move {
                let res = api::thermo::diagram(
                    &req_thermo.fluid,
                    &req_thermo.diagram,
                    req_thermo.isolines.clone(),
                    req_thermo.pressure,
                )
                .await;
                if let Ok(res) = res {
                    diagram_sig.set(Some(res));
                }
            });
        }
    }

    // ── Couche dynamique : points du cycle (résolus à chaque tick) ──
    let mut cycle_sig = use_signal(|| Option::<Vec<api::thermo::CyclePoint>>::None);
    let mut cycle_requested = use_signal(String::new);
    {
        let request = build_cycle_request(&thermo, values.as_ref());
        let key = serde_json::to_string(&request).unwrap_or_default();
        if cycle_requested.read().clone() != key {
            cycle_requested.set(key);
            if let Some((fluid, diagram_kind, points, pressure)) = request {
                spawn(async move {
                    let res =
                        api::thermo::cycle_points(&fluid, &diagram_kind, points, pressure).await;
                    if let Ok(pts) = res {
                        cycle_sig.set(Some(pts));
                    }
                });
            } else {
                cycle_sig.set(None);
            }
        }
    }

    // ── Rendu ──
    let Some(dia) = diagram_sig().clone() else {
        return rsx! {
            div { class: "flex h-full w-full items-center justify-center",
                span { class: "animate-spin inline-block rounded-full h-6 w-6 border-b-2 border-gray-400" }
            }
        };
    };
    let xm = dia.x.clone();
    let ym = dia.y.clone();

    // Points du cycle projetés (cercles + labels), polyline fermée.
    let cycle_pts = cycle_sig().clone().unwrap_or_default();
    let mut projected: Vec<(f64, f64, f64, f64, String)> = Vec::new();
    for p in &cycle_pts {
        let (x, y) = project(p.coords[0], p.coords[1], &xm, &ym, w, h, pad);
        // Décalage du label calculé côté Rust (pas d'arithmétique f64/int
        // dans le rsx).
        projected.push((x, y, x + 6.0, y - 6.0, p.label.clone()));
    }
    let inner_w = w - 2.0 * pad;
    let inner_h = h - 2.0 * pad;
    // Labels d'axes précalculés (pas d'arithmétique dans le rsx) :
    // conversion bar sur l'axe pression du p-h (absolu, ÷1e5) + arrondi.
    // Posés DANS le cadre — les labels ancrés hors viewBox étaient
    // tronqués en rendu (retour 2026-09-18).
    let y_div = if thermo.pressure_unit == ThermoPressureUnit::Bar && thermo.diagram == "ph" {
        1.0e5
    } else {
        1.0
    };
    let y_unit = if y_div > 1.0 { "bar" } else { ym.unit.as_str() };
    let x_min_lbl = fmt_axis(xm.min, thermo.axis_decimals, 1.0);
    let x_max_lbl = fmt_axis(xm.max, thermo.axis_decimals, 1.0);
    let y_min_lbl = fmt_axis(ym.min, thermo.axis_decimals, y_div);
    let y_max_lbl = fmt_axis(ym.max, thermo.axis_decimals, y_div);
    let (lbl_x_in, lbl_y_in, lbl_y_below) = (pad + 5.0, pad + 13.0, h - pad + 13.0);
    let w_pad_lbl = w - pad;
    let y_min_y = h - pad - 6.0;
    let cycle_path: String = projected
        .iter()
        .map(|(x, y, _, _, _)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");

    rsx! {
        div { class: "flex h-full w-full flex-col",
            svg {
                view_box: "0 0 400 300",
                class: "h-full w-full",
                xmlns: "http://www.w3.org/2000/svg",
                rect {
                    x: "{pad}",
                    y: "{pad}",
                    width: "{inner_w}",
                    height: "{inner_h}",
                    fill: "#ffffff",
                    stroke: "{FRAME}",
                    "stroke-width": "1",
                }
                for c in dia.polylines.clone() {
                    polyline {
                        key: "{c.id}-{c.points.len()}",
                        points: "{polyline_attr(&c.points, &xm, &ym, w, h, pad)}",
                        fill: "none",
                        stroke: if c.id.starts_with("sat_") { DOME } else { CURVE },
                        "stroke-width": if c.id.starts_with("sat_") { "2.5" } else { "1.2" },
                        "stroke-linejoin": "round",
                    }
                }
                if projected.len() >= 2 {
                    polyline {
                        points: "{cycle_path}",
                        fill: "none",
                        stroke: "{CYCLE}",
                        "stroke-width": "2",
                        "stroke-linejoin": "round",
                        "stroke-dasharray": "none",
                    }
                }
                for (x, y, lx, ly, label) in projected.clone() {
                    g { key: "{label}-{x}",
                        circle {
                            cx: "{x}",
                            cy: "{y}",
                            r: "4",
                            fill: "{CYCLE}",
                        }
                        text {
                            x: "{lx}",
                            y: "{ly}",
                            style: "font-size: 9px",
                            fill: "{TEXT_MUTED}",
                            "{label}"
                        }
                    }
                }
                // Labels d'axes (dans le cadre : jamais tronqués).
                text {
                    x: "{lbl_x_in}",
                    y: "{lbl_y_below}",
                    style: "font-size: 9px",
                    fill: "{TEXT_MUTED}",
                    "{x_min_lbl} {xm.unit}"
                }
                text {
                    x: "{w_pad_lbl}",
                    y: "{lbl_y_below}",
                    "text-anchor": "end",
                    style: "font-size: 9px",
                    fill: "{TEXT_MUTED}",
                    "{x_max_lbl}"
                }
                text {
                    x: "{lbl_x_in}",
                    y: "{lbl_y_in}",
                    style: "font-size: 9px",
                    fill: "{TEXT_MUTED}",
                    "{y_max_lbl} {y_unit}"
                }
                text {
                    x: "{lbl_x_in}",
                    y: "{y_min_y}",
                    style: "font-size: 9px",
                    fill: "{TEXT_MUTED}",
                    "{y_min_lbl}"
                }
            }
        }
    }
}

/// Format d'un label d'axe : diviseur d'unité (Pa → bar, absolu) +
/// arrondi — `None` = auto (décimales selon l'ordre de grandeur affiché).
fn fmt_axis(v: f64, decimals: Option<u8>, div: f64) -> String {
    let v = v / div;
    let d = match decimals {
        Some(d) => (d as usize).min(6),
        None => {
            let a = v.abs();
            if a >= 1000.0 {
                0
            } else if a >= 100.0 {
                1
            } else if a >= 1.0 {
                2
            } else {
                3
            }
        }
    };
    format!("{v:.d$}")
}

/// Construit la requête cycle-points depuis les options et les valeurs
/// live : `None` si aucune paire complète (pas de requête inutile).
type CycleRequest = (String, String, Vec<serde_json::Value>, Option<f64>);

fn build_cycle_request(
    thermo: &ThermoChartOptions,
    values: Option<&HashMap<String, Option<Vec<TelemetryPoint>>>>,
) -> Option<CycleRequest> {
    let values = values?;
    let mut points = Vec::new();
    for p in &thermo.points {
        let (Some(v1), Some(v2)) = (
            last_value_of(values, &p.v1.series_key()),
            last_value_of(values, &p.v2.series_key()),
        ) else {
            continue;
        };
        points.push(serde_json::json!({
            "label": p.label,
            "input_pair": p.input_pair,
            "v1": v1,
            "v2": v2,
        }));
    }
    if points.is_empty() {
        return None;
    }
    Some((
        thermo.fluid.clone(),
        thermo.diagram.clone(),
        points,
        thermo.pressure,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn axis(min: f64, max: f64, log: bool) -> api::thermo::AxisMeta {
        api::thermo::AxisMeta {
            label: "a".into(),
            unit: String::new(),
            min,
            max,
            log,
        }
    }

    #[test]
    fn projections_lineaire_et_log() {
        // Lin : 0.5 du range → 0.5.
        assert!((lin_project(5.0, 0.0, 10.0) - 0.5).abs() < 1e-12);
        // Log : 100 dans [10, 1000] → 0.5 (1 décade de chaque côté).
        assert!((log_project(100.0, 10.0, 1000.0) - 0.5).abs() < 1e-12);
        // Bornage : hors plage → clampé.
        assert_eq!(lin_project(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(log_project(0.0, 10.0, 1000.0), 0.0);
    }

    #[test]
    fn format_axe_bar_et_arrondi() {
        // Auto : décimales selon l'ordre de grandeur affiché.
        assert_eq!(fmt_axis(452_000.0, None, 1.0), "452000");
        assert_eq!(fmt_axis(16.4173, None, 1.0), "16.42");
        assert_eq!(fmt_axis(0.0147, None, 1.0), "0.015");
        // bar absolu : Pa ÷ 1e5, arrondi choisi.
        assert_eq!(fmt_axis(452_000.0, Some(2), 1.0e5), "4.52");
        assert_eq!(fmt_axis(101_325.0, Some(3), 1.0e5), "1.013");
        // Arrondi borné (entrée corrompue).
        assert_eq!(fmt_axis(2.0, Some(255), 1.0), "2.000000");
    }

    #[test]
    fn derniere_valeur_de_la_serie() {
        let mut values = HashMap::new();
        values.insert(
            "temperature|dev-1".to_string(),
            Some(vec![
                TelemetryPoint {
                    ts: 1.0,
                    value: 10.0,
                },
                TelemetryPoint {
                    ts: 2.0,
                    value: 20.0,
                },
            ]),
        );
        assert_eq!(last_value_of(&values, "temperature|dev-1"), Some(20.0));
        // Série dégradée (None) → None.
        values.insert("temperature|dev-1".to_string(), None);
        assert_eq!(last_value_of(&values, "temperature|dev-1"), None);
    }
}
