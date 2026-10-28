//! Thermodynamic diagram engine — CoolProp sweeps in Rust.
//!
//! The C++ `PropertyPlot` class (iso-lines) is not exported in the C API:
//! curves are computed by **sweeps** of the safe wrapper — saturation
//! dome, iso-lines, psychrometric — split at discontinuities
//! (NaN-free output: the front only draws polylines).
//!
//! Pure: inputs = strings + ranges, output = SI polylines + axis meta;
//! the front projects (log/lin per axis), the unit is declared here.

use serde::{Deserialize, Serialize};

use crate::safe;

/// Diagram type. `Psychro` = humid air (`haprops_si` path, separate
/// from pure fluids — see plan D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramKind {
    /// log(p)-h: x = Hmass (J/kg), y = P (Pa) — y log when rendered.
    Ph,
    /// T-s : x = Smass (J/kg·K), y = T (K).
    Ts,
    /// Psychrometric: x = T (°C), y = W (kg water / kg dry air), configurable
    /// atmospheric pressure.
    Psychro,
}

/// Range of an axis (SI units of the diagram).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxisRange {
    pub min: f64,
    pub max: f64,
}

/// A requested iso-line: CoolProp parameter + values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolineSpec {
    pub param: String,
    pub values: Vec<f64>,
}

/// Axis meta (projection and dressing on the front side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisMeta {
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    /// The axis renders on a logarithmic scale (y of the p-h).
    pub log: bool,
}

/// Diagram result: split SI polylines + axis meta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramResult {
    /// Curves: saturation dome, iso-lines, each a continuous polyline.
    pub polylines: Vec<DiagramCurve>,
    pub x: AxisMeta,
    pub y: AxisMeta,
}

/// A named curve (for the legend / styling on the front side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramCurve {
    /// Series identifier (e.g. `"sat_liq"`, `"sat"`, `"T:400"`, `"P:1e5"`).
    pub id: String,
    pub points: Vec<[f64; 2]>,
}

/// Samples `n` log-evenly-spaced points over [min, max] (≥ 2).
fn log_grid(min: f64, max: f64, n: usize) -> Vec<f64> {
    let n = n.max(2);
    if min <= 0.0 || max <= min {
        // Linear fallback (psychro ranges around 0 °C in P… not applicable
        // here: grid on T/P always > 0) — safety.
        return (0..n)
            .map(|i| min + (max - min) * i as f64 / (n - 1) as f64)
            .collect();
    }
    (0..n)
        .map(|i| {
            let f = i as f64 / (n - 1) as f64;
            let la = min.ln();
            let lb = max.ln();
            (la + (lb - la) * f).exp()
        })
        .collect()
}

/// Samples `n` evenly spaced points over [min, max] (≥ 2).
fn lin_grid(min: f64, max: f64, n: usize) -> Vec<f64> {
    let n = n.max(2);
    (0..n)
        .map(|i| min + (max - min) * i as f64 / (n - 1) as f64)
        .collect()
}

/// Cut policy on an x jump: [`None`] = continuous curves
/// (saturation, iso-RH — never cut, only at holes);
/// `Some(frac)` = cut if |Δx|/|x| exceeds `frac` after 3 points
/// (isotherms crossing the dome).
#[derive(Debug, Clone, Copy)]
enum Jump {
    None,
    Relative(f64),
}

/// Generic sweep: samples the grid, splits at `sample` failures
/// and at jumps ([`Jump`] policy). Polylines < 2 points are
/// dropped. Pure — tested without CoolProp.
fn sweep(
    grid: Vec<f64>,
    jump: Jump,
    mut sample: impl FnMut(f64) -> Option<[f64; 2]>,
) -> Vec<Vec<[f64; 2]>> {
    let mut curves: Vec<Vec<[f64; 2]>> = Vec::new();
    let mut current: Vec<[f64; 2]> = Vec::new();
    let mut last: Option<[f64; 2]> = None;
    for t in grid {
        let Some(p) = sample(t) else {
            if !current.is_empty() {
                curves.push(std::mem::take(&mut current));
            }
            last = None;
            continue;
        };
        // Jump detected (branch change) → new polyline.
        if let (Some(prev), Some(cur)) = (last, Some(p)) {
            let _ = cur;
            if let Jump::Relative(frac) = jump {
                if current.len() >= 3
                    && prev[0].abs() > 1e-9
                    && ((p[0] - prev[0]) / prev[0].abs()).abs() > frac
                {
                    curves.push(std::mem::take(&mut current));
                }
            }
        }
        current.push(p);
        last = Some(p);
    }
    if !current.is_empty() {
        curves.push(current);
    }
    curves.into_iter().filter(|c| c.len() >= 2).collect()
}

