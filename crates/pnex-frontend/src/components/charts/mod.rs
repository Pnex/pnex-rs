//! Charts SVG maison — zéro dépendance (écoles `pages/visualisation.rs`
//! 2026-08-19 + PRD viz D30). Deux familles :
//!
//! 1. [`TimeSeriesChart`] — l'**extraction** du chart multi-séries de la
//!    page Visualisation (rendu **identique** : même viewBox, même
//!    grille, mêmes polylines + points) ;
//! 2. les **rendus d'instruments** du studio SCADA — jauge radiale,
//!    tuile valeur, mini-courbe, indicateur — dessinés pour remplir un
//!    widget du canvas (le cadre/sélection est dessiné par l'éditeur,
//!    jamais ici).
//!
//! Toute la géométrie est **pure et testée** (les composants rsx ne font
//! que du rendu) ; les couleurs sont des littéraux (jamais de classe
//! construite dynamiquement pour le scanner Tailwind).

use dioxus::prelude::*;

pub mod thermo;
use pnex_core::TelemetryPoint;

/// Palette des séries (littéraux — attributs SVG `stroke`/`fill`, pas
/// des classes). Même palette que la page Visualisation d'origine.
pub const PALETTE: [&str; 6] = [
    "#0d9488", "#4f46e5", "#db2777", "#f59e0b", "#0284c7", "#65a30d",
];

// ─────────────────────── Chart multi-séries (extrait) ───────────────────────

/// Géométrie du chart extrait (viewBox + padding pour labels Y) —
/// **identiques** aux constantes d'origine.
const CHART_W: f64 = 800.0;
const CHART_H: f64 = 280.0;
const PAD_L: f64 = 48.0;
const PAD_R: f64 = 16.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 24.0;

/// Une série à dessiner (vue-model découplée de la page).
#[derive(Clone, PartialEq)]
pub struct ChartSeries {
    /// Index de couleur dans [`PALETTE`].
    pub color_index: usize,
    pub label: String,
    /// `None` = requête en échec ou dégradée.
    pub points: Option<Vec<TelemetryPoint>>,
}

