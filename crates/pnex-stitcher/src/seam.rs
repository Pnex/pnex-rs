//! Coutures routées par le contenu (pipeline Quality).
//!
//! Le blend « sélection » du pipeline Legacy coupe implicitement au
//! bissecteur des centralités : une couture fixe, aveugle au contenu, qui
//! tranche les objets en parallaxe. Ici on **route** la coupe : par paire
//! au recouvrement suffisant, une programmation dynamique par ligne cherche
//! le chemin de coût minimal — coût pixel = différence de couleur entre les
//! deux frames (gainées) — avec lissage implicite `|Δs| ≤ 1` par ligne
//! (famille graph-cut/Dijkstra d'OpenCV et stitchEm, en DP séquentiel).
//!
//! Sortie : [`OwnerMap`], la carte « chaque pixel appartient à UNE frame »
//! que consomme [`crate::multiband::blend`] (masque binaire par frame).
//! Pixels hors bande / trous → [`NO_OWNER`].

use std::collections::HashMap;

use crate::align::build_pairs;
use crate::project::{interior_weight, project_uvc, PreparedFrame};
use crate::render::{lat_of, lon_of, sample_bilinear};
use crate::QualityParams;

/// Valeur « personne ne couvre ce pixel » dans [`OwnerMap::owner`].
pub const NO_OWNER: u16 = u16::MAX;

/// Carte de propriété : pour chaque pixel de l'équirect de sortie, la frame
/// qui le fournit (index d'entrée) ou [`NO_OWNER`].
#[derive(Debug, Clone)]
pub struct OwnerMap {
    pub width: u32,
    pub height: u32,
    /// `width·height` index de frame, rangée-major.
    pub owner: Vec<u16>,
}

impl OwnerMap {
    /// Nombre de pixels couverts par ligne (métrique de couverture de bande).
    #[must_use]
    pub fn covered_per_row(&self) -> Vec<u32> {
        self.owner
            .chunks(self.width as usize)
            .map(|row| row.iter().filter(|&&o| o != NO_OWNER).count() as u32)
            .collect()
    }
}

/// Coupe trouvée pour une ligne d'une paire : intervalle de double
/// couverture `[x0, x1)` (colonnes vues par les deux frames) et colonne de
/// coupe `s` (à `a` pour `col < s`, à `b` sinon). Colonnes absolues à la
/// résolution de travail des coutures.
#[derive(Debug, Clone, Copy)]
struct SeamRow {
    x0: u32,
    x1: u32,
    seam: u32,
}

/// Tables de couture par paire (clé `(a, b)` avec a < b).
type SeamTables = HashMap<(usize, usize), Vec<Option<SeamRow>>>;