/// Diagram request (everything optional except fluid + type).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramRequest {
    /// Pure fluid, predefined mixture or inline spec
    /// (`"Water"`, `"R410A"`, `"Propane[0.5]&Ethane[0.5]"`).
    pub fluid: String,
    pub diagram: DiagramKind,
    /// SI axis bounds; `None` = auto-fit on the computations.
    #[serde(default)]
    pub x_range: Option<AxisRange>,
    #[serde(default)]
    pub y_range: Option<AxisRange>,
    #[serde(default)]
    pub isolines: Vec<IsolineSpec>,
    /// Points per sweep (default 80, bounded 10..=300).
    #[serde(default)]
    pub n_points: Option<usize>,
    /// Psychro only: atmospheric pressure (default 101_325 Pa).
    #[serde(default)]
    pub pressure: Option<f64>,
}

/// Sampling bounds per diagram (SI).
struct Frame {
    t_range: (f64, f64),
    p_range: (f64, f64),
}

fn frame_for(kind: DiagramKind, fluid: &str) -> safe::Result<Frame> {
    let _ = fluid;
    Ok(match kind {
        DiagramKind::Ph | DiagramKind::Ts => Frame {
            // Universal sweep range: 0.9×T_triple → 0.95×T_crit for
            // the dome; iso-lines bounded by CoolProp successes.
            t_range: (273.16, 647.0),
            p_range: (100.0, 5.0e7),
        },
        DiagramKind::Psychro => Frame {
            t_range: (253.15, 333.15), // −20..60 °C
            p_range: (80_000.0, 110_000.0),
        },
    })
}

/// Computes a full diagram (dome + iso-lines). Error = CoolProp
/// rejected the fluid/diagram or an iso-line is inconsistent.
pub fn compute_diagram(req: &DiagramRequest) -> safe::Result<DiagramResult> {
    let n = req.n_points.unwrap_or(80).clamp(10, 300);
    match req.diagram {
        DiagramKind::Ph | DiagramKind::Ts => compute_pure(req, n),
        DiagramKind::Psychro => compute_psychro(req, n),
    }
}

/// Rounds to `d` significant digits (stable iso-line labels).
fn round_sig(v: f64, digits: u32) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let exp = v.abs().log10().floor() as i32 - digits as i32 + 1;
    let factor = 10_f64.powi(exp);
    (v / factor).round() * factor
}

/// "Base line" iso-lines when the request asks for none:
/// a bare dome is unreadable (user feedback 2026-09-18). Ph =
/// iso-T (useful zone of the cycle) + iso-Q; Ts = iso-P + iso-Q — except
/// pseudo-pure fluids (R410A…), which reject 0<Q<1 ("quality must be
/// equal to 0 or 1"): no iso-Q, backed by a CoolProp probe. Any
/// explicit iso-line in the request disables the default.
fn default_isolines(
    kind: DiagramKind,
    fluid: &str,
    tc: f64,
    t_triple: f64,
    p_crit: f64,
) -> Vec<IsolineSpec> {
    // Probe: is 0<Q<1 defined for this fluid? (true = pure fluid, false =
    // pseudo-pure). A single CoolProp call, T below critical.
    let q_supported =
        safe::props_si("P", "T", (t_triple * 1.3).min(tc * 0.9), "Q", 0.5, fluid).is_ok();
    let mut specs = vec![];
    match kind {
        DiagramKind::Ph => specs.push(IsolineSpec {
            param: "T".into(),
            values: log_grid((t_triple * 1.5).max(tc * 0.72), tc * 0.98, 8)
                .iter()
                .map(|t| round_sig(*t, 3))
                .collect(),
        }),
        DiagramKind::Ts => specs.push(IsolineSpec {
            param: "P".into(),
            values: log_grid(1.0e5_f64.max(p_crit * 0.02), p_crit * 0.9, 8)
                .iter()
                .map(|p| round_sig(*p, 2))
                .collect(),
        }),
        DiagramKind::Psychro => {}
    }
    if q_supported {
        specs.push(IsolineSpec {
            param: "Q".into(),
            values: vec![0.1, 0.3, 0.5, 0.7, 0.9],
        });
    }
    specs
}

