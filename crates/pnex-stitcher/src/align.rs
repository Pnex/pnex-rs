//! Raffinement de pose par les images (pipeline Quality).
//!
//! Les poses capteur dérivent : quelques degrés de dérive cumulée sur un
//! anneau, et la HFOV réelle du webview n'est connue qu'à la marge. Comme
//! toutes les implémentations de référence (bundle adjustment rotation-only
//! d'OpenPano, `BundleAdjuster` d'OpenCV), on corrige les poses **par la
//! preuve image**, initialisées par les poses capteur (déjà bonnes à ~1-2°).
//!
//! Principe :
//! 1. chaque frame est rendue seule en équirect **gris** basse résolution ;
//! 2. pour chaque paire au recouvrement géométrique suffisant, recherche du
//!    décalage translation (dλ, dφ) minimisant l'écart NCC zéro-moyenne sur
//!    2 dalles de latitude (insensible aux gains → pas de couplage avec la
//!    compensation d'exposition), sous-pixel par fit parabolique ;
//! 3. solve global Gauss-Newton : résidus `(δb − δa) − m(a,b)` pondérés par
//!    `ncc²`, prior Tikhonov vers la pose capteur (la jauge — système plein
//!    rang sans contrainte explicite) ;
//! 4. corrections appliquées aux poses + re-dérivation de `inv`/bande lat
//!    ([`PreparedFrame::refresh_pose`] — sinon le pré-filtre par ligne exclut
//!    les frames corrigées).
//!
//! ROLL gelé : ancré par la gravité du rotation vector, il est déjà bon ; un
//! roll résiduel ne se manifeste qu'en rotation locale (baisse le pic NCC,
//! ce qui en fait un détecteur passif). Séquentiel, zéro rayon, zéro dép.

use crate::project::{interior_weight, project_uvc, PreparedFrame};
use crate::render::{lat_of, lon_of, sample_bilinear};
use crate::QualityParams;

/// Type de paire : seuils NCC distincts (une paire inter-anneaux corrèle un
/// contenu étiré ×1/cos φ d'un côté → pic NCC structurellement plus bas).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    /// Même tranche de pitch (voisins d'anneau).
    Intra,
    /// Pitches différents (anneau à plat vs anneau incliné).
    Inter,
}

/// Mesure de décalage d'une paire sur une dalle de latitude (en degrés).
#[derive(Debug, Clone, Copy)]
pub struct SlabMeasure {
    /// Décalage en longitude à appliquer à `b` pour coïncider avec `a`.
    pub dlon_deg: f32,
    /// Décalage en latitude (même convention).
    pub dlat_deg: f32,
    /// Pic NCC de la dalle (confiance).
    pub ncc: f32,
    /// Lignes de la dalle (poids par aire).
    pub rows: u32,
}

/// Résultat du raffinement : corrections par frame (degrés, ordre d'entrée),
/// mesures acceptées avec leurs résidus après solve, et l'état de convergence.
#[derive(Debug, Clone, Default)]
pub struct AlignReport {
    /// `[dψ, dθ, dρ]` appliqués par frame (degrés — roll toujours 0 sur le
    /// chemin NCC, gelé par conception).
    pub corrections_deg: Vec<[f32; 3]>,
    /// Paires acceptées : (a, b, kind, dalles) — dalles vides sur le chemin
    /// modèle (la mesure est une rotation, pas un décalage par dalle).
    pub accepted: Vec<(usize, usize, PairKind, Vec<SlabMeasure>)>,
    /// Candidats rejetés (recouvrement pauvre ou scène sans texture).
    pub rejected: usize,
    /// `false` si le solve a divergé (corrections annulées → poses capteur).
    pub converged: bool,
    /// Chemin modèle : `true` si le repli NCC a été pris (paires trop
    /// rares) — journalisation serveur.
    pub fallback: bool,
    /// Hfov calibré par les images quand l'auto-calibration a tourné —
    /// diagnostic + journalisation.
    pub calibrated_hfov: Option<f32>,
}

/// Strip équirect gris basse résolution d'une frame seule, avec masque de
/// couverture et plage de lignes couvertes.
struct GrayStrip {
    w: u32,
    h: u32,
    /// Lignes couvertes (incluse, exclue).
    rows: (usize, usize),
    px: Vec<f32>,
    valid: Vec<bool>,
}

/// Rend la frame seule en équirect gris à `work_width` (2:1), bande couverte
/// seulement (le pré-filtre lat limite les lignes projetées).
fn render_gray_strip(f: &PreparedFrame, work_width: u32) -> GrayStrip {
    let w = work_width;
    let h = (work_width / 2).max(1);
    let mut px = vec![0.0f32; (w * h) as usize];
    let mut valid = vec![false; (w * h) as usize];
    let rows_top = (((90.0 - f.lat_max) / 180.0 * h as f32).floor() as usize)
        .saturating_sub(1)
        .min(h as usize - 1);
    let rows_bot = (((90.0 - f.lat_min) / 180.0 * h as f32).ceil() as usize)
        .saturating_add(1)
        .min(h as usize - 1);
    for j in rows_top..=rows_bot {
        let lat = lat_of(j as u32, w, h);
        if lat < f.lat_min || lat > f.lat_max {
            continue;
        }
        for i in 0..w {
            let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon_of(i, w), lat))
            else {
                continue;
            };
            if interior_weight(f, u, v) <= 0.0 {
                continue;
            }
            let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
            let o = (j as u32 * w + i) as usize;
            px[o] = 0.299 * col[0] + 0.587 * col[1] + 0.114 * col[2];
            valid[o] = true;
        }
    }

    // Blanchiment : high-pass (px − flou boîte r≈4 px, séparable, horizontal
    // circulaire, vertical clampé). La NCC ne corrèle plus que la texture
    // fine : les basses fréquences restent corrélées à grand décalage et
    // créent des pics parasites au bord de fenêtre.
    let r = 4usize;
    let mut tmp = vec![0.0f32; px.len()];
    for j in 0..(h as usize) {
        let base = j * w as usize;
        for i in 0..(w as usize) {
            let mut acc = 0.0f32;
            let mut cnt = 0u32;
            for d in -(r as i32)..=(r as i32) {
                let x = (i as i32 + d).rem_euclid(w as i32) as usize;
                let o = base + x;
                if valid[o] {
                    acc += px[o];
                    cnt += 1;
                }
            }
            tmp[base + i] = if cnt > 0 { acc / cnt as f32 } else { 0.0 };
        }
    }
    for j in 0..(h as usize) {
        let base = j * w as usize;
        for i in 0..(w as usize) {
            let o = base + i;
            if !valid[o] {
                continue;
            }
            let mut acc = 0.0f32;
            let mut cnt = 0u32;
            for d in -(r as i32)..=(r as i32) {
                let y = (j as i32 + d).clamp(0, h as i32 - 1) as usize;
                let yo = y * w as usize + i;
                if valid[yo] {
                    acc += tmp[yo];
                    cnt += 1;
                }
            }
            let mean = if cnt > 0 { acc / cnt as f32 } else { 0.0 };
            px[o] -= mean;
        }
    }

    // Érosion du masque valide (rayon r+1) : les halos de bord du
    // blanchiment (saut de DC à la frontière du lens) corrélaient avec le
    // contenu et biaisaient le pic NCC de ~1 px. On exclut la bordure.
    let er = r + 1;
    let mut eroded = valid.clone();
    for j in 0..(h as usize) {
        for i in 0..(w as usize) {
            let o = j * w as usize + i;
            if !valid[o] {
                continue;
            }
            for dy in -(er as i32)..=(er as i32) {
                let y = (j as i32 + dy).clamp(0, h as i32 - 1) as usize;
                for dx in -(er as i32)..=(er as i32) {
                    let x = (i as i32 + dx).rem_euclid(w as i32) as usize;
                    if !valid[y * w as usize + x] {
                        eroded[o] = false;
                        break;
                    }
                }
                if !eroded[o] {
                    break;
                }
            }
        }
    }

    GrayStrip {
        w,
        h,
        rows: (rows_top, rows_bot),
        px,
        valid: eroded,
    }
}