/// Calcule les coutures par paire puis résout la carte de propriété.
#[must_use]
pub fn find_seams(frames: &[PreparedFrame], out_width: u32, qp: &QualityParams) -> OwnerMap {
    let w = out_width;
    let h = (out_width / 2).max(1);
    let mut owner = vec![NO_OWNER; (w * h) as usize];
    if frames.is_empty() || frames.len() > usize::from(u16::MAX - 1) {
        return OwnerMap {
            width: w,
            height: h,
            owner,
        };
    }

    let seam_w = if qp.seam_half_res {
        (w / 2).max(64) & !1
    } else {
        w
    };
    let tables = compute_seam_tables(frames, seam_w);

    // Passe unique sur l'équirect : couverture simple → propriétaire évident ;
    // double couverture → coupe par la table de couture de la paire (les deux
    // frames au poids intérieur maximal) ; fallback = poids max (comportement
    // Legacy) hors des régions à double couverture tabulées.
    let row_len = w as usize;
    for j in 0..h {
        let lat = lat_of(j, h);
        let eligible: Vec<usize> = frames
            .iter()
            .enumerate()
            .filter(|(_, f)| lat >= f.lat_min && lat <= f.lat_max)
            .map(|(k, _)| k)
            .collect();
        if eligible.is_empty() {
            continue;
        }
        let seam_j = ((j as u64 * seam_w as u64) / w as u64) as usize;
        let scale = w as f32 / seam_w as f32;
        for i in 0..w {
            // Projette dans les frames éligibles, garde les 2 meilleurs poids.
            let (mut b1, mut w1, mut b2, mut w2) = (usize::MAX, 0.0f32, usize::MAX, 0.0f32);
            for &k in &eligible {
                let f = &frames[k];
                let Some((u, v, _)) =
                    project_uvc(f, crate::project::sample_ray(f, lon_of(i, w), lat))
                else {
                    continue;
                };
                let iw = interior_weight(f, u, v);
                if iw <= 0.0 {
                    continue;
                }
                if iw > w1 {
                    b2 = b1;
                    w2 = w1;
                    b1 = k;
                    w1 = iw;
                } else if iw > w2 {
                    b2 = k;
                    w2 = iw;
                }
            }
            let o = j as usize * row_len + i as usize;
            if b1 == usize::MAX {
                continue; // NO_OWNER
            }
            if b2 == usize::MAX {
                owner[o] = b1 as u16;
                continue;
            }
            // Double couverture : la paire (a<b) a-t-elle une coupe ici ?
            let (a, b) = (b1.min(b2), b1.max(b2));
            if let Some(table) = tables.get(&(a, b)) {
                if let Some(Some(sr)) = table.get(seam_j) {
                    let (x0, x1, s) = (
                        (sr.x0 as f32 * scale).floor() as u32,
                        (sr.x1 as f32 * scale).ceil() as u32,
                        (sr.seam as f32 * scale).round() as u32,
                    );
                    if i >= x0 && i < x1 {
                        owner[o] = if i < s { a as u16 } else { b as u16 };
                        continue;
                    }
                }
            }
            owner[o] = b1 as u16; // fallback : poids intérieur maximal
        }
    }

    OwnerMap {
        width: w,
        height: h,
        owner,
    }
}