/// Pure fluid diagrams (Ph / Ts): saturation dome (liquid and vapour
/// branches by sweep in reduced temperature) + iso-lines (sweeps with
/// splitting at failures/jumps — two-phase zones naturally make
/// holes).
fn compute_pure(req: &DiagramRequest, n: usize) -> safe::Result<DiagramResult> {
    if req.fluid.contains('&') || req.fluid.contains('[') {
        // An inline mixture has no simple dome (mixture VLE): v1
        // refuses, the message points to a pure or predefined fluid.
        return Err(safe::CoolPropError(
            "inline mixture diagram not supported in v1 (pure fluid or \
             predefined mixture required, e.g. R410A)"
                .into(),
        ));
    }
    let frame = frame_for(req.diagram, &req.fluid)?;
    let (tc, t_triple) = critical_and_triple(&req.fluid)?;
    let p_crit = safe::props_1si(&req.fluid, "Pcrit")?;

    // Dome temperature grid: avoids Tc (sensitive sweep) by stopping
    // at 0.999·Tc; log-spaced to be denser near the critical point.
    let t_min = (t_triple * 1.005).max(200.0);
    let t_max = tc * 0.999;
    let dome_t = log_grid(t_min, t_max, n);
    // Low pressure threshold = Psat at the bottom of the dome ("auto fit"):
    // isotherm sweeps started from the absolute floor (100 Pa) and their
    // superheated vapour tails stretched the y axis over useless
    // decades — the dome ended up squashed at the top of the canvas.
    let p_floor = safe::props_si("P", "T", t_min, "Q", 0.0, &req.fluid)
        .unwrap_or(frame.p_range.0)
        .max(frame.p_range.0);

    let mut curves: Vec<DiagramCurve> = Vec::new();

    // ── Saturation dome (Q=0 liquid, Q=1 vapour) ──
    let sat_liq: Vec<Vec<[f64; 2]>> = sweep(dome_t.clone(), Jump::None, |t| {
        let p = safe::props_si("P", "T", t, "Q", 0.0, &req.fluid).ok()?;
        let x = match req.diagram {
            DiagramKind::Ph => safe::props_si("Hmass", "T", t, "Q", 0.0, &req.fluid).ok()?,
            DiagramKind::Ts => safe::props_si("Smass", "T", t, "Q", 0.0, &req.fluid).ok()?,
            DiagramKind::Psychro => unreachable!(),
        };
        Some([x, p_or_t(req.diagram, t, p)])
    });
    for seg in sat_liq {
        curves.push(DiagramCurve {
            id: "sat_liq".into(),
            points: seg,
        });
    }
    let sat_vap: Vec<Vec<[f64; 2]>> = sweep(dome_t.clone(), Jump::None, |t| {
        let p = safe::props_si("P", "T", t, "Q", 1.0, &req.fluid).ok()?;
        let x = match req.diagram {
            DiagramKind::Ph => safe::props_si("Hmass", "T", t, "Q", 1.0, &req.fluid).ok()?,
            DiagramKind::Ts => safe::props_si("Smass", "T", t, "Q", 1.0, &req.fluid).ok()?,
            DiagramKind::Psychro => unreachable!(),
        };
        Some([x, p_or_t(req.diagram, t, p)])
    });
    for seg in sat_vap {
        curves.push(DiagramCurve {
            id: "sat_vap".into(),
            points: seg,
        });
    }

    // ── Iso-lines ("base line" defaults if the request is empty) ──
    let defaults;
    let isolines: &[IsolineSpec] = if req.isolines.is_empty() {
        defaults = default_isolines(req.diagram, &req.fluid, tc, t_triple, p_crit);
        &defaults
    } else {
        &req.isolines
    };
    for iso in isolines {
        match iso.param.as_str() {
            "P" => {
                // Isobar: horizontal in Ph, curve (s(T,P) vs T) in Ts.
                for &p in &iso.values {
                    let pts: Vec<[f64; 2]> = match req.diagram {
                        DiagramKind::Ph => vec![], // handled by the front (straight line)
                        _ => {
                            // Ts isobar: sweep in T (probe) from the bottom of the dome
                            // up to superheated vapour. The old grid
                            // mixed units (bounds in Pa read as
                            // K → absurd s and stretched x axis).
                            let grid = log_grid(t_min, tc * 1.2, n);
                            sweep(grid, Jump::Relative(0.35), |t_probe| {
                                let t =
                                    safe::props_si("T", "P", p, "T", t_probe, &req.fluid).ok()?;
                                let s = safe::props_si("Smass", "P", p, "T", t, &req.fluid).ok()?;
                                Some([s, t])
                            })
                            .into_iter()
                            .flatten()
                            .collect()
                        }
                    };
                    if !pts.is_empty() {
                        curves.push(DiagramCurve {
                            id: format!("P:{p}"),
                            points: pts,
                        });
                    }
                }
            }
            "T" => {
                // Isotherm: sweep in P (gas + subcooled liquid), the
                // two-phase zone splits naturally.
                for &t in &iso.values {
                    let grid = log_grid(p_floor, p_crit * 1.2, n);
                    let segs = sweep(grid, Jump::Relative(0.35), |p| {
                        let h = safe::props_si(
                            match req.diagram {
                                DiagramKind::Ph => "Hmass",
                                _ => "Smass",
                            },
                            "T",
                            t,
                            "P",
                            p,
                            &req.fluid,
                        )
                        .ok()?;
                        let y = match req.diagram {
                            DiagramKind::Ph => p,
                            DiagramKind::Ts => t,
                            DiagramKind::Psychro => unreachable!(),
                        };
                        Some([h, y])
                    });
                    for seg in segs {
                        curves.push(DiagramCurve {
                            id: format!("T:{t}"),
                            points: seg,
                        });
                    }
                }
            }
            "Q" => {
                // Iso-quality (inside the dome): sweep in T.
                for &q in &iso.values {
                    if !(0.0..=1.0).contains(&q) {
                        continue;
                    }
                    let segs = sweep(dome_t.clone(), Jump::None, |t| {
                        let p = safe::props_si("P", "T", t, "Q", q, &req.fluid).ok()?;
                        let x = safe::props_si(
                            match req.diagram {
                                DiagramKind::Ph => "Hmass",
                                _ => "Smass",
                            },
                            "T",
                            t,
                            "Q",
                            q,
                            &req.fluid,
                        )
                        .ok()?;
                        Some([x, p_or_t(req.diagram, t, p)])
                    });
                    for seg in segs {
                        curves.push(DiagramCurve {
                            id: format!("Q:{q}"),
                            points: seg,
                        });
                    }
                }
            }
            other => {
                return Err(safe::CoolPropError(format!(
                    "unknown iso-line: {other:?} (P, T, Q supported)"
                )));
            }
        }
    }

    let (x, y) = axis_meta(req.diagram, &req.x_range, &req.y_range, &curves);
    Ok(DiagramResult {
        polylines: curves,
        x,
        y,
    })
}

