//! Moteur de diagrammes thermodynamiques — sweeps CoolProp en Rust.
//!
//! La classe C++ `PropertyPlot` (iso-lignes) n'est pas exportée dans l'API C
//! : les courbes sont calculées par **sweeps** du wrapper safe — dôme de
//! saturation, iso-lignes, psychrométrique — segmentées aux discontinuités
//! (sortie sans NaN : l'avant ne dessine que des polylines).
//!
//! Pur : entrées = strings + ranges, sortie = polylines SI + méta d'axes ;
//! le front projette (log/lin par axe), l'unité est déclarée ici.

use serde::{Deserialize, Serialize};

use crate::safe;

/// Type de diagramme. `Psychro` = air humide (chemin `haprops_si`, séparé
/// des fluides purs — cf. plan D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramKind {
    /// log(p)-h : x = Hmass (J/kg), y = P (Pa) — y log au rendu.
    Ph,
    /// T-s : x = Smass (J/kg·K), y = T (K).
    Ts,
    /// Psychrométrique : x = T (°C), y = W (kg eau / kg air sec), pression
    /// atmosphérique paramétrable.
    Psychro,
}

/// Plage d'un axe (unités SI du diagramme).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxisRange {
    pub min: f64,
    pub max: f64,
}

/// Une iso-ligne demandée : paramètre CoolProp + valeurs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IsolineSpec {
    pub param: String,
    pub values: Vec<f64>,
}

/// Méta d'un axe (projection et habillage côté front).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisMeta {
    pub label: String,
    pub unit: String,
    pub min: f64,
    pub max: f64,
    /// L'axe se rend en échelle logarithmique (y du p-h).
    pub log: bool,
}

/// Résultat d'un diagramme : polylines SI découpées + méta d'axes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramResult {
    /// Courbes : dôme de saturation, iso-lignes, chaque polyline continue.
    pub polylines: Vec<DiagramCurve>,
    pub x: AxisMeta,
    pub y: AxisMeta,
}

/// Une courbe nommée (pour la légende / le style côté front).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramCurve {
    /// Identifiant de série (ex. `"sat_liq"`, `"sat"`, `"T:400"`, `"P:1e5"`).
    pub id: String,
    pub points: Vec<[f64; 2]>,
}

/// Échantillonne `n` points log-équirépartis sur [min, max] (≥ 2).
fn log_grid(min: f64, max: f64, n: usize) -> Vec<f64> {
    let n = n.max(2);
    if min <= 0.0 || max <= min {
        // Repli linéaire (ranges psychro autour de 0 °C en P… non applicable
        // ici : grille sur T/P toujours > 0) — sécurité.
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

/// Échantillonne `n` points équirépartis sur [min, max] (≥ 2).
fn lin_grid(min: f64, max: f64, n: usize) -> Vec<f64> {
    let n = n.max(2);
    (0..n)
        .map(|i| min + (max - min) * i as f64 / (n - 1) as f64)
        .collect()
}

/// Politique de coupure sur saut de x : [`None`] = courbes continues
/// (saturation, iso-RH — jamais coupées, seulement aux trous) ;
/// `Some(frac)` = coupure si |Δx|/|x| dépasse `frac` après 3 points
/// (isothermes traversant le dôme).
#[derive(Debug, Clone, Copy)]
enum Jump {
    None,
    Relative(f64),
}

/// Sweep générique : échantillonne la grille, découpe aux échecs de `sample`
/// et aux sauts (politique [`Jump`]). Les polylines < 2 points sont
/// abandonnées. Pur — testé sans CoolProp.
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
        // Saut détecté (changement de branche) → nouvelle polyline.
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

/// Requête d'un diagramme (tout optionnel sauf fluide + type).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramRequest {
    /// Fluide pur, mélange prédéfini ou spec inline
    /// (`"Water"`, `"R410A"`, `"Propane[0.5]&Ethane[0.5]"`).
    pub fluid: String,
    pub diagram: DiagramKind,
    /// Bornes d'axes SI ; `None` = auto-fit sur les calculs.
    #[serde(default)]
    pub x_range: Option<AxisRange>,
    #[serde(default)]
    pub y_range: Option<AxisRange>,
    #[serde(default)]
    pub isolines: Vec<IsolineSpec>,
    /// Points par sweep (défaut 80, borné 10..=300).
    #[serde(default)]
    pub n_points: Option<usize>,
    /// Psychro uniquement : pression atmosphérique (défaut 101_325 Pa).
    #[serde(default)]
    pub pressure: Option<f64>,
}

/// Bornes d'échantillonnage par diagramme (SI).
struct Frame {
    t_range: (f64, f64),
    p_range: (f64, f64),
}

fn frame_for(kind: DiagramKind, fluid: &str) -> safe::Result<Frame> {
    let _ = fluid;
    Ok(match kind {
        DiagramKind::Ph | DiagramKind::Ts => Frame {
            // Plage universelle de sweeps : 0.9×T_triple → 0.95×T_crit pour
            // le dôme ; iso-lignes bornées par les réussites CoolProp.
            t_range: (273.16, 647.0),
            p_range: (100.0, 5.0e7),
        },
        DiagramKind::Psychro => Frame {
            t_range: (253.15, 333.15), // −20..60 °C
            p_range: (80_000.0, 110_000.0),
        },
    })
}

/// Calcule un diagramme complet (dôme + iso-lignes). Erreur = CoolProp a
/// rejeté le fluide/diagramme ou une iso-ligne est incohérente.
pub fn compute_diagram(req: &DiagramRequest) -> safe::Result<DiagramResult> {
    let n = req.n_points.unwrap_or(80).clamp(10, 300);
    match req.diagram {
        DiagramKind::Ph | DiagramKind::Ts => compute_pure(req, n),
        DiagramKind::Psychro => compute_psychro(req, n),
    }
}

/// Arrondit à `d` chiffres significatifs (labels d'iso-lignes stables).
fn round_sig(v: f64, digits: u32) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let exp = v.abs().log10().floor() as i32 - digits as i32 + 1;
    let factor = 10_f64.powi(exp);
    (v / factor).round() * factor
}