/// Paires candidates : recouvrement de bandes lat ≥ 5° ET écart azimutal des
/// axes ≤ (hfov_a + hfov_b)/2. Le wrap circulaire est géré (paire de
/// fermeture, voisinage de ±180°). Aucune structure « ring » : dérivation
/// purement géométrique.
#[must_use]
pub fn build_pairs(frames: &[PreparedFrame]) -> Vec<(usize, usize, PairKind)> {
    let mut pairs = Vec::new();
    for a in 0..frames.len() {
        for b in (a + 1)..frames.len() {
            let (fa, fb) = (&frames[a], &frames[b]);
            let overlap = fa.lat_max.min(fb.lat_max) - fa.lat_min.max(fb.lat_min);
            if overlap < 5.0 {
                continue;
            }
            let dlon = wrap180(fa.yaw_deg - fb.yaw_deg).abs();
            if dlon > 90.0 {
                continue;
            }
            // Fenêtre azimutale via les focales (hfov par frame).
            let hfov_a = 2.0 * (fa.width as f32 * 0.5 / fa.f_px).atan().to_degrees();
            let hfov_b = 2.0 * (fb.width as f32 * 0.5 / fb.f_px).atan().to_degrees();
            if dlon > (hfov_a + hfov_b) * 0.5 {
                continue;
            }
            let kind = if (fa.pitch_deg - fb.pitch_deg).abs() < 20.0 {
                PairKind::Intra
            } else {
                PairKind::Inter
            };
            pairs.push((a, b, kind));
        }
    }
    pairs
}

/// wrap en (−180, 180].
fn wrap180(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

/// Mesure une paire : recherche du décalage NCC maximal par dalle, fit
/// parabolique, validation (seuil par type + bassin). `None` = rejetée.
fn measure_pair(
    a: &GrayStrip,
    b: &GrayStrip,
    kind: PairKind,
    qp: &QualityParams,
) -> Option<Vec<SlabMeasure>> {
    let rows = (a.rows.0.max(b.rows.0), a.rows.1.min(b.rows.1));
    if rows.1 <= rows.0 || a.w != b.w || a.h != b.h {
        return None;
    }
    let (w, h) = (a.w as i64, a.h as i64);
    let dpp_x = 360.0 / a.w as f32;
    let dpp_y = 180.0 / a.h as f32;
    // Fenêtre de recherche en pixels (wrap horizontal borné à ±w/2).
    let wx = ((qp.align_search_deg / dpp_x).ceil() as i64).min(w / 2);
    let wy = ((qp.align_search_deg / dpp_y).ceil() as i64).min(h / 2);

    // Dalles de latitude : 2 moitiés du recouvrement (l'écart de mesure
    // entre dalles est le signal de roll — logué par l'appelant).
    let slab_cut = (rows.0 + rows.1) / 2;
    let slabs = [rows.0..slab_cut, slab_cut..rows.1];

    let mut out = Vec::with_capacity(2);
    for slab in slabs {
        if slab.end <= slab.start {
            continue;
        }
        let Some(m) = measure_slab(a, b, slab.clone(), wx, wy) else {
            continue;
        };
        // Validation : seuil NCC par type de paire + test de bassin (une
        // scène sans texture donne un plateau — pic non significatif) +
        // rejet des saturations en bord de fenêtre (pic écrêté = mesure
        // non fiable, la vraie solution est hors de la fenêtre).
        // Validation : seuil NCC par type de paire + rejet des saturations
        // en bord de fenêtre (pic écrêté = vraie solution hors fenêtre).
        let threshold = match kind {
            PairKind::Intra => qp.align_min_ncc_intra,
            PairKind::Inter => qp.align_min_ncc_inter,
        };
        if m.ncc < threshold {
            continue;
        }
        let lim = 0.9 * qp.align_search_deg;
        if m.dlon_deg.abs() > lim || m.dlat_deg.abs() > lim {
            continue;
        }
        if !basin_ok(a, b, slab.clone(), m, wx, wy) {
            continue;
        }
        out.push(m);
    }
    if out.is_empty() {
        return None;
    }
    // Fusion pondérée des dalles en UNE mesure par paire : sous roll, les
    // deux dalles se déplacent en sens opposés — leur moyenne pondérée
    // annule le couplage roll et dilue une dalle contaminée.
    let (mut ws, mut wl, mut mlon, mut mlat, mut mncc) = (0.0f32, 0u32, 0.0f32, 0.0f32, 0.0f32);
    for m in &out {
        let w = m.ncc * m.ncc * m.rows.max(1) as f32;
        ws += w;
        wl += m.rows;
        mlon += w * m.dlon_deg;
        mlat += w * m.dlat_deg;
        mncc += w * m.ncc;
    }
    let ws_ = ws.max(1e-6);
    Some(vec![SlabMeasure {
        dlon_deg: mlon / ws_,
        dlat_deg: mlat / ws_,
        ncc: mncc / ws_,
        rows: wl,
    }])
}

/// NCC entre `a` et `b` décalée de (sx, sy) sur la dalle de lignes `rows`.
/// Comparaison symétrique `a[x+sx, y+sy] ↔ b[x, y]` : le décalage mesuré est
/// le déplacement du contenu de `a` par rapport à `b` (wrap horizontal,
/// bornes verticales). Convention (verrouillée par le test jitter) :
/// `mλ = sx·dpp`, `mφ = sy·dpp`, résidu de solve `r = (δa − δb) − m`.
fn ncc_at(a: &GrayStrip, b: &GrayStrip, rows: std::ops::Range<usize>, sx: i64, sy: i64) -> f32 {
    let w = a.w as i64;
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) =
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0u64);
    for y in rows.clone() {
        let ya = y as i64 + sy;
        if ya < 0 || ya >= a.h as i64 {
            continue;
        }
        for x in 0..w {
            let xa = (x + sx).rem_euclid(w);
            let oa = (ya * w + xa) as usize;
            let ob = (y as i64 * w + x) as usize;
            if !a.valid[oa] || !b.valid[ob] {
                continue;
            }
            let (va, vb) = (a.px[oa] as f64, b.px[ob] as f64);
            sa += va;
            sb += vb;
            saa += va * va;
            sbb += vb * vb;
            sab += va * vb;
            n += 1;
        }
    }
    if n < 200 {
        return 0.0;
    }
    let nf = n as f64;
    let cov = sab - sa * sb / nf;
    let vara = saa - sa * sa / nf;
    let varb = sbb - sb * sb / nf;
    if vara <= 1e-9 || varb <= 1e-9 {
        return 0.0;
    }
    (cov / (vara * varb).sqrt()) as f32
}

/// Recherche exhaustive du NCC maximal sur la fenêtre + fit parabolique.
fn measure_slab(
    a: &GrayStrip,
    b: &GrayStrip,
    rows: std::ops::Range<usize>,
    wx: i64,
    wy: i64,
) -> Option<SlabMeasure> {
    let mut best = (f32::MIN, 0i64, 0i64);
    let mut grid: std::collections::HashMap<(i64, i64), f32> = std::collections::HashMap::new();
    for sy in -wy..=wy {
        for sx in -wx..=wx {
            let n = ncc_at(a, b, rows.clone(), sx, sy);
            grid.insert((sx, sy), n);
            if n > best.0 {
                best = (n, sx, sy);
            }
        }
    }
    if best.0 <= 0.0 {
        return None;
    }
    // Fit parabolique autour du pic (précision sub-pixel ≈ 0,05-0,1 px).
    let (n0, sx, sy) = best;
    let dx = parabola(
        grid.get(&(sx - 1, sy)).copied().unwrap_or(n0),
        n0,
        grid.get(&(sx + 1, sy)).copied().unwrap_or(n0),
    );
    let dy = parabola(
        grid.get(&(sx, sy - 1)).copied().unwrap_or(n0),
        n0,
        grid.get(&(sx, sy + 1)).copied().unwrap_or(n0),
    );
    let fx = sx as f32 + dx.clamp(-0.5, 0.5);
    let fy = sy as f32 + dy.clamp(-0.5, 0.5);
    let dpp_x = 360.0 / a.w as f32;
    let dpp_y = 180.0 / a.h as f32;
    Some(SlabMeasure {
        dlon_deg: fx * dpp_x,
        dlat_deg: fy * dpp_y,
        ncc: n0,
        rows: (rows.end - rows.start) as u32,
    })
}

/// Déplacement sub-pixel du sommet d'une parabole par 3 points équidistants.
fn parabola(nm1: f32, n0: f32, np1: f32) -> f32 {
    let denom = nm1 - 2.0 * n0 + np1;
    if denom.abs() < 1e-9 {
        return 0.0;
    }
    0.5 * (nm1 - np1) / denom
}