/// y coordinate per diagram: P (Ph) or T (Ts).
fn p_or_t(kind: DiagramKind, t: f64, p: f64) -> f64 {
    match kind {
        DiagramKind::Ph => p,
        DiagramKind::Ts | DiagramKind::Psychro => t,
    }
}

/// Tc and T_triple of the fluid (dome bounds).
fn critical_and_triple(fluid: &str) -> safe::Result<(f64, f64)> {
    let tc = safe::props_1si(fluid, "Tcrit")?;
    let tt = safe::props_1si(fluid, "Ttriple")?;
    Ok((tc, tt))
}

/// Psychrometric: x = T in °C, y = W (kg/kg dry air) via `haprops_si`.
/// Saturation curve (RH=1) + iso-RH; constant W = horizontal lines (front).
fn compute_psychro(req: &DiagramRequest, n: usize) -> safe::Result<DiagramResult> {
    let p_atm = req.pressure.unwrap_or(101_325.0);
    let (t_lo, t_hi) = {
        let f = frame_for(DiagramKind::Psychro, &req.fluid)?;
        f.t_range
    };
    let grid = lin_grid(t_lo, t_hi, n);
    let rh_values: Vec<f64> = {
        // Default: iso-RH 20/40/60/80 %; saturated always.
        let custom: Vec<f64> = req
            .isolines
            .iter()
            .filter(|i| i.param.eq_ignore_ascii_case("RH"))
            .flat_map(|i| i.values.iter().copied())
            .collect();
        if custom.is_empty() {
            vec![0.2, 0.4, 0.6, 0.8]
        } else {
            custom
        }
    };

    let mut curves: Vec<DiagramCurve> = Vec::new();
    let mut w_max = 0.0f64;
    let mut t_min_seen = t_hi;
    let mut t_max_seen = t_lo;

    // Saturation (RH = 1): upper bound of W at each T.
    let sat: Vec<Vec<[f64; 2]>> = sweep(grid.clone(), Jump::None, |t| {
        let w = safe::haprops_si("W", "T", t, "P", p_atm, "R", 1.0).ok()?;
        Some([(t - 273.15), w])
    });
    for seg in &sat {
        for p in seg {
            w_max = w_max.max(p[1]);
            t_min_seen = t_min_seen.min(p[0]);
            t_max_seen = t_max_seen.max(p[0]);
        }
    }
    for seg in sat {
        curves.push(DiagramCurve {
            id: "RH:1".into(),
            points: seg,
        });
    }

    // Iso-RH.
    for rh in rh_values {
        if !(0.0..=1.0).contains(&rh) {
            return Err(safe::CoolPropError(format!("invalid RH: {rh}")));
        }
        let segs = sweep(grid.clone(), Jump::None, |t| {
            let w = safe::haprops_si("W", "T", t, "P", p_atm, "R", rh).ok()?;
            Some([(t - 273.15), w])
        });
        for seg in segs {
            for p in &seg {
                w_max = w_max.max(p[1]);
                t_min_seen = t_min_seen.min(p[0]);
                t_max_seen = t_max_seen.max(p[0]);
            }
            curves.push(DiagramCurve {
                id: format!("RH:{rh}"),
                points: seg,
            });
        }
    }

    let x = AxisMeta {
        label: "T".into(),
        unit: "°C".into(),
        min: req.x_range.map(|r| r.min).unwrap_or(t_min_seen),
        max: req.x_range.map(|r| r.max).unwrap_or(t_max_seen),
        log: false,
    };
    let y = AxisMeta {
        label: "W".into(),
        unit: "kg/kg".into(),
        min: req.y_range.map(|r| r.min).unwrap_or(0.0),
        max: req.y_range.map(|r| r.max).unwrap_or(w_max * 1.05),
        log: false,
    };
    Ok(DiagramResult {
        polylines: curves,
        x,
        y,
    })
}