/// Couture DP par paire à la résolution `seam_w`.
fn compute_seam_tables(frames: &[PreparedFrame], seam_w: u32) -> SeamTables {
    let mut tables = SeamTables::new();
    let seam_h = (seam_w / 2).max(1);
    for (a, b, _kind) in build_pairs(frames) {
        let (fa, fb) = (&frames[a], &frames[b]);
        // Fenêtre azimutale de la paire : du milieu des axes ± (demi-somme
        // hfov + marge). Colonnes absolues (wrap géré par rem_euclid).
        let mid = 0.5 * (fa.yaw_deg + fb.yaw_deg - wrap_signed(fa.yaw_deg - fb.yaw_deg));
        let hfov_a = 2.0 * (fa.width as f32 * 0.5 / fa.f_px).atan().to_degrees();
        let hfov_b = 2.0 * (fb.width as f32 * 0.5 / fb.f_px).atan().to_degrees();
        let half_span = (hfov_a + hfov_b) * 0.5 + 4.0;
        let dpp = 360.0 / seam_w as f32;
        let span_cols = ((half_span / dpp).ceil() as i64).min(seam_w as i64 / 2);
        let mid_col = (((mid + 180.0).rem_euclid(360.0) / 360.0) * seam_w as f32) as i64;

        // Lignes de recouvrement de bandes, à la résolution des coutures.
        let j0 = (((90.0 - fa.lat_max.min(fb.lat_max)) / 180.0 * seam_h as f32).floor() as i64)
            .clamp(0, seam_h as i64 - 1);
        let j1 = (((90.0 - fa.lat_min.max(fb.lat_min)) / 180.0 * seam_h as f32).ceil() as i64)
            .clamp(0, seam_h as i64 - 1);
        if j1 <= j0 {
            continue;
        }

        // Coût par pixel et DP : `|Δs| ≤ 1` par ligne (two-pass min-plus).
        let cols = (span_cols * 2) as usize;
        let rows = (j1 - j0) as usize;
        let inf = f32::INFINITY;
        let mut dp_prev = vec![inf; cols];
        let mut dp_cur = vec![0.0f32; cols];
        // Choix parent : 0 = x−1, 1 = x, 2 = x+1 (par rapport à la ligne
        // précédente), stocké par ligne pour le backtracking.
        let mut choice = vec![0u8; rows * cols];
        // Intervalle de double couverture par ligne (colonnes absolues).
        let mut runs: Vec<Option<(u32, u32)>> = vec![None; rows];

        for (r, j) in (j0..j1).enumerate() {
            let lat = lat_of(j as u32, seam_h);
            let (mut x0, mut x1) = (u32::MAX, 0u32);
            for c in 0..cols {
                let col_abs = (mid_col - span_cols + c as i64).rem_euclid(seam_w as i64);
                let lon = lon_of(col_abs as u32, seam_w);
                let pa = project_uvc(fa, crate::project::sample_ray(fa, lon, lat))
                    .filter(|&(u, v, _)| interior_weight(fa, u, v) > 0.0);
                let pb = project_uvc(fb, crate::project::sample_ray(fb, lon, lat))
                    .filter(|&(u, v, _)| interior_weight(fb, u, v) > 0.0);
                let cost = match (pa, pb) {
                    (Some((ua, va, _)), Some((ub, vb, _))) => {
                        x0 = x0.min(col_abs as u32);
                        x1 = x1.max(col_abs as u32 + 1);
                        let ca = sample_bilinear(&fa.rgb, fa.width, fa.height, ua, va);
                        let cb = sample_bilinear(&fb.rgb, fb.width, fb.height, ub, vb);
                        let (la, lb) = (fa.local_gain_at(lon, lat), fb.local_gain_at(lon, lat));
                        let mut d = 0.0f32;
                        for k in 0..3 {
                            d += (ca[k] * fa.gain[k] * la - cb[k] * fb.gain[k] * lb).abs();
                        }
                        d
                    }
                    _ => inf,
                };
                if r == 0 {
                    dp_cur[c] = cost;
                } else {
                    let (m0, m1, m2) = (
                        dp_prev[c.checked_sub(1).unwrap_or(cols - 1)],
                        dp_prev[c],
                        dp_prev[(c + 1) % cols],
                    );
                    let (best_idx, best) = if m0 <= m1 && m0 <= m2 {
                        (0, m0)
                    } else if m1 <= m2 {
                        (1, m1)
                    } else {
                        (2, m2)
                    };
                    choice[r * cols + c] = best_idx as u8;
                    dp_cur[c] = if best.is_finite() { best + cost } else { inf };
                }
            }
            if x0 != u32::MAX {
                runs[r] = Some((x0, x1));
            }
            std::mem::swap(&mut dp_prev, &mut dp_cur);
        }

        // Backtrack depuis le minimum de la dernière ligne.
        let mut path = vec![None; rows];
        if let Some(mut c) = dp_prev
            .iter()
            .enumerate()
            .filter(|(_, &v)| v.is_finite())
            .min_by(|(_, va), (_, vb)| va.total_cmp(vb))
            .map(|(c, _)| c)
        {
            for r in (0..rows).rev() {
                path[r] = Some(c);
                if r > 0 {
                    let off = match choice[r * cols + c] {
                        0 => 1,                    // parent venait de x−1
                        2 => cols.wrapping_sub(1), // parent venait de x+1
                        _ => 0,
                    };
                    c = (c + off) % cols;
                }
            }
        }

        // Table par ligne : intervalle de double couverture + colonne de coupe.
        let mut table: Vec<Option<SeamRow>> = vec![None; seam_h as usize];
        for (r, j) in (j0..j1).enumerate() {
            let (Some(sc), Some((x0, x1))) = (path[r], runs[r]) else {
                continue;
            };
            let col_abs = (mid_col - span_cols + sc as i64).rem_euclid(seam_w as i64) as u32;
            table[j as usize] = Some(SeamRow {
                x0,
                x1,
                seam: col_abs,
            });
        }
        tables.insert((a, b), table);
    }
    tables
}

/// wrap en [−180, 180].
fn wrap_signed(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wrap_signed : bornes du cercle.
    #[test]
    fn wrap_signed_bornes() {
        assert!((wrap_signed(190.0) - (-170.0)).abs() < 1e-6);
        assert!((wrap_signed(-170.0) - (-170.0)).abs() < 1e-6);
        assert!((wrap_signed(90.0) - 90.0).abs() < 1e-6);
    }
}