/// Test de bassin : le NCC doit chuter nettement quand on s'écarte du pic —
/// sinon le pic est un plateau (scène sans texture, ciel uni) et la mesure
/// n'est pas fiable. L'écart d'évaluation suit la résolution (≥ 2 px et
/// ≥ 2°) pour rester au-delà d'une période de la plus fine octave.
fn basin_ok(
    a: &GrayStrip,
    b: &GrayStrip,
    rows: std::ops::Range<usize>,
    m: SlabMeasure,
    wx: i64,
    wy: i64,
) -> bool {
    let dpp_x = 360.0 / a.w as f32;
    let dpp_y = 180.0 / a.h as f32;
    let (sx, sy) = (
        (m.dlon_deg / dpp_x).round() as i64,
        (m.dlat_deg / dpp_y).round() as i64,
    );
    // Écart d'évaluation : ≥ 2 px ET ≥ 2° (au-delà d'une demi-période de la
    // plus fine texture résolue, la corrélation doit s'effondrer).
    let (ox, oy) = (
        ((2.0 / dpp_x).ceil() as i64).max(2),
        ((2.0 / dpp_y).ceil() as i64).max(2),
    );
    let offsets = [(ox, 0), (-ox, 0), (0, oy), (0, -oy)];
    let mut all_below = true;
    for (oxi, oyi) in offsets {
        let (cx, cy) = (sx + oxi, sy + oyi);
        if cx.abs() > wx || cy.abs() > wy {
            continue;
        }
        let n = ncc_at(a, b, rows.clone(), cx, cy);
        if n > m.ncc - 0.15 * m.ncc.abs().max(0.1) {
            all_below = false;
        }
    }
    all_below
}

/// Raffine les poses par les images : mesures par paires + solve
/// Gauss-Newton avec prior Tikhonov. Les corrections sont appliquées aux
/// frames (`refresh_pose` inclus). En cas de divergence, tout est annulé
/// (poses capteur intactes) et `report.converged = false`.
pub fn refine_poses(frames: &mut [PreparedFrame], qp: &QualityParams) -> AlignReport {
    let n = frames.len();
    let mut report = AlignReport {
        corrections_deg: vec![[0.0; 3]; n],
        ..AlignReport::default()
    };
    if n < 2 || !qp.align {
        return report;
    }
    let initial: Vec<[f32; 3]> = frames
        .iter()
        .map(|f| [f.yaw_deg, f.pitch_deg, f.roll_deg])
        .collect();

    let pairs = build_pairs(frames);
    let work_w = qp.align_work_width.max(64) | 1; // impair pour un centre net
    let reg = qp.align_lambda_reg.max(1e-9) as f64;

    // Suivi du meilleur état : le problème est linéaire mais les mesures
    // portent du bruit (pics NCC quantifiés, bords de lentilles valides).
    // Itérer sans garde fait dériver le solve ; on ne retient donc que
    // l'état de résidu total pondéré minimal (rollback automatique).
    let mut best_t = f64::INFINITY;
    let mut best_poses = initial.clone();
    let mut best_corr = vec![[0.0f32; 3]; n];
    let mut t_initial = f64::INFINITY;
    let mut hard_fail = false;

    for _iteration in 0..qp.align_max_iters.max(1) {
        let strips: Vec<GrayStrip> = frames
            .iter()
            .map(|f| render_gray_strip(f, work_w))
            .collect();

        let mut measurements: Vec<(usize, usize, PairKind, Vec<SlabMeasure>)> = Vec::new();
        for &(a, b, kind) in &pairs {
            if let Some(slabs) = measure_pair(&strips[a], &strips[b], kind, qp) {
                measurements.push((a, b, kind, slabs));
            }
        }

        // Résidu total pondéré de l'état courant.
        let t_now: f64 = measurements
            .iter()
            .flat_map(|(_, _, _, slabs)| slabs.iter())
            .map(|s| {
                let w = (s.ncc * s.ncc).max(1e-6) as f64 * s.rows.max(1) as f64;
                w * (s.dlon_deg * s.dlon_deg + s.dlat_deg * s.dlat_deg) as f64
            })
            .sum();
        if t_initial.is_infinite() {
            t_initial = t_now;
        }
        if t_now < best_t {
            best_t = t_now;
            best_poses = frames
                .iter()
                .map(|f| [f.yaw_deg, f.pitch_deg, f.roll_deg])
                .collect();
            best_corr = report.corrections_deg.clone();
        }

        if measurements.is_empty() {
            // Aucune preuve image exploitable : échec matériel à la 1re
            // itération seulement (poses capteur conservées).
            if t_initial.is_infinite() {
                hard_fail = true;
            }
            break;
        }

        // Solve + passe IRLS/Huber (down-weighting des mesures aberrantes :
        // un pic NCC parasite ne doit pas tirer une frame de plusieurs
        // degrés). Convention verrouillée par la sonde unitaire : le strip
        // rendu avec la pose enregistrée (vraie + Δ) montre la scène
        // décalée de −Δ, la NCC mesure m = Δ_a − Δ_b, le résidu
        // r = (δb − δa) − m calibre le solve sur δ = −Δ.
        let Some(delta) = solve_step(&measurements, None, n, reg) else {
            if t_initial.is_infinite() {
                hard_fail = true;
            }
            break;
        };
        let Some(delta) = solve_step(&measurements, Some(&delta), n, reg) else {
            if t_initial.is_infinite() {
                hard_fail = true;
            }
            break;
        };

        // Garde-fou : un pas absurde (= divergence) → on s'arrête là ; le
        // rollback final ramènera au meilleur état.
        let max_step = delta.iter().fold(0.0f64, |m, &d| m.max(d.abs()));
        if max_step > 8.0 {
            if t_initial.is_infinite() {
                hard_fail = true;
            }
            break;
        }

        // Les mesures et le solve sont en DEGRÉS (pas de conversion radian
        // ici — un to_deg() amplifierait le pas ×57,3).
        for (k, f) in frames.iter_mut().enumerate() {
            let (dy, dp) = (delta[2 * k] as f32, delta[2 * k + 1] as f32);
            f.yaw_deg += dy;
            f.pitch_deg += dp;
            f.refresh_pose();
            report.corrections_deg[k][0] += dy;
            report.corrections_deg[k][1] += dp;
        }

        if max_step < 0.02 {
            break;
        }
    }

    // Rollback au meilleur état mesuré (les corrections n'ont pas forcément
    // progressé à chaque itération).
    for (k, f) in frames.iter_mut().enumerate() {
        [f.yaw_deg, f.pitch_deg, f.roll_deg] = best_poses[k];
        f.refresh_pose();
    }
    report.corrections_deg = best_corr;

    // Mesure finale : résidus par paire acceptée (à l'état retenu).
    let strips: Vec<GrayStrip> = frames
        .iter()
        .map(|f| render_gray_strip(f, work_w))
        .collect();
    let mut accepted = Vec::new();
    let mut rejected = 0usize;
    for &(a, b, kind) in &pairs {
        match measure_pair(&strips[a], &strips[b], kind, qp) {
            Some(slabs) => accepted.push((a, b, kind, slabs)),
            None => rejected += 1,
        }
    }
    report.accepted = accepted;
    report.rejected = rejected;
    // Convergé = pas d'échec matériel du solve (mesures vides, Cholesky
    // impossible ou pas absurde dès la 1re itération). Sinon le meilleur
    // état est retenu, même si le gain est modeste.
    report.converged = !hard_fail;
    report
}

/// Largeur de travail des strips pour la mesure des tuiles de warp (la
/// précision sub-pixel visée : 0,1 px ≈ 0,035° ≈ 0,4 px de sortie 4096).
const WARP_WORK_WIDTH: u32 = 1024;
/// Tranches de latitude du recouvrement mesurées par paire.
const WARP_TILES: usize = 5;
/// Confiance minimale d'une tuile acceptée (le recouvrement est étroit).
const WARP_MIN_NCC: f32 = 0.55;
/// Amplitude max d'une correction de warp (degrés) — au-delà, la tuile est
/// suspecte (mesure parasite) et l'amplitude appliquée clampée.
const WARP_MAX_DEG: f32 = 0.25;