/// Axis meta: as requested, otherwise auto-fit on the computed curves.
fn axis_meta(
    kind: DiagramKind,
    x_range: &Option<AxisRange>,
    y_range: &Option<AxisRange>,
    curves: &[DiagramCurve],
) -> (AxisMeta, AxisMeta) {
    let mut x_min = f64::MAX;
    let mut x_max = f64::MIN;
    let mut y_min = f64::MAX;
    let mut y_max = f64::MIN;
    for c in curves {
        for p in &c.points {
            x_min = x_min.min(p[0]);
            x_max = x_max.max(p[0]);
            y_min = y_min.min(p[1]);
            y_max = y_max.max(p[1]);
        }
    }
    let (label_x, unit_x, label_y, unit_y, log_x, log_y) = match kind {
        DiagramKind::Ph => ("Hmass", "J/kg", "P", "Pa", false, true),
        DiagramKind::Ts => ("Smass", "J/kg·K", "T", "K", false, false),
        DiagramKind::Psychro => ("T", "°C", "W", "kg/kg", false, false),
    };
    let pad = |min: f64, max: f64| -> (f64, f64) {
        let d = (max - min).abs();
        (min - d * 0.05, max + d * 0.05)
    };
    // Multiplicative padding on a log axis: a linear pad (5 % of
    // the amplitude) makes the min negative when min ≪ amplitude (p-h, P from
    // ~10⁵ to ~10⁶ Pa) — and a min ≤ 0 on a log axis crushes all
    // curves against the top edge (visible p-h bug).
    let log_pad = |min: f64, max: f64| -> (f64, f64) {
        if min > 0.0 {
            (min * 0.8, max * 1.25)
        } else {
            (max * 1e-3, max * 1.25)
        }
    };
    let (x_min, x_max) = match x_range {
        Some(r) => (r.min, r.max),
        None => pad(x_min, x_max),
    };
    let (y_min, y_max) = match y_range {
        Some(r) => (r.min, r.max),
        None => {
            if log_y {
                log_pad(y_min, y_max)
            } else {
                pad(y_min, y_max)
            }
        }
    };
    (
        AxisMeta {
            label: label_x.into(),
            unit: unit_x.into(),
            min: x_min,
            max: x_max,
            log: log_x,
        },
        AxisMeta {
            label: label_y.into(),
            unit: unit_y.into(),
            min: y_min,
            max: y_max,
            log: log_y,
        },
    )
}