/// Formatage heure locale d'un timestamp epoch secondes (axe X).
fn time_label(ts: f64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

/// Polyline "x,y x,y …" d'une série sur l'échelle commune — pur.
fn polyline_of(
    points: &[TelemetryPoint],
    t_range: (f64, f64),
    v_range: (f64, f64),
) -> (String, Vec<(f64, f64)>) {
    let (t_min, t_max) = t_range;
    let (v_min, v_max) = v_range;
    let t_span = (t_max - t_min).max(1.0);
    let v_span = (v_max - v_min).max(1e-9);
    let x = |ts: f64| PAD_L + (ts - t_min) / t_span * (CHART_W - PAD_L - PAD_R);
    let y = |v: f64| PAD_T + (1.0 - (v - v_min) / v_span) * (CHART_H - PAD_T - PAD_B);
    let coords: Vec<(f64, f64)> = points
        .iter()
        .map(|p| {
            // Un seul point (ou plusieurs au même ts) : x centré pour
            // rester visible (comportement d'origine conservé).
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
    (path, coords)
}

/// Chart multi-séries — rendu identique à l'original de la page
/// Visualisation (échelle Y globale, grille 4 lignes, labels min/max +
/// heures de bord, polyline + cercles par point).
#[component]
pub fn TimeSeriesChart(series: Vec<ChartSeries>) -> Element {
    // Fenêtre temporelle globale + amplitude Y globale.
    let mut t_min = f64::MAX;
    let mut t_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for s in &series {
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
        return rsx! {};
    }

    #[allow(clippy::type_complexity)]
    let paths: Vec<(usize, String, Vec<(f64, f64)>)> = series
        .iter()
        .filter_map(|s| {
            let points = s.points.as_ref()?;
            if points.is_empty() {
                return None;
            }
            let (path, coords) = polyline_of(points, (t_min, t_max), (v_min, v_max));
            Some((s.color_index, path, coords))
        })
        .collect();

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
            for s in &series {
                div { key: "{s.label}", class: "flex items-center gap-2 text-sm text-gray-700",
                    span { class: "h-2.5 w-2.5 rounded-full", style: "background-color: {PALETTE[s.color_index % PALETTE.len()]}" }
                    "{s.label}"
                }
            }
        }
        svg {
            view_box: "0 0 {CHART_W} {CHART_H}",
            class: "w-full h-auto",
            xmlns: "http://www.w3.org/2000/svg",
            for gy in &grid_ys {
                line { x1: "{PAD_L}", y1: "{gy}", x2: "{CHART_W - PAD_R}", y2: "{gy}",
                    stroke: "#e5e7eb", "stroke-width": "1" }
            }
            text { x: "{PAD_L - 6.0}", y: "{PAD_T + 4.0}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {y_max_label} }
            text { x: "{PAD_L - 6.0}", y: "{CHART_H - PAD_B}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {y_min_label} }
            text { x: "{PAD_L}", y: "{CHART_H - 6.0}",
                class: "fill-gray-400", style: "font-size: 11px", {t_start_label} }
            text { x: "{CHART_W - PAD_R}", y: "{CHART_H - 6.0}", "text-anchor": "end",
                class: "fill-gray-400", style: "font-size: 11px", {t_end_label} }
            for (index, path, coords) in &paths {
                polyline {
                    points: "{path}",
                    fill: "none",
                    stroke: "{PALETTE[*index % PALETTE.len()]}",
                    "stroke-width": "2",
                    "stroke-linejoin": "round",
                    "stroke-linecap": "round"
                }
                for (px, py) in coords {
                    circle { cx: "{px}", cy: "{py}", r: "2.5",
                        fill: "{PALETTE[*index % PALETTE.len()]}" }
                }
            }
        }
    }
}

// ─────────────────────── Instruments SCADA ───────────────────────

/// Couleur d'un instrument pour une valeur donnée (le **dernier seuil
/// franchi** gagne) — pur.
pub fn threshold_color(thresholds: &[pnex_core::Threshold], value: f64, default: &str) -> String {
    let mut color = default.to_string();
    for t in thresholds {
        if value >= t.value {
            color = t.color.clone();
        }
    }
    color
}

/// Formatage d'une valeur avec décimales bornées (0..=6) — pur.
pub fn format_value(value: f64, decimals: u8) -> String {
    format!("{:.*}", decimals.min(6) as usize, value)
}

const GAUGE_INSET: f64 = 18.0;

/// Point sur l'arc de jauge (angle 180° = min à gauche → 0° = max à
/// droite) — pur. Retourne le centre, le rayon et le point.
pub fn gauge_arc_geometry(w: f64, h: f64) -> (f64, f64, f64) {
    let cx = w / 2.0;
    let cy = (h - GAUGE_INSET).min(h * 0.72).max(24.0);
    let r = (w / 2.0 - GAUGE_INSET).min(cy - 8.0).max(12.0);
    (cx, cy, r)
}

/// Coordonnées polaires → cartésiennes sur l'arc de jauge — pur.
pub fn gauge_point(cx: f64, cy: f64, r: f64, ratio: f64) -> (f64, f64) {
    let ratio = ratio.clamp(0.0, 1.0);
    let angle = std::f64::consts::PI * (1.0 - ratio); // π (gauche) → 0 (droite)
    (cx + r * angle.cos(), cy - r * angle.sin())
}

/// Chemin d'arc SVG entre deux ratios (sweep horaire en bas) — pur.
pub fn gauge_arc_path(cx: f64, cy: f64, r: f64, from: f64, to: f64) -> String {
    let (x1, y1) = gauge_point(cx, cy, r, from);
    let (x2, y2) = gauge_point(cx, cy, r, to);
    // large-arc = 0 (jamais plus de 180°), sweep = 1 (sens horaire).
    format!("M {x1:.1} {y1:.1} A {r:.1} {r:.1} 0 0 1 {x2:.1} {y2:.1}")
}

/// Normalise une valeur sur min..max → 0.0..1.0 — pur.
pub fn clamp_ratio(value: f64, min: f64, max: f64) -> f64 {
    if max <= min {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}