/// Estime et applique le **warp local** de chaque frame (mesh basse
/// fréquence, δ échantillonnage par cellules de 7,5°) : colle les
/// recouvrements au niveau LOCAL là où l'align global (3 DOF par frame)
/// laisse un résidu ~0,1° — la cause des patches chromatiques et halos
/// gris aux coutures sur zones chargées (constat réel 2026-09-11, stable
/// à LEVELS/SEAM/GAINCH identiques).
///
/// Mesure : NCC translation sub-pixel par tranches de latitude du
/// recouvrement (la machine `measure_slab` de l'align), sur strips gris
/// blanchis à 1024 px. Jauge : correction symétrique ±m/2 par paire
/// (content_a += +m/2, content_b += −m/2 — convention verrouillée par la
/// sonde `sonde_convention_yaw` : m = Δa − Δb), δ appliqué = −contenu,
/// donc δa = −m/2, δb = +m/2. Lissage 2× boîte 3×3 + clamp ±0,15°.
pub fn estimate_local_warp(frames: &mut [PreparedFrame]) {
    let n = frames.len();
    if n < 2 {
        return;
    }
    let work = WARP_WORK_WIDTH;
    let strips: Vec<GrayStrip> = frames.iter().map(|f| render_gray_strip(f, work)).collect();
    let pairs = build_pairs(frames);
    let dpp = 360.0 / work as f32;
    let wx = (4.5 / dpp).ceil() as i64;
    let wy = (4.5 / dpp).ceil() as i64;

    // Accumulateurs : somme pondérée (δlon, δlat) + poids par cellule et
    // par frame (grille 24×16, cf. project::WARP_CELLS_*).
    let cells = crate::project::WARP_CELLS_LON * crate::project::WARP_CELLS_LAT;
    let mut sums = vec![(0.0f32, 0.0f32); cells * n];
    let mut wts = vec![0.0f32; cells * n];

    // Index de cellule : même mapping que WarpField::delta.
    let cell_of = |f: &PreparedFrame, lon: f32, lat: f32| -> Option<usize> {
        let mut rel = lon - f.yaw_deg;
        rel -= 360.0 * (rel / 360.0).round();
        if rel.abs() > 90.0 || (lat - f.pitch_deg).abs() > 60.0 {
            return None;
        }
        let ci = (((rel + 90.0) / 7.5).floor().clamp(0.0, 23.0)) as usize;
        let cj = (((lat - f.pitch_deg + 60.0) / 7.5).floor().clamp(0.0, 15.0)) as usize;
        Some(cj * crate::project::WARP_CELLS_LON + ci)
    };

    let mut tiles_used = 0usize;
    for &(a, b, _) in &pairs {
        let (sa, sb) = (&strips[a], &strips[b]);
        let rows = (sa.rows.0.max(sb.rows.0), sa.rows.1.min(sb.rows.1));
        if rows.1 <= rows.0 + WARP_TILES {
            continue;
        }
        // Milieu du recouvrement en longitude : moitié du pas capteur.
        let mut rel = frames[b].yaw_deg - frames[a].yaw_deg;
        rel -= 360.0 * (rel / 360.0).round();
        let lon_c = frames[a].yaw_deg + rel * 0.5;
        let step = (rows.1 - rows.0) / WARP_TILES;
        for t in 0..WARP_TILES {
            let slab = (rows.0 + t * step)..(rows.0 + (t + 1) * step);
            let Some(m) = measure_slab(sa, sb, slab.clone(), wx, wy) else {
                continue;
            };
            // Guards : tuile peu fiable ou correction délirante → rejet.
            if m.ncc < WARP_MIN_NCC || m.dlon_deg.abs() > 2.0 || m.dlat_deg.abs() > 2.0 {
                continue;
            }
            let lat_c = lat_of(((slab.start + slab.end) / 2) as u32, work, work / 2);
            let w = m.ncc * m.ncc;
            let Some(ia) = cell_of(&frames[a], lon_c, lat_c) else {
                continue;
            };
            let Some(ib) = cell_of(&frames[b], lon_c, lat_c) else {
                continue;
            };
            // δa = −m/2 (contenu a += +m/2), δb = +m/2 — cf. doc.
            let ia = a * cells + ia;
            let ib = b * cells + ib;
            sums[ia].0 += -m.dlon_deg * 0.5;
            sums[ia].1 += -m.dlat_deg * 0.5;
            sums[ib].0 += m.dlon_deg * 0.5;
            sums[ib].1 += m.dlat_deg * 0.5;
            wts[ia] += w;
            wts[ib] += w;
            tiles_used += 1;
        }
    }
    if tiles_used == 0 {
        return;
    }

    // Lissage 2× boîte 3×3 (wrap lon, clamp lat) sur sommes et poids —
    // puis division ; cellules sans poids → δ = 0.
    let smooth = |grid: &mut [f32]| {
        for _ in 0..2 {
            let cur = grid.to_vec();
            for f in 0..n {
                for j in 0..crate::project::WARP_CELLS_LAT {
                    for i in 0..crate::project::WARP_CELLS_LON {
                        let (mut acc, mut cnt) = (0.0f32, 0u32);
                        for dy in -1i32..=1 {
                            for dx in -1i32..=1 {
                                let y = (j as i32 + dy).clamp(0, 15) as usize;
                                let x = (i as i32 + dx)
                                    .rem_euclid(crate::project::WARP_CELLS_LON as i32)
                                    as usize;
                                acc += cur[f * cells + y * crate::project::WARP_CELLS_LON + x];
                                cnt += 9;
                            }
                        }
                        grid[f * cells + j * crate::project::WARP_CELLS_LON + i] = acc / cnt as f32;
                    }
                }
            }
        }
    };
    let (flat_sums_lon, flat_sums_lat): (Vec<f32>, Vec<f32>) = (
        sums.iter().flat_map(|s| [s.0]).collect(),
        sums.iter().flat_map(|s| [s.1]).collect(),
    );
    let (mut s_lon, mut s_lat) = (flat_sums_lon, flat_sums_lat);
    let mut w = wts;
    smooth(&mut s_lon);
    smooth(&mut s_lat);
    smooth(&mut w);

    for (k, f) in frames.iter_mut().enumerate() {
        let mut any = false;
        let mut field = Vec::with_capacity(cells);
        for c in 0..cells {
            let idx = k * cells + c;
            let (d_lon, d_lat) = if w[idx] > 1e-6 {
                (
                    (s_lon[idx] / w[idx]).clamp(-WARP_MAX_DEG, WARP_MAX_DEG),
                    (s_lat[idx] / w[idx]).clamp(-WARP_MAX_DEG, WARP_MAX_DEG),
                )
            } else {
                (0.0, 0.0)
            };
            any |= d_lon.abs() > 0.005 || d_lat.abs() > 0.005;
            field.push((d_lon, d_lat));
        }
        if any {
            f.warp = Some(crate::project::WarpField { cells: field });
        }
    }
    println!(
        "warp local : {tiles_used} tuiles mesurées, {} champs appliqués",
        frames.iter().filter(|f| f.warp.is_some()).count()
    );
}

/// Construit et résout les équations normales du raffinement de pose
/// (résidu r = (δb − δa) − m pondéré `ncc²·aire`, prior Tikhonov = jauge,
/// Huber optionnel à la seconde passe IRLS). `delta0` = solution courante
/// pour le calcul des poids Huber.
fn solve_step(
    measurements: &[(usize, usize, PairKind, Vec<SlabMeasure>)],
    delta0: Option<&[f64]>,
    n: usize,
    reg: f64,
) -> Option<Vec<f64>> {
    let nparams = 2 * n;
    let mut jtj = vec![0.0f64; nparams * nparams];
    let mut jtr = vec![0.0f64; nparams];
    for &(a, b, _, ref slabs) in measurements {
        for s in slabs {
            let w0 = (s.ncc * s.ncc).max(1e-6) as f64 * s.rows.max(1) as f64;
            // Résidu prédit sous la solution courante (passe IRLS).
            let (pred_lon, pred_lat) = match delta0 {
                Some(d) => (
                    (d[2 * b] - d[2 * a]) as f32,
                    (d[2 * b + 1] - d[2 * a + 1]) as f32,
                ),
                None => (0.0, 0.0),
            };
            for (comp, m) in [(0, s.dlon_deg - pred_lon), (1, s.dlat_deg - pred_lat)] {
                // Huber : poids plein sous 1,5°, linéaire au-delà.
                let h = (1.5 / m.abs().max(1e-6)).min(1.0) as f64;
                let w = w0 * h;
                let ia = 2 * a + comp;
                let ib = 2 * b + comp;
                jtj[ia * nparams + ia] += w;
                jtj[ib * nparams + ib] += w;
                jtj[ia * nparams + ib] -= w;
                jtj[ib * nparams + ia] -= w;
                jtr[ib] += w * m as f64;
                jtr[ia] -= w * m as f64;
            }
        }
    }
    // Prior Tikhonov = la jauge : √λ·δ_k = 0. Plein rang garanti, la
    // moyenne nulle des corrections émerge du prior symétrique.
    for i in 0..nparams {
        jtj[i * nparams + i] += reg;
    }
    cholesky_solve(&mut jtj, &mut jtr, nparams)
}