// ─────────────────────────── cycles ───────────────────────────

/// A cycle state point: CoolProp input pair (name, e.g.
/// `PT_INPUTS`) + values, and a free label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CyclePointInput {
    pub label: String,
    /// CoolProp pair name (`PT_INPUTS`, `HmassP_INPUTS`, `PQ_INPUTS`…) —
    /// never an index (C enum orders are not a stable API).
    pub input_pair: String,
    pub v1: f64,
    pub v2: f64,
}

/// Result for a cycle point: full properties + coordinates
/// in the target diagram (x = Hmass/Smass/T °C, y = P/T/W).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CyclePointOutput {
    pub label: String,
    /// Computed SI properties (T, P, Hmass, Smass, Dmass, Q if two-phase).
    pub props: std::collections::BTreeMap<String, f64>,
    pub coords: [f64; 2],
    /// Textual phase when available (pure fluids).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

/// Computes each point of a cycle (AbstractState per call — the spec may
/// be a mixture) and returns the diagram coordinates.
pub fn cycle_points(
    fluid: &str,
    diagram: DiagramKind,
    points: &[CyclePointInput],
    pressure: Option<f64>,
) -> safe::Result<Vec<CyclePointOutput>> {
    let mut out = Vec::with_capacity(points.len());
    for p in points {
        let idx = safe::get_input_pair_index(&p.input_pair).map_err(|e| {
            safe::CoolPropError(format!("unknown input pair {:?}: {}", p.input_pair, e.0))
        })?;
        // PropsSI accepts any pair by its property names: we go
        // through AbstractState to honour input_pair literally.
        let props = state_props(fluid, idx, p.v1, p.v2)?;
        let coords = match diagram {
            DiagramKind::Ph => [props["Hmass"], props["P"]],
            DiagramKind::Ts => [props["Smass"], props["T"]],
            DiagramKind::Psychro => {
                // Conversion (T, W or RH…) → (T °C, W): the state must come
                // from a pair containing W — W is recomputed by HAPropsSI if
                // absent (T + RH by default).
                let w = props.get("W").copied().unwrap_or_else(|| {
                    safe::haprops_si(
                        "W",
                        "T",
                        props["T"],
                        "P",
                        pressure.unwrap_or(101_325.0),
                        "R",
                        0.5,
                    )
                    .unwrap_or(0.0)
                });
                [props["T"] - 273.15, w]
            }
        };
        out.push(CyclePointOutput {
            label: p.label.clone(),
            props,
            coords,
            phase: None,
        });
    }
    Ok(out)
}

/// Full properties of a state via AbstractState (mixture OK).
fn state_props(
    fluid: &str,
    input_pair: i64,
    v1: f64,
    v2: f64,
) -> safe::Result<std::collections::BTreeMap<String, f64>> {
    // The spec may carry a backend ("HEOS::…") — extract_backend.
    let (backend, fluids) = safe::extract_backend(fluid).unwrap_or_default();
    let backend = if backend.is_empty() { "HEOS" } else { &backend };
    let handle = safe::abstract_state_factory(backend, fluids.trim_start_matches("HEOS::"))?;
    let result = (|| {
        safe::abstract_state_update(handle, input_pair, v1, v2)?;
        let mut props = std::collections::BTreeMap::new();
        for (name, key) in [
            ("T", "T"),
            ("P", "P"),
            ("Hmass", "Hmass"),
            ("Smass", "Smass"),
            ("Dmass", "Dmass"),
        ] {
            let idx = safe::get_param_index(key)?;
            props.insert(
                name.to_string(),
                safe::abstract_state_keyed_output(handle, idx)?,
            );
        }
        // Quality (out of domain / incompressible fluid → Q omitted).
        if let Ok(qi) = safe::get_param_index("Q") {
            if let Ok(q) = safe::abstract_state_keyed_output(handle, qi) {
                if q.is_finite() && (0.0..=1.0).contains(&q) {
                    props.insert("Q".to_string(), q);
                }
            }
        }
        Ok(props)
    })();
    let _ = safe::abstract_state_free(handle);
    result
}