/// Iso-lignes « traits de base » quand la requête n'en demande aucune :
/// un dôme nu est illisible (retour utilisateur 2026-09-18). Ph =
/// iso-T (zone utile du cycle) + iso-Q ; Ts = iso-P + iso-Q — sauf
/// fluides pseudo-purs (R410A…), qui rejettent 0<Q<1 (« quality must be
/// equal to 0 or 1 ») : iso-Q absente, sonde CoolProp à l'appui. Toute
/// iso-ligne explicite de la requête désactive le défaut.
fn default_isolines(
    kind: DiagramKind,
    fluid: &str,
    tc: f64,
    t_triple: f64,
    p_crit: f64,
) -> Vec<IsolineSpec> {
    // Sonde : 0<Q<1 défini pour ce fluide ? (vrai = fluide pur, faux =
    // pseudo-pur). Un seul appel CoolProp, T sous le critère.
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

/// Diagrammes fluides purs (Ph / Ts) : dôme de saturation (branches liquide
/// et vapeur par sweep en température réduite) + iso-lignes (sweeps avec
/// découpe aux échecs/sauts — les zones deux-phases font naturellement des
/// trous).
fn compute_pure(req: &DiagramRequest, n: usize) -> safe::Result<DiagramResult> {
    if req.fluid.contains('&') || req.fluid.contains('[') {
        // Un mélange inline n'a pas de dôme simple (VLE de mélange) : v1
        // refuse, le message guide vers un fluide pur ou prédéfini.
        return Err(safe::CoolPropError(
            "diagramme de mélange inline non supporté en v1 (fluide pur ou \
             mélange prédéfini requis, ex. R410A)"
                .into(),
        ));
    }
    let frame = frame_for(req.diagram, &req.fluid)?;
    let (tc, t_triple) = critical_and_triple(&req.fluid)?;
    let p_crit = safe::props_1si(&req.fluid, "Pcrit")?;

    // Grille de température du dôme : évite Tc (sweep sensible) en s'arrêtant
    // à 0.999·Tc ; log-spaced pour denser près du point critique.
    let t_min = (t_triple * 1.005).max(200.0);
    let t_max = tc * 0.999;
    let dome_t = log_grid(t_min, t_max, n);
    // Seuil bas de pression = Psat au bas du dôme (« ajustement auto ») :
    // les sweeps d'isothermes partaient du plancher absolu (100 Pa) et leurs
    // queues de vapeur surchauffée étiraient l'axe y sur des décades
    // inutiles — le dôme se retrouvait en sire en haut du canvas.
    let p_floor = safe::props_si("P", "T", t_min, "Q", 0.0, &req.fluid)
        .unwrap_or(frame.p_range.0)
        .max(frame.p_range.0);

    let mut curves: Vec<DiagramCurve> = Vec::new();

    // ── Dôme de saturation (Q=0 liquide, Q=1 vapeur) ──
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

    // ── Iso-lignes (défauts « traits de base » si la requête est vide) ──
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
                // Isobare : horizontale dans Ph, courbe (s(T,P) vs T) dans Ts.
                for &p in &iso.values {
                    let pts: Vec<[f64; 2]> = match req.diagram {
                        DiagramKind::Ph => vec![], // traité par le front (droite)
                        _ => {
                            // Isobare Ts : sweep en T (sonde) du bas du dôme
                            // jusqu'en vapeur surchauffée. L'ancienne grille
                            // melangeait les unités (bornes en Pa lues comme
                            // des K → s absurdes et axe x étiré).
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
                // Isotherme : sweep en P (gaz + liquide sous-saturation), la
                // zone deux-phases coupe naturellement.
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
                // Iso-qualité (intérieur du dôme) : sweep en T.
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
                    "iso-ligne inconnue : {other:?} (P, T, Q supportés)"
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

/// Coordonnée y selon le diagramme : P (Ph) ou T (Ts).
fn p_or_t(kind: DiagramKind, t: f64, p: f64) -> f64 {
    match kind {
        DiagramKind::Ph => p,
        DiagramKind::Ts | DiagramKind::Psychro => t,
    }
}

/// Tc et T_triple du fluide (bornes du dôme).
fn critical_and_triple(fluid: &str) -> safe::Result<(f64, f64)> {
    let tc = safe::props_1si(fluid, "Tcrit")?;
    let tt = safe::props_1si(fluid, "Ttriple")?;
    Ok((tc, tt))
}

/// Psychrométrique : x = T en °C, y = W (kg/kg air sec) via `haprops_si`.
/// Courbe de saturation (RH=1) + iso-RH ; W constants = horizontales (front).
fn compute_psychro(req: &DiagramRequest, n: usize) -> safe::Result<DiagramResult> {
    let p_atm = req.pressure.unwrap_or(101_325.0);
    let (t_lo, t_hi) = {
        let f = frame_for(DiagramKind::Psychro, &req.fluid)?;
        f.t_range
    };
    let grid = lin_grid(t_lo, t_hi, n);
    let rh_values: Vec<f64> = {
        // Défaut : iso-RH 20/40/60/80 % ; saturée toujours.
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

    // Saturation (RH = 1) : borne haute de W sur chaque T.
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
            return Err(safe::CoolPropError(format!("RH invalide : {rh}")));
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

/// Méta d'axes : demandée, sinon auto-fit sur les courbes calculées.
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
    // Padding multiplicatif sur un axe log : un pad linéaire (5 % de
    // l'amplitude) rend le min négatif quand le min ≪ amplitude (p-h, P de
    // ~10⁵ à ~10⁶ Pa) — et un min ≤ 0 sur un axe log écrase toutes les
    // courbes contre le bord haut (bug visible du p-h).
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

/// Un point d'état d'un cycle : paire d'inputs CoolProp (nom, ex.
/// `PT_INPUTS`) + valeurs, et un label libre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CyclePointInput {
    pub label: String,
    /// Nom de paire CoolProp (`PT_INPUTS`, `HmassP_INPUTS`, `PQ_INPUTS`…) —
    /// jamais un indice (les ordres d'enums C ne sont pas une API stable).
    pub input_pair: String,
    pub v1: f64,
    pub v2: f64,
}

/// Résultat pour un point de cycle : propriétés complètes + coordonnées
/// du diagramme cible (x = Hmass/Smass/T °C, y = P/T/W).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CyclePointOutput {
    pub label: String,
    /// Propriétés SI calculées (T, P, Hmass, Smass, Dmass, Q si deux-phases).
    pub props: std::collections::BTreeMap<String, f64>,
    pub coords: [f64; 2],
    /// Phase textuelle quand elle est disponible (fluides purs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

/// Calcule chaque point d'un cycle (AbstractState par appel — la spec peut
/// être un mélange) et retourne les coordonnées du diagramme.
pub fn cycle_points(
    fluid: &str,
    diagram: DiagramKind,
    points: &[CyclePointInput],
    pressure: Option<f64>,
) -> safe::Result<Vec<CyclePointOutput>> {
    let mut out = Vec::with_capacity(points.len());
    for p in points {
        let idx = safe::get_input_pair_index(&p.input_pair).map_err(|e| {
            safe::CoolPropError(format!(
                "paire d'inputs inconnue {:?} : {}",
                p.input_pair, e.0
            ))
        })?;
        // PropsSI accepte toute paire par ses noms de propriétés : on passe
        // par AbstractState pour respecter input_pair littéralement.
        let props = state_props(fluid, idx, p.v1, p.v2)?;
        let coords = match diagram {
            DiagramKind::Ph => [props["Hmass"], props["P"]],
            DiagramKind::Ts => [props["Smass"], props["T"]],
            DiagramKind::Psychro => {
                // Conversion (T, W ou RH…) → (T °C, W) : l'état doit venir
                // d'une paire contenant W — on recalcule W par HAPropsSI si
                // absent (T + RH par défaut).
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

/// Propriétés complètes d'un état via AbstractState (mélange OK).
fn state_props(
    fluid: &str,
    input_pair: i64,
    v1: f64,
    v2: f64,
) -> safe::Result<std::collections::BTreeMap<String, f64>> {
    // La spec peut porter un backend ("HEOS::…") — extract_backend.
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
        // Qualité (hors domaine / fluide incompressible → Q omis).
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