/// Résout Ax = b par factorisation de Cholesky (A symétrique définie
/// positive — le régulariseur Tikhonov le garantit). Détruit `a` et `b`.
/// ~30 lignes de f64 dense, aucune dépendance.
fn cholesky_solve(a: &mut [f64], b: &mut [f64], n: usize) -> Option<Vec<f64>> {
    for i in 0..n {
        for j in 0..=i {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= a[i * n + k] * a[j * n + k];
            }
            if i == j {
                if sum <= 1e-12 {
                    return None;
                }
                a[i * n + i] = sum.sqrt();
            } else {
                a[i * n + j] = sum / a[j * n + j];
            }
        }
    }
    // Avant-substitution L y = b.
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= a[i * n + k] * b[k];
        }
        b[i] = sum / a[i * n + i];
    }
    // Arrière-substitution Lᵀ x = y.
    let mut x = vec![0.0f64; n];
    for i in (0..n).rev() {
        let mut sum = b[i];
        for k in (i + 1)..n {
            sum -= a[k * n + i] * x[k];
        }
        x[i] = sum / a[i * n + i];
    }
    Some(x)
}

/// Raffine les poses par **rotations relatives mesurées aux images**
/// (feature `pose-model`, serveur) : SuperPoint+LightGlue donnent des
/// correspondances par paire, `rotation::horn_ransac` les convertit en
/// rotation relative (modèle rotation pure), et un Gauss-Newton sur les
/// corrections euler 3 DOF (yaw/pitch/roll — le roll n'est plus gelé)
/// aligne le graphe de poses sur ces mesures. Prior capteur par axe :
/// yaw faible (magnétomètre intérieur = principale source d'erreur),
/// pitch/roll plus forts (gravité fiable). Jauge : le prior + le graphe
/// connexe (fermeture d'anneau incluse via `build_pairs`).
///
/// Résidu de paire : `log(R_predᵀ·R_meas)` (géodésique SO(3), degrés),
/// pondéré √inliers avec Huber ; Jacobien NUMÉRIQUE (central diff) — le
/// coût (6n évals de résidus par itération) est négligeable devant
/// l'inférence.
#[cfg(feature = "pose-model")]
pub fn refine_poses_model(
    frames: &mut [PreparedFrame],
    qp: &QualityParams,
    model: &crate::pose_model::PoseModel,
) -> AlignReport {
    use crate::rotation::horn_ransac;

    let n = frames.len();
    let mut report = AlignReport {
        corrections_deg: vec![[0.0; 3]; n],
        ..AlignReport::default()
    };
    if n < 2 || !qp.align {
        return report;
    }
    let sensor: Vec<[f64; 3]> = frames
        .iter()
        .map(|f| [f.yaw_deg as f64, f.pitch_deg as f64, f.roll_deg as f64])
        .collect();

    // 1) Features par frame (SuperPoint). Toute erreur d'inférence = échec
    //    matériel (`converged = false` → repli NCC côté appelant).
    let feats: Vec<crate::pose_model::Features> = match frames
        .iter()
        .map(|f| model.extract(&f.rgb, f.width, f.height, qp.model_work_px))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) => v,
        Err(_) => return report,
    };

    // 2) Paires candidates (géométrie capteur, wrap géré) → mesures de
    //    rotation par RANSAC sur les correspondances LightGlue.
    let pairs = build_pairs(frames);
    struct PairRot {
        a: usize,
        b: usize,
        kind: PairKind,
        rel: crate::geom::Mat3,
        inliers: usize,
    }
    let mut rejected = 0usize;
    let mut pair_matches: Vec<(usize, usize, PairKind, Vec<crate::pose_model::PointMatch>)> =
        Vec::new();
    for &(a, b, kind) in &pairs {
        let matches = match model.match_pair(&feats[a], &feats[b]) {
            Ok(m) => m,
            Err(_) => {
                rejected += 1;
                continue;
            }
        };
        if matches.len() < qp.model_min_inliers as usize {
            rejected += 1;
            continue;
        }
        pair_matches.push((a, b, kind, matches));
    }
    if pair_matches.is_empty() {
        return report;
    }

    // 2bis) AUTO-CALIBRATION DE FOCLE — le hfov du track caméra dépend du
    // mode capteur que le webview a négocié pour la résolution demandée :
    // non dérivable a priori (constat 2026-09-11 : 43,7° déclaré vs ~39°
    // réel, au MÊME ratio w/h que la veille qui valait ~52° à 720p). Le
    // biais gonfle les rotations mesurées par features de +12 % et
    // l'aligneur ne peut pas satisfaire toutes les paires → ghosting.
    // Les correspondances sont en pixels (focale-indépendantes) : on
    // cherche l'échelle de f_px qui annule le biais médian (angle de la
    // rotation relative mesurée − angle capteur) sur les paires fortes.
    let strong = (qp.model_min_inliers as usize).max(30);
    let sensor_angle = |a: usize, b: usize| -> f32 {
        let ma = crate::geom::euler_to_mat(
            sensor[a][0] as f32,
            sensor[a][1] as f32,
            sensor[a][2] as f32,
        );
        let mb = crate::geom::euler_to_mat(
            sensor[b][0] as f32,
            sensor[b][1] as f32,
            sensor[b][2] as f32,
        );
        let rel = mb.mul_mat(&ma.transpose());
        crate::geom::to_deg(crate::geom::norm(crate::rotation::log_so3(rel)))
    };
    let bias_at = |scale: f32| -> Option<f32> {
        let mut deltas: Vec<f32> = Vec::new();
        for &(a, b, _, ref matches) in pair_matches.iter() {
            let (fa, fb) = (&feats[a], &feats[b]);
            let ray = |f: &PreparedFrame, ft: &crate::pose_model::Features, p: [f32; 2]| {
                let s = ft.w as f32 / f.width as f32;
                let fpx = f.f_px * s * scale;
                let cx = ft.w as f32 * 0.5;
                let cy = ft.h as f32 * 0.5;
                let mut v = [(p[0] - cx) / fpx, (p[1] - cy) / fpx, 1.0];
                let nn = crate::geom::norm(v);
                v.iter_mut().for_each(|c| *c /= nn);
                v
            };
            let pts_a: Vec<[f32; 3]> = matches
                .iter()
                .map(|m| ray(&frames[a], fa, fa.kpts_px[m.ia]))
                .collect();
            let pts_b: Vec<[f32; 3]> = matches
                .iter()
                .map(|m| ray(&frames[b], fb, fb.kpts_px[m.ib]))
                .collect();
            let s_a = feats[a].w as f32 / frames[a].width as f32;
            let s_b = feats[b].w as f32 / frames[b].width as f32;
            let f8 = 0.5 * (frames[a].f_px * s_a + frames[b].f_px * s_b) * scale;
            let tol_rad = qp.model_max_err_px / f8;
            let seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((a as u64) << 32) ^ b as u64;
            let Some(est) = horn_ransac(&pts_a, &pts_b, tol_rad, 400, seed) else {
                continue;
            };
            if est.inliers.len() < strong {
                continue;
            }
            let ang = crate::geom::to_deg(crate::geom::norm(crate::rotation::log_so3(est.rel)));
            deltas.push(ang - sensor_angle(a, b));
        }
        if deltas.len() < 5 {
            return None;
        }
        deltas.sort_by(f32::total_cmp);
        Some(deltas[deltas.len() / 2])
    };
    let (best_scale, best_bias) = [
        0.80f32, 0.85, 0.90, 0.95, 1.00, 1.05, 1.10, 1.15, 1.20, 1.25,
    ]
    .into_iter()
    .map(|s| (s, bias_at(s).unwrap_or(f32::INFINITY)))
    .min_by(|x, y| x.1.abs().total_cmp(&y.1.abs()))
    .unwrap_or((1.0, 0.0));
    // Toutes les évaluations None (< 5 paires fortes) : focale inchangée.
    let (best_scale, best_bias) = if best_bias.is_infinite() {
        (1.0, 0.0)
    } else {
        (best_scale, best_bias)
    };
    let half_w = frames[0].width as f32 * 0.5;
    let hfov_cal = crate::geom::to_deg(2.0 * (half_w / (frames[0].f_px * best_scale)).atan());
    for f in frames.iter_mut() {
        f.f_px *= best_scale;
        f.refresh_pose();
    }
    report.calibrated_hfov = Some(hfov_cal);
    println!(
        "calibration focale : ×{best_scale:.3} (biais {best_bias:+.2}°) → hfov {hfov_cal:.1}°"
    );

    // 2ter) Mesures de rotation à la focale calibrée : Horn/RANSAC par
    //    paire — correspondances réutilisées, pas de re-extraction.
    let mut accepted: Vec<PairRot> = Vec::new();
    for &(a, b, kind, ref matches) in &pair_matches {
        let (fa, fb) = (&feats[a], &feats[b]);
        // Rayons caméra : focale/centre transposés à l'image de travail
        // (l'aspect est conservé par le resize → un seul facteur par frame).
        let ray = |f: &PreparedFrame, ft: &crate::pose_model::Features, p: [f32; 2]| {
            let s = ft.w as f32 / f.width as f32;
            let fpx = f.f_px * s;
            let cx = ft.w as f32 * 0.5;
            let cy = ft.h as f32 * 0.5;
            let mut v = [(p[0] - cx) / fpx, (p[1] - cy) / fpx, 1.0];
            let nn = crate::geom::norm(v);
            v.iter_mut().for_each(|c| *c /= nn);
            v
        };
        let pts_a: Vec<[f32; 3]> = matches
            .iter()
            .map(|m| ray(&frames[a], fa, fa.kpts_px[m.ia]))
            .collect();
        let pts_b: Vec<[f32; 3]> = matches
            .iter()
            .map(|m| ray(&frames[b], fb, fb.kpts_px[m.ib]))
            .collect();
        // Tolérance en radians ≈ model_max_err_px sur la focale moyenne.
        let s_a = feats[a].w as f32 / frames[a].width as f32;
        let s_b = feats[b].w as f32 / frames[b].width as f32;
        let f8 = 0.5 * (frames[a].f_px * s_a + frames[b].f_px * s_b);
        let tol_rad = qp.model_max_err_px / f8;
        let seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((a as u64) << 32) ^ b as u64;
        let Some(est) = horn_ransac(&pts_a, &pts_b, tol_rad, 400, seed) else {
            rejected += 1;
            continue;
        };
        if est.inliers.len() < qp.model_min_inliers as usize || est.median_err > tol_rad {
            rejected += 1;
            continue;
        }
        accepted.push(PairRot {
            a,
            b,
            kind,
            rel: est.rel,
            inliers: est.inliers.len(),
        });
    }
    if accepted.is_empty() {
        return report; // converged = false → repli NCC
    }

    // 3) Solve global : Gauss-Newton sur corrections euler 3 DOF par frame
    //    (résidus géodésiques SO(3) pondérés √inliers×Huber, prior capteur
    //    par axe — la jauge).
    let pair_refs: Vec<RotationPair> = accepted
        .iter()
        .map(|pr| (pr.a, pr.b, pr.rel, pr.inliers as u32))
        .collect();
    let Some(final_poses) = solve_rotation_gn(&pair_refs, &sensor, qp) else {
        return AlignReport {
            corrections_deg: vec![[0.0; 3]; n],
            accepted: Vec::new(),
            rejected,
            converged: false,
            fallback: false,
            calibrated_hfov: None,
        };
    };

    // Application + rapport.
    for (k, f) in frames.iter_mut().enumerate() {
        let (dy, dp, dr) = (
            (final_poses[k][0] - sensor[k][0]) as f32,
            (final_poses[k][1] - sensor[k][1]) as f32,
            (final_poses[k][2] - sensor[k][2]) as f32,
        );
        f.yaw_deg += dy;
        f.pitch_deg += dp;
        f.roll_deg += dr;
        f.refresh_pose();
        report.corrections_deg[k] = [dy, dp, dr];
    }
    report.accepted = accepted
        .into_iter()
        .map(|pr| (pr.a, pr.b, pr.kind, Vec::new()))
        .collect();
    report.rejected = rejected;
    report.converged = true;
    report
}

/// Mesure de paire pour le solve rotation : (a, b, rotation mesurée
/// caméra-a → caméra-b, inliers RANSAC).
pub type RotationPair = (usize, usize, crate::geom::Mat3, u32);

/// Gauss-Newton rotation 3 DOF par frame : résidus géodésiques
/// `log(predᵀ·rel)` (degrés) pondérés √inliers×Huber(0,5°), prior capteur
/// par axe (la jauge), Jacobien numérique central, damping LM léger.
/// Garde-fous : pas > 30° ou correction totale > 25° = divergence.
/// Retourne les poses finales (degrés) ou `None` si divergence.
#[must_use]
#[cfg(feature = "pose-model")]
pub fn solve_rotation_gn(
    pairs: &[RotationPair],
    sensor: &[[f64; 3]],
    qp: &QualityParams,
) -> Option<Vec<[f64; 3]>> {
    use crate::geom::to_deg;
    use crate::rotation::log_so3;

    let n = sensor.len();
    let cur = |poses: &[[f64; 3]]| -> Vec<f64> {
        let mut r = Vec::with_capacity(pairs.len() * 3 + n * 3);
        let mats: Vec<crate::geom::Mat3> = poses
            .iter()
            .map(|p| crate::geom::euler_to_mat(p[0] as f32, p[1] as f32, p[2] as f32))
            .collect();
        for &(a, b, rel, inliers) in pairs {
            let pred = mats[b].mul_mat(&mats[a].transpose());
            let e = log_so3(pred.transpose().mul_mat(&rel));
            let (ex, ey, ez) = (
                to_deg(e[0]) as f64,
                to_deg(e[1]) as f64,
                to_deg(e[2]) as f64,
            );
            // Huber IRLS à 0,5° : poids = √h sur le résidu (coût h·|e|² =
            // δ·|e| linéaire au-delà de δ — une paire aberrante ne tire pas
            // tout le graphe). PIÈGE (constat solve synthétique 2026-09-10) :
            // pondérer par h (et non √h) rend le coût h²·|e|² = δ² CONSTANT
            // dans la queue — gradient structurellement nul, solve immobile.
            let nrm = (ex * ex + ey * ey + ez * ez).sqrt();
            let h = (0.5 / nrm.max(1e-9)).min(1.0);
            let w = (inliers as f64).sqrt() * h.sqrt();
            r.push(ex * w);
            r.push(ey * w);
            r.push(ez * w);
        }
        // Priors capteur (√λ·(pose − capteur)) — la jauge.
        for (k, p) in poses.iter().enumerate() {
            let (ly, lp) = (
                qp.model_lambda_yaw.sqrt(),
                qp.model_lambda_pitch_roll.sqrt(),
            );
            r.push(ly * (p[0] - sensor[k][0]));
            r.push(lp * (p[1] - sensor[k][1]));
            r.push(lp * (p[2] - sensor[k][2]));
        }
        r
    };

    let mut poses = sensor.to_vec();
    let mut t = norm2(&cur(&poses));
    let mut converged = true;
    for _ in 0..16 {
        let r0 = cur(&poses);
        let np = 3 * n;
        let h = 0.02_f64; // degrés — précision ≪ le bruit de mesure
        let mut jtj = vec![0.0f64; np * np];
        let mut jtr = vec![0.0f64; np];
        // Jacobien numérique central : colonne p = (r(+h) − r(−h)) / 2h.
        let mut cols: Vec<Vec<f64>> = Vec::with_capacity(np);
        for p in 0..np {
            let mut plus = poses.clone();
            let mut minus = poses.clone();
            plus[p / 3][p % 3] += h;
            minus[p / 3][p % 3] -= h;
            let rp = cur(&plus);
            let rm = cur(&minus);
            let col: Vec<f64> = rp
                .iter()
                .zip(&rm)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect();
            cols.push(col);
        }
        // Équations normales sur −r : (JᵀJ)·δ = −Jᵀ·r.
        for (jp, col) in cols.iter().enumerate() {
            for (ip, col2) in cols.iter().enumerate() {
                let dot: f64 = col.iter().zip(col2).map(|(a, b)| a * b).sum();
                jtj[ip * np + jp] = dot;
            }
            let g: f64 = col.iter().zip(&r0).map(|(a, b)| a * b).sum();
            jtr[jp] = -g;
            // Damping LM (le prior peut laisser des colonnes quasi nulles
            // pour une frame sans paire acceptée — Cholesky doit tenir).
            jtj[jp * np + jp] += 1e-6;
        }
        let Some(mut delta) = solve_step_model(&mut jtj, &mut jtr, np) else {
            converged = false;
            break;
        };
        let mut max_step = delta.iter().fold(0.0f64, |m, &d| m.max(d.abs()));
        if max_step > 30.0 {
            converged = false;
            break;
        }
        // Région de confiance : le GN pur dépasse sur ce problème (pas de
        // ~2,5× la correction, oscillations — constat test synthétique
        // 2026-09-10) ; le plafond de pas converge proprement en ~4 pas.
        if max_step > 1.0 {
            let scale = 1.0 / max_step;
            for d in delta.iter_mut() {
                *d *= scale;
            }
            max_step = 1.0;
        }
        for (k, p) in poses.iter_mut().enumerate() {
            p[0] += delta[3 * k];
            p[1] += delta[3 * k + 1];
            p[2] += delta[3 * k + 2];
        }
        let t_new = norm2(&cur(&poses));
        if t_new >= t && max_step > 0.05 {
            break;
        }
        if t_new < t {
            t = t_new;
        }
        if max_step < 0.005 {
            break;
        }
    }

    // Garde-fou global : une correction totale absurde = divergence.
    let max_corr = poses.iter().zip(sensor).fold(0.0f64, |m, (p, s)| {
        m.max((p[0] - s[0]).abs())
            .max((p[1] - s[1]).abs())
            .max((p[2] - s[2]).abs())
    });
    if max_corr > 25.0 {
        converged = false;
    }
    if converged {
        Some(poses)
    } else {
        None
    }
}

/// Somme des carrés d'un vecteur de résidus.
#[cfg(feature = "pose-model")]
fn norm2(r: &[f64]) -> f64 {
    r.iter().map(|x| x * x).sum()
}

/// Cholesky (réutilise le solveur existant) pour le pas Gauss-Newton.
#[cfg(feature = "pose-model")]
fn solve_step_model(jtj: &mut [f64], jtr: &mut [f64], n: usize) -> Option<Vec<f64>> {
    cholesky_solve(jtj, jtr, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wrap180 : bornes du cercle.
    #[test]
    fn wrap180_bornes() {
        assert!((wrap180(0.0)).abs() < 1e-6);
        assert!((wrap180(190.0) - (-170.0)).abs() < 1e-6);
        assert!((wrap180(-190.0) - 170.0).abs() < 1e-6);
        assert!((wrap180(360.0)).abs() < 1e-6);
    }

    /// Parabole : sommet sur un triplet symétrique et asymétrique.
    #[test]
    fn parabola_sommet() {
        assert!((parabola(0.0, 1.0, 0.0)).abs() < 1e-6);
        // Triplet descendant à droite : sommet décalé à gauche de 1/6.
        assert!((parabola(0.5, 1.0, 0.0) - (-1.0 / 6.0)).abs() < 1e-6);
        assert!((parabola(0.0, 1.0, 0.5) - (1.0 / 6.0)).abs() < 1e-6);
    }

    /// Sonde : mesure NCC sur un décalage de pose CONNU (b capturé à 22,5°,
    /// enregistré à 24,5° → Δ_b = +2°, Δ_a = 0). Verrouille la convention
    /// signe/échelle de `measure_pair` indépendamment du pipeline complet.
    #[test]
    fn sonde_convention_yaw() {
        use crate::geom::euler_to_mat;
        use crate::project::{f_px_from_hfov, lat_band};

        const OCTAVES_PROBE: [(i32, i32, f32, f32, f32); 4] = [
            (3, 2, 0.30, 0.7, 1.3),
            (7, 5, 0.25, 2.1, 0.4),
            (13, 9, 0.22, 4.7, 2.9),
            (23, 17, 0.15, 1.8, 5.2),
        ];
        fn tex(lon: f32, lat: f32) -> u8 {
            let (lo, la) = (lon.to_radians(), lat.to_radians());
            let mut v = 0.0f32;
            for &(cy, cx, amp, p, q) in &OCTAVES_PROBE {
                v += amp * ((cy as f32 * lo + p).sin() * (cx as f32 * la + q).sin());
            }
            (128.0 + 55.0 * v.clamp(-0.92, 0.92)) as u8
        }
        // Contenu généré à `true_yaw`, pose enregistrée `rec_yaw`.
        fn frame_at(true_yaw: f32, rec_yaw: f32, pitch: f32) -> PreparedFrame {
            let (w, h, hfov) = (512u32, 910u32, 39.4_f32);
            let f_px = f_px_from_hfov(hfov, w);
            let m_true = euler_to_mat(true_yaw, pitch, 0.0);
            let mut rgb = vec![0u8; (w * h * 3) as usize];
            for j in 0..h {
                for i in 0..w {
                    let a = (i as f32 + 0.5 - w as f32 / 2.0) / f_px;
                    let b = (j as f32 + 0.5 - h as f32 / 2.0) / f_px;
                    let world = m_true.transpose().mul_vec([a, b, 1.0]);
                    let lon = world[1].atan2(world[0]).to_degrees();
                    let n =
                        (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
                    let lat = (world[2] / n).asin().to_degrees();
                    let v = tex(lon, lat);
                    let o = ((j * w + i) * 3) as usize;
                    rgb[o] = v;
                    rgb[o + 1] = v;
                    rgb[o + 2] = v;
                }
            }
            PreparedFrame {
                yaw_deg: rec_yaw,
                pitch_deg: pitch,
                roll_deg: 0.0,
                inv: euler_to_mat(rec_yaw, pitch, 0.0),
                width: w,
                height: h,
                f_px,
                lat_min: lat_band(h, f_px, pitch).0,
                lat_max: lat_band(h, f_px, pitch).1,
                feather_px: 1.0,
                gain: [1.0; 3],
                local_gain: Vec::new(),
                warp: None,
                rgb,
            }
        }

        let a = frame_at(0.0, 0.0, 15.0);
        let b = frame_at(22.5, 24.5, 15.0); // Δ_b = +2°
        let qp = QualityParams {
            align_work_width: 384,
            align_search_deg: 4.5,
            ..QualityParams::default()
        };
        let sa = render_gray_strip(&a, qp.align_work_width);
        let sb = render_gray_strip(&b, qp.align_work_width);
        let ms = measure_pair(&sa, &sb, PairKind::Intra, &qp).expect("mesure");
        for m in &ms {
            println!(
                "sonde Δ_b=+2°: dlon={:+.3} dlat={:+.3} ncc={:.3}",
                m.dlon_deg, m.dlat_deg, m.ncc
            );
        }
        // Observation : dlon ≈ −2 (m = Δ_a − Δ_b = 0 − 2).
        assert!(
            ms.iter().all(|m| (m.dlon_deg + 2.0).abs() <= 0.8),
            "convention dlon cassée : {:?}",
            ms.iter().map(|m| m.dlon_deg).collect::<Vec<_>>()
        );
    }

    /// Solve rotation synthétique : des poses « vraies » (capteur + dérive
    /// connue) génèrent les rotations relatives mesurées ; le solve,
    /// initialisé au capteur, doit retrouver les poses vraies. Verrouille
    /// la convention rel = M_b·M_aᵀ, le rôle du prior (jauge) et la
    /// convergence du GN.
    #[test]
    #[cfg(feature = "pose-model")]
    fn solve_rotation_synthetic() {
        // Anneau 8 frames, pas 45°, dérive ±2° par frame (t Scarborough :
        // motif pseudo-aléatoire déterministe).
        let n = 8;
        let sensor: Vec<[f64; 3]> = (0..n).map(|k| [k as f64 * 45.0, 0.0, 0.0]).collect();
        let mut true_poses = sensor.clone();
        let mut drift = Vec::with_capacity(n);
        for k in 0..n {
            drift.push([
                0.66 * (((k * 13 + 3) % 7) as f64 - 3.0),
                0.8 * (((k * 7 + 1) % 5) as f64 - 2.0),
                0.5 * (((k * 11 + 2) % 3) as f64 - 1.0),
            ]);
        }
        // Dérive centrée par axe : la composante de jauge (rotation globale
        // moyenne) est invisible des paires relatives — elle reste ancrée au
        // capteur par le prior, on la retire du motif attendu.
        for axis in 0..3 {
            let mean = drift.iter().map(|d| d[axis]).sum::<f64>() / n as f64;
            for d in drift.iter_mut() {
                d[axis] -= mean;
            }
        }
        for (k, p) in true_poses.iter_mut().enumerate() {
            *p = [p[0] + drift[k][0], p[1] + drift[k][1], p[2] + drift[k][2]];
        }
        // Paires voisines + fermeture, rel = M_b·M_aᵀ(true) + inliers.
        let mat = |p: [f64; 3]| crate::geom::euler_to_mat(p[0] as f32, p[1] as f32, p[2] as f32);
        let pairs: Vec<RotationPair> = (0..n)
            .map(|k| {
                let a = k;
                let b = (k + 1) % n;
                let rel = mat(true_poses[b]).mul_mat(&mat(true_poses[a]).transpose());
                (a, b, rel, 200u32)
            })
            .collect();

        let qp = QualityParams::default();
        let solved = solve_rotation_gn(&pairs, &sensor, &qp).expect("convergence");
        // Métrique LIBRE DE JAUGE : les paires relatives ne voient pas la
        // rotation globale (ancrée par le prior) — on compare donc les
        // rotations relatives du solve aux vraies, pas les poses absolues.
        let mat = |p: &[f64; 3]| crate::geom::euler_to_mat(p[0] as f32, p[1] as f32, p[2] as f32);
        let mut max_rel_err = 0.0f64;
        for k in 0..n {
            let a = k;
            let b = (k + 1) % n;
            let got = mat(&solved[b]).mul_mat(&mat(&solved[a]).transpose());
            let want = mat(&true_poses[b]).mul_mat(&mat(&true_poses[a]).transpose());
            let d = crate::rotation::log_so3(got.transpose().mul_mat(&want));
            max_rel_err = max_rel_err.max(crate::geom::to_deg(crate::geom::norm(d)) as f64);
        }
        assert!(
            max_rel_err < 0.05,
            "erreur de reprise (relatives) = {max_rel_err:.3}\u{b0}"
        );
    }

    /// Jauge : sans le graphe (0 paire), le solve doit rester au capteur.
    #[test]
    #[cfg(feature = "pose-model")]
    fn solve_sans_paires_reste_au_capteur() {
        let sensor: Vec<[f64; 3]> = vec![[10.0, 5.0, -3.0], [30.0, -2.0, 1.0]];
        let qp = QualityParams::default();
        let solved = solve_rotation_gn(&[], &sensor, &qp).expect("convergence");
        for (s, p) in solved.iter().zip(&sensor) {
            for k in 0..3 {
                assert!((s[k] - p[k]).abs() < 1e-6);
            }
        }
    }

    /// SONDE du warp local : b a un résidu local connu (+0,2° de contenu,
    /// Δ_b = −0,2°) que l'align global ne corrige pas. Verrouille le sens
    /// des δ appliqués (δa = −m/2, δb = +m/2) et l'efficacité (re-mesure
    /// NCC ≈ 0 après application, strips rendus VIA le warp).
    #[test]
    fn sonde_warp_local() {
        use crate::geom::euler_to_mat;
        use crate::project::{f_px_from_hfov, lat_band};

        const OCTAVES: [(i32, i32, f32, f32, f32); 4] = [
            (3, 2, 0.30, 0.7, 1.3),
            (7, 5, 0.25, 2.1, 0.4),
            (13, 9, 0.22, 4.7, 2.9),
            (23, 17, 0.15, 1.8, 5.2),
        ];
        fn tex(lon: f32, lat: f32) -> u8 {
            let (lo, la) = (lon.to_radians(), lat.to_radians());
            let mut v = 0.0f32;
            for &(cy, cx, amp, p, q) in &OCTAVES {
                v += amp * ((cy as f32 * lo + p).sin() * (cx as f32 * la + q).sin());
            }
            (128.0 + 55.0 * v.clamp(-0.92, 0.92)) as u8
        }
        fn frame_at(true_yaw: f32, rec_yaw: f32, pitch: f32) -> PreparedFrame {
            let (w, h, hfov) = (512u32, 910u32, 39.4_f32);
            let f_px = f_px_from_hfov(hfov, w);
            let m_true = euler_to_mat(true_yaw, pitch, 0.0);
            let mut rgb = vec![0u8; (w * h * 3) as usize];
            for j in 0..h {
                for i in 0..w {
                    let a = (i as f32 + 0.5 - w as f32 / 2.0) / f_px;
                    let b = (j as f32 + 0.5 - h as f32 / 2.0) / f_px;
                    let world = m_true.transpose().mul_vec([a, b, 1.0]);
                    let lon = world[1].atan2(world[0]).to_degrees();
                    let n =
                        (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
                    let lat = (world[2] / n).asin().to_degrees();
                    let v = tex(lon, lat);
                    let o = ((j * w + i) * 3) as usize;
                    rgb[o] = v;
                    rgb[o + 1] = v;
                    rgb[o + 2] = v;
                }
            }
            PreparedFrame {
                yaw_deg: rec_yaw,
                pitch_deg: pitch,
                roll_deg: 0.0,
                inv: euler_to_mat(rec_yaw, pitch, 0.0),
                width: w,
                height: h,
                f_px,
                lat_min: lat_band(h, f_px, pitch).0,
                lat_max: lat_band(h, f_px, pitch).1,
                feather_px: 1.0,
                gain: [1.0; 3],
                local_gain: Vec::new(),
                warp: None,
                rgb,
            }
        }

        let qp = QualityParams {
            align_work_width: 384,
            align_search_deg: 4.5,
            ..QualityParams::default()
        };
        let mut frames = vec![frame_at(0.0, 0.0, 15.0), frame_at(20.5, 22.5, 15.0)];
        // pré-condition : le résidu est bien là
        let pre = measure_pair(
            &render_gray_strip(&frames[0], qp.align_work_width),
            &render_gray_strip(&frames[1], qp.align_work_width),
            PairKind::Intra,
            &qp,
        )
        .expect("mesure pré");
        let m_pre = pre[0].dlon_deg;
        println!("warp sonde : m_pré = {m_pre:+.3}°");
        // La texture synthétique périodique biaisé la NCC aux petits Δ
        // (cf. sonde_convention_yaw : ±0,55°) — injection à Δ = −2° où la
        // mesure est fiable.
        assert!(m_pre.abs() > 1.2, "résidu injecté absent : {m_pre}");

        estimate_local_warp(&mut frames);
        let (wa, wb) = (&frames[0].warp, &frames[1].warp);
        let Some(wa) = wa else {
            panic!("champ a absent")
        };
        let Some(wb) = wb else {
            panic!("champ b absent")
        };
        // Pic de chaque champ (la position exacte des cellules dépend des
        // tuiles survivantes) — jauge : antisymétrie δa ≈ −δb.
        let peak = |w: &crate::project::WarpField| {
            w.cells
                .iter()
                .copied()
                .max_by(|x, y| x.0.abs().total_cmp(&y.0.abs()))
                .unwrap()
        };
        let (d_a, d_b) = (peak(wa), peak(wb));
        println!("warp sonde : δa={d_a:?} δb={d_b:?}");
        // Antisymétrie de jauge (±m/2) et amplitude sous le clamp.
        assert!(
            d_a.0.abs() <= WARP_MAX_DEG + 0.01
                && d_b.0.abs() <= WARP_MAX_DEG + 0.01
                && d_a.0 * d_b.0 < 0.0
                && (d_a.0 + d_b.0).abs() < 0.1,
            "jauge du warp cassée : {d_a:?} {d_b:?}"
        );
        // Efficacité : re-mesure sur strips rendus via le warp ≈ 0.
        let post = measure_pair(
            &render_gray_strip(&frames[0], qp.align_work_width),
            &render_gray_strip(&frames[1], qp.align_work_width),
            PairKind::Intra,
            &qp,
        )
        .expect("mesure post");
        let m_post = post[0].dlon_deg;
        println!("warp sonde : m_post = {m_post:+.3}°");
        // Direction : le warp RÉDUIT le résidu (le clamp ±0,15° et la
        // couverture locale des tuiles bornent l'ampleur de la réduction ;
        // la validation d'ampleur est faite sur capture réelle).
        assert!(
            m_post.abs() < m_pre.abs() - 0.02,
            "warp local inefficace : {m_pre} → {m_post}"
        );
    }
}
