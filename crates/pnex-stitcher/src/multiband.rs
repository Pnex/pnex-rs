//! Blending multi-bandes (pipeline Quality) — mécanique exacte du
//! `MultiBandBlender` d'OpenCV, en Rust pur séquentiel.
//!
//! Chaque frame fournit son image warpée **masquée par la carte de
//! propriété binaire** (poids = masque seul : un poids lisse biaiserait la
//! position de couture décidée par le DP). On accumule, par niveau :
//!
//! - numérateur += pyramide de Laplace de `img ⊙ mask` ;
//! - dénominateur += pyramide **gaussienne** de `mask`.
//!
//! puis on normalise par niveau et on collapse (upsample + somme du
//! grossier au fin). Niveaux fins : masque quasi binaire → hautes
//! fréquences commutent net (pas de double exposition) ; niveaux grossiers :
//! rampes larges → exposition/colorimétrie fondent sur une largeur de
//! couture. C'est ce qui tue à la fois le fantôme du moyennage et la marche
//! du fondu étroit.
//!
//! Dénominateur ≈ 0 (hors bande, trous) → pixel non couvert, rendu au
//! [`crate::render::fill_band_holes`]/[`crate::render::fill_poles`] **après**
//! le blend (le masque retourné pilote les fills). Le sous-échantillonnage
//! horizontal boucle (raccord λ = ±180), sinon artifact de couture verticale
//! à la colonne 0.

use crate::project::PreparedFrame;
use crate::render::{lat_of, lon_of, sample_bilinear};
use crate::seam::OwnerMap;

/// Blend multi-bandes de toutes les frames selon la carte de propriété.
///
/// Retourne le RGB8 final et le masque de couverture (`den > 0` au niveau
/// fin) — les fills passent après, sur ce masque.
#[must_use]
pub fn blend(frames: &[PreparedFrame], owner: &OwnerMap, levels: u32) -> (Vec<u8>, Vec<bool>) {
    let w = owner.width;
    let h = owner.height;
    let npix = (w * h) as usize;
    let mut rgb = vec![0u8; npix * 3];
    let mut covered = vec![false; npix];
    if frames.is_empty() || w == 0 || h == 0 {
        return (rgb, covered);
    }
    let n_levels = levels.clamp(1, 8) as usize;

    // Tailles par niveau (division plafonnée).
    let sizes: Vec<(u32, u32)> = (0..=n_levels)
        .scan((w, h), |s, _| {
            let cur = *s;
            *s = (s.0.div_ceil(2), s.1.div_ceil(2));
            Some(cur)
        })
        .collect();

    // Accumulateurs : numérateur RGB f32 + dénominateur poids, par niveau.
    let mut num: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize * 3])
        .collect();
    let mut den: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize])
        .collect();
    // Accumulateurs du chemin plat (levels ≤ 1 : crossfade du fondu).
    let mut flat_num = vec![0.0f32; npix * 3];
    let mut flat_den = vec![0.0f32; npix];

    for (k, f) in frames.iter().enumerate() {
        // 1) Masque binaire de propriété (cf. docs module).
        let mut img = vec![0.0f32; npix * 3];
        let mut mask = vec![0.0f32; npix];
        let j_top =
            (((90.0 - f.lat_max) / 180.0 * h as f32).floor() as i64).clamp(0, h as i64 - 1) as u32;
        let j_bot =
            (((90.0 - f.lat_min) / 180.0 * h as f32).ceil() as i64).clamp(0, h as i64 - 1) as u32;
        for j in j_top..=j_bot {
            let lat = lat_of(j, w, h);
            if lat < f.lat_min || lat > f.lat_max {
                continue;
            }
            for i in 0..w {
                if owner.owner[(j * w + i) as usize] == k as u16 {
                    mask[(j * w + i) as usize] = 1.0;
                }
            }
        }
        // 2) Fondu ÉTROIT le long de la couture (~4×SEAM_FEATHER_PX de
        // rampe) : les masques binaires produisaient des coupures DURES des
        // objets (chaise/écran/radiateur traversés par la couture) et des
        // marches d'exposition en blocs — la diffusion multibande large,
        // elle, recrée les fantômes (retirée 2026-09-11). Le fondu court
        // crossfade les deux frames sur une largeur où leur désaccord
        // résiduel (~quelques px) reste invisible.
        feather_mask(&mut mask, w, h, SEAM_FEATHER_PX);
        // 3) Content wherever the pyramid can reach, not only where the
        // weight is > 0: Laplacian/Gaussian levels spread each pixel over
        // ~2^levels px, so an image that turns black right past its weight
        // leaks that black edge into the seam and the normalisation no
        // longer reconstructs (hard seams at 3 levels, white halos along
        // every seam at 5 — real capture beb93c42, 2026-09-28). Like
        // OpenCV's MultiBandBlender, the IMAGE extends over the footprint
        // and only the WEIGHTS are masked.
        // Projection impossible (rayon hors capteur) → poids annulé.
        let reach = if n_levels <= 1 { 0 } else { 2u32 << n_levels };
        let near = dilate_support(&mask, w, h, reach);
        for j in 0..h {
            let lat = lat_of(j, w, h);
            for i in 0..w {
                let o = (j * w + i) as usize;
                if !near[o] {
                    continue;
                }
                let lon = lon_of(i, w);
                let Some((u, v, _)) =
                    crate::project::project_uvc(f, crate::project::sample_ray(f, lon, lat))
                else {
                    mask[o] = 0.0;
                    continue;
                };
                let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                let lg = f.local_gain_at(lon, lat);
                img[o * 3] = (col[0] * f.gain[0] * lg).min(255.0);
                img[o * 3 + 1] = (col[1] * f.gain[1] * lg).min(255.0);
                img[o * 3 + 2] = (col[2] * f.gain[2] * lg).min(255.0);
            }
        }

        // Niveaux ≤ 1 : moyenne pondérée PURE (crossfade du fondu étroit).
        // La pyramide Laplacienne AMPLIFIE une rampe étroite (le résidu
        // haute fréquence de la discontinuité divisé par un poids faible
        // → bandes claires/sombres le long des coutures, constat
        // 2026-09-11) — elle n'a de sens qu'avec des rampes larges (≥2).
        if n_levels <= 1 {
            for (o, &m) in mask.iter().enumerate() {
                if m <= 0.0 {
                    continue;
                }
                flat_den[o] += m;
                let im = &img[o * 3..o * 3 + 3];
                flat_num[o * 3] += im[0] * m;
                flat_num[o * 3 + 1] += im[1] * m;
                flat_num[o * 3 + 2] += im[2] * m;
            }
            continue;
        }

        // Marche pyramidale : on ne garde que 2 niveaux consécutifs à la
        // fois (le pic mémoire reste les accumulateurs, pas les pyramides).
        let (mut prev_rgb, mut prev_mask) = (img, mask);
        for l in 0..=n_levels {
            if l == n_levels {
                // G[top] pondéré par la Gaussienne du masque de la frame.
                for (idx, (d, s)) in num[l].iter_mut().zip(&prev_rgb).enumerate() {
                    *d += s * prev_mask[idx / 3];
                }
                den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
                break;
            }
            let next_size = sizes[l + 1];
            let next_rgb = reduce_rgb(&prev_rgb, sizes[l], next_size);
            let next_mask = reduce_1(&prev_mask, sizes[l], next_size);
            // Laplacien L[l] = G[l] − expand(G[l+1]) = prev_rgb − lap — le
            // signe était INVERSÉ (lap − prev_rgb = −L : le collapse
            // SOUSTRAIT le détail, reconstruction fausse dès qu'il y a de
            // la HF). Pondéré par la Gaussienne du masque : sinon chaque
            // frame déverse son contenu PLEIN dans le recouvrement
            // (double-comptage → 100+200 « = » 300 → halos blancs saturés
            // autour des lumières, constat 2026-09-11 ; diagnostic par
            // portage Python isolé de la pyramide).
            let lap = expand_rgb(&next_rgb, next_size, sizes[l]);
            for (idx, (dst, (a, b))) in num[l].iter_mut().zip(lap.iter().zip(&prev_rgb)).enumerate()
            {
                *dst += (b - a) * prev_mask[idx / 3];
            }
            den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
            prev_rgb = next_rgb;
            prev_mask = next_mask;
        }
    }

    // Chemin plat : normalisation directe (pas de collapse).
    if n_levels <= 1 {
        for (o, &dv) in flat_den.iter().enumerate() {
            if dv > 1e-6 {
                let (r, g, b) = (
                    flat_num[o * 3] / dv,
                    flat_num[o * 3 + 1] / dv,
                    flat_num[o * 3 + 2] / dv,
                );
                rgb[o * 3] = r.round().clamp(0.0, 255.0) as u8;
                rgb[o * 3 + 1] = g.round().clamp(0.0, 255.0) as u8;
                rgb[o * 3 + 2] = b.round().clamp(0.0, 255.0) as u8;
            }
            // Seuil BAS : avec les masques fondus, l'anneau extérieur de la
            // rampe a un poids 0,1-0,5 mais un contenu RÉEL — le déclarer
            // non couvert le livre au fill qui l'étale en bande blanche
            // (constat 2026-09-11). 0.5 ne valait que pour du binaire.
            covered[o] = dv > 0.05;
        }
        return (rgb, covered);
    }

    // Normalisation par niveau puis collapse (du grossier vers le fin).
    let mut cur = {
        let n = &num[n_levels];
        let d = &den[n_levels];
        let mut out = vec![0.0f32; n.len()];
        for ((o, &dv), dst) in n.chunks_exact(3).zip(d.iter()).zip(out.chunks_exact_mut(3)) {
            let dn = if dv > 1e-6 { dv } else { 1.0 };
            dst[0] = o[0] / dn;
            dst[1] = o[1] / dn;
            dst[2] = o[2] / dn;
        }
        out
    };
    for l in (0..n_levels).rev() {
        // expand produit un NOUVEAU buffer à la taille du niveau `l` (le
        // collapse ne peut pas se faire en place : les tailles diffèrent).
        let mut next = expand_rgb(&cur, sizes[l + 1], sizes[l]);
        let n = &num[l];
        let d = &den[l];
        for ((dst, v), &dv) in next
            .chunks_exact_mut(3)
            .zip(n.chunks_exact(3))
            .zip(d.iter())
        {
            let dn = if dv > 1e-6 { dv } else { 1.0 };
            dst[0] += v[0] / dn;
            dst[1] += v[1] / dn;
            dst[2] += v[2] / dn;
        }
        cur = next;
    }

    for (px, src) in rgb.chunks_exact_mut(3).zip(cur.chunks_exact(3)) {
        px[0] = (src[0].round().clamp(0.0, 255.0)) as u8;
        px[1] = (src[1].round().clamp(0.0, 255.0)) as u8;
        px[2] = (src[2].round().clamp(0.0, 255.0)) as u8;
    }
    // Couverture : dénominateur du niveau fin (masque binaire accumulé).
    for (c, &dv) in covered.iter_mut().zip(&den[0]) {
        *c = dv > 0.5;
    }
    (rgb, covered)
}

// ─────────────────────────── pyramides ───────────────────────────

/// Demi-largeur du fondu de couture (px) : la rampe fait ~2× cette valeur.
/// Étroit par choix — un fondu large recrée la diffusion en fantômes
/// (cf. défaut LEVELS≥2, retiré 2026-09-11).
pub const SEAM_FEATHER_PX: u32 = 6;

/// Étale un masque binaire en rampe douce : `passes` érosions-dilations
/// approximées par boîte 3×3 sur le front (la rampe finit en pente douce
/// 0→1 sur ~2×radius). Horizontal bouclant (la longitude boucle),
/// vertical clampé.
fn feather_mask(mask: &mut [f32], w: u32, h: u32, radius: u32) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    // 2 passes de boîte (H circulaire + V clampée) : la rampe 0→1 fait
    // ~2×(2×radius) px. `cur` = entrée de chaque passe, résultat dans cur.
    let r = radius as i64;
    let (uw, uh) = (w as usize, h as usize);
    let mut cur = mask.to_vec();
    let mut next = vec![0.0f32; mask.len()];
    for _ in 0..2 {
        // horizontal wrap : cur → next
        for j in 0..uh {
            let base = j * uw;
            for i in 0..uw {
                let mut acc = 0.0f32;
                for d in -r..=r {
                    let x = (i as i64 + d).rem_euclid(uw as i64) as usize;
                    acc += cur[base + x];
                }
                next[base + i] = acc / (2 * r + 1) as f32;
            }
        }
        // vertical clamp : next → cur
        for j in 0..uh {
            let base = j * uw;
            for i in 0..uw {
                let mut acc = 0.0f32;
                for d in -r..=r {
                    let y = (j as i64 + d).clamp(0, uh as i64 - 1) as usize;
                    acc += next[y * uw + i];
                }
                cur[base + i] = acc / (2 * r + 1) as f32;
            }
        }
    }
    mask.copy_from_slice(&cur);
}

/// Pixels within `radius` px (box, longitude wrapping, latitude clamped) of
/// a positive weight — where a frame's content must be rendered for the
/// pyramid. Separable running counts: O(pixels) whatever the radius.
fn dilate_support(mask: &[f32], w: u32, h: u32, radius: u32) -> Vec<bool> {
    let (uw, uh) = (w as usize, h as usize);
    let src: Vec<bool> = mask.iter().map(|&m| m > 0.0).collect();
    if radius == 0 || uw == 0 || uh == 0 {
        return src;
    }
    let r = (radius as usize).min(uw / 2);
    // Horizontal pass (wrapping): count of set pixels in [i - r, i + r].
    let mut horiz = vec![false; src.len()];
    for j in 0..uh {
        let row = &src[j * uw..(j + 1) * uw];
        let mut count: usize = (0..=2 * r).map(|d| row[(d + uw - r) % uw] as usize).sum();
        for i in 0..uw {
            horiz[j * uw + i] = count > 0;
            count -= row[(i + uw - r) % uw] as usize;
            count += row[(i + r + 1) % uw] as usize;
        }
    }
    // Vertical pass (clamped): prefix sums per column.
    let rv = radius as usize;
    let mut out = vec![false; src.len()];
    let mut prefix = vec![0usize; uh + 1];
    for i in 0..uw {
        for j in 0..uh {
            prefix[j + 1] = prefix[j] + horiz[j * uw + i] as usize;
        }
        for j in 0..uh {
            let lo = j.saturating_sub(rv);
            let hi = (j + rv + 1).min(uh);
            out[j * uw + i] = prefix[hi] > prefix[lo];
        }
    }
    out
}

/// Noyau de Burt [1,4,6,4,1]/16.
const H: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Sous-échantillonnage ×2 (1 plan) : horizontal circulaire (la longitude
/// boucle), vertical clampé.
fn reduce_1(src: &[f32], size: (u32, u32), dst_size: (u32, u32)) -> Vec<f32> {
    let (w, h) = size;
    let (dw, dh) = dst_size;
    let mut dst = vec![0.0f32; (dw * dh) as usize];
    for j in 0..dh {
        for i in 0..dw {
            let mut acc = 0.0f32;
            for (m, &hm) in H.iter().enumerate() {
                let sy = (2 * j as i64 + m as i64 - 2).clamp(0, h as i64 - 1) as u32;
                for (n, &hn) in H.iter().enumerate() {
                    let sx = (2 * i as i64 + n as i64 - 2).rem_euclid(w as i64) as u32;
                    acc += hm * hn * src[(sy * w + sx) as usize];
                }
            }
            dst[(j * dw + i) as usize] = acc;
        }
    }
    dst
}

fn reduce_rgb(src: &[f32], size: (u32, u32), dst_size: (u32, u32)) -> Vec<f32> {
    let (w, h) = size;
    let (dw, dh) = dst_size;
    let mut dst = vec![0.0f32; (dw * dh) as usize * 3];
    for j in 0..dh {
        for i in 0..dw {
            let mut acc = [0.0f32; 3];
            for (m, &hm) in H.iter().enumerate() {
                let sy = (2 * j as i64 + m as i64 - 2).clamp(0, h as i64 - 1) as u32;
                for (n, &hn) in H.iter().enumerate() {
                    let sx = (2 * i as i64 + n as i64 - 2).rem_euclid(w as i64) as u32;
                    let o = ((sy * w + sx) * 3) as usize;
                    for k in 0..3 {
                        acc[k] += hm * hn * src[o + k];
                    }
                }
            }
            let d = ((j * dw + i) * 3) as usize;
            dst[d] = acc[0];
            dst[d + 1] = acc[1];
            dst[d + 2] = acc[2];
        }
    }
    dst
}

/// Position fine → (indice grossier, phase 0|1) : x = 2·c + f.
fn div_mod2(x: i64) -> (i64, i64) {
    (x.div_euclid(2), x.rem_euclid(2))
}

fn expand_rgb(src: &[f32], size: (u32, u32), dst_size: (u32, u32)) -> Vec<f32> {
    // 3 canaux entrelacés : on interpole chaque canal par l'indexation
    // commune (les mêmes poids s'appliquent).
    let (w, h) = size;
    let (dw, dh) = dst_size;
    let mut dst = vec![0.0f32; (dw * dh) as usize * 3];
    let at = |x: i64, y: i64, k: usize| -> f32 {
        let yc = y.clamp(0, h as i64 - 1) as u32;
        let xc = x.rem_euclid(w as i64) as u32;
        src[((yc * w + xc) * 3) as usize + k]
    };
    for j in 0..dh as i64 {
        let (cj, fj) = div_mod2(j);
        for i in 0..dw as i64 {
            let (ci, fi) = div_mod2(i);
            let d = ((j as u32 * dw + i as u32) * 3) as usize;
            for k in 0..3 {
                let v = if fi == 0 {
                    if fj == 0 {
                        (at(ci - 1, cj - 1, k)
                            + 6.0 * at(ci, cj - 1, k)
                            + at(ci + 1, cj - 1, k)
                            + at(ci - 1, cj, k)
                            + 6.0 * at(ci, cj, k)
                            + at(ci + 1, cj, k))
                            / 16.0
                    } else {
                        (at(ci - 1, cj - 1, k)
                            + 2.0 * at(ci, cj - 1, k)
                            + at(ci + 1, cj - 1, k)
                            + at(ci - 1, cj, k)
                            + 2.0 * at(ci, cj, k)
                            + at(ci + 1, cj, k))
                            / 8.0
                    }
                } else if fj == 0 {
                    (at(ci, cj - 1, k) + at(ci + 1, cj - 1, k) + at(ci, cj, k) + at(ci + 1, cj, k))
                        / 4.0
                } else {
                    (at(ci, cj, k) + at(ci + 1, cj, k)) / 2.0
                };
                dst[d + k] = v;
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Support dilation: box of `radius` around set pixels, longitude
    /// wrapping, latitude clamped.
    #[test]
    fn dilate_support_wraps_longitude_and_clamps_latitude() {
        let (w, h) = (20u32, 10u32);
        let mut mask = vec![0.0f32; (w * h) as usize];
        mask[(5 * w + 1) as usize] = 1.0; // (i=1, j=5)
        let near = dilate_support(&mask, w, h, 2);
        let at = |i: u32, j: u32| near[(j * w + i) as usize];
        assert!(at(1, 5) && at(3, 5) && at(0, 5) && at(19, 5)); // wraps west
        assert!(!at(4, 5) && !at(18, 5));
        assert!(at(1, 3) && at(1, 7) && !at(1, 2) && !at(1, 8));
        assert!(at(19, 7)); // box corner, wrapped
                            // Radius 0: the support itself.
        assert_eq!(
            dilate_support(&mask, w, h, 0)
                .iter()
                .filter(|&&b| b)
                .count(),
            1
        );
    }

    /// Gain unitaire des opérations pyramidales : reduce et expand
    /// reproduisent une image constante (condition lap = g − expand∘reduce).
    #[test]
    fn gain_unitaire_pyramide() {
        let size = (16u32, 8u32);
        let src: Vec<f32> = vec![42.0; (size.0 * size.1) as usize];
        let small = reduce_1(&src, size, (size.0 / 2, size.1 / 2));
        assert!(small.iter().all(|&v| (v - 42.0).abs() < 1e-4));

        let src3: Vec<f32> = src.iter().flat_map(|&v| [v, v, v]).collect();
        let small3 = reduce_rgb(&src3, size, (size.0 / 2, size.1 / 2));
        assert!(small3.iter().all(|&v| (v - 42.0).abs() < 1e-4));
        let back3 = expand_rgb(&small3, (size.0 / 2, size.1 / 2), size);
        assert!(back3.iter().all(|&v| (v - 42.0).abs() < 1e-4));
    }

    /// Deux frames CONSTANTES en recouvrement total : la sortie doit être
    /// le crossfade 100 → 150 → 200 — JAMAIS ~300 (double-comptage du
    /// contenu plein dans le recouvrement, bug du numérateur non pondéré
    /// par le masque, constat 2026-09-11).
    #[test]
    fn recouvrement_constant_sans_double_comptage() {
        let (w, h) = (64u32, 32u32);
        let frames: Vec<(f32, Vec<f32>, Vec<f32>)> = (0..2)
            .map(|k| {
                let v = if k == 0 { 100.0 } else { 200.0 };
                let img = vec![v; (w * h * 3) as usize];
                // Masques binaires complémentaires par moitié, fondus par
                // la pyramide : recouvrement total des deux frames.
                let mut mask = vec![0.0f32; (w * h) as usize];
                let lim = if k == 0 {
                    w as usize / 2 + 8
                } else {
                    w as usize / 2 - 8
                };
                for j in 0..h as usize {
                    for i in 0..w as usize {
                        if (k == 0 && i < lim) || (k == 1 && i >= lim) {
                            mask[j * w as usize + i] = 1.0;
                        }
                    }
                }
                (v, img, mask)
            })
            .collect();
        // Accumulation (identique à blend) puis collapse.
        let n_levels = 2usize;
        let mut sizes = vec![(w, h)];
        for _ in 0..n_levels {
            let (lw, lh) = sizes.last().unwrap();
            sizes.push((lw.div_ceil(2), lh.div_ceil(2)));
        }
        let mut num: Vec<Vec<f32>> = sizes
            .iter()
            .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize * 3])
            .collect();
        let mut den: Vec<Vec<f32>> = sizes
            .iter()
            .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize])
            .collect();
        for (_, img, mask) in &frames {
            let (mut prev_rgb, mut prev_mask) = (img.clone(), mask.clone());
            for l in 0..=n_levels {
                if l == n_levels {
                    for (idx, (d, s)) in num[l].iter_mut().zip(&prev_rgb).enumerate() {
                        *d += s * prev_mask[idx / 3];
                    }
                    den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
                    break;
                }
                let next_size = sizes[l + 1];
                let next_rgb = reduce_rgb(&prev_rgb, sizes[l], next_size);
                let next_mask = reduce_1(&prev_mask, sizes[l], next_size);
                let lap = expand_rgb(&next_rgb, next_size, sizes[l]);
                for (idx, (dst, (a, b))) in
                    num[l].iter_mut().zip(lap.iter().zip(&prev_rgb)).enumerate()
                {
                    *dst += (b - a) * prev_mask[idx / 3];
                }
                den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
                prev_rgb = next_rgb;
                prev_mask = next_mask;
            }
        }
        let mut cur = {
            let n = &num[n_levels];
            let d = &den[n_levels];
            let mut out = vec![0.0f32; n.len()];
            for ((o, &dv), dst) in n.chunks_exact(3).zip(d.iter()).zip(out.chunks_exact_mut(3)) {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] = o[0] / dn;
                dst[1] = o[1] / dn;
                dst[2] = o[2] / dn;
            }
            out
        };
        for l in (0..n_levels).rev() {
            let mut next = expand_rgb(&cur, sizes[l + 1], sizes[l]);
            let n = &num[l];
            let d = &den[l];
            for ((dst, v), &dv) in next
                .chunks_exact_mut(3)
                .zip(n.chunks_exact(3))
                .zip(d.iter())
            {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] += v[0] / dn;
                dst[1] += v[1] / dn;
                dst[2] += v[2] / dn;
            }
            cur = next;
        }
        // Colonne 0 (plein masque frame 0) = 100 ; colonne w−1 = 200 ;
        // le centre = entre les deux. JAMAIS ≥ 250 (double-comptage).
        let at = |i: usize| cur[(16 * w as usize + i) * 3];
        println!(
            "profil: {}",
            (0..w as usize)
                .step_by(4)
                .map(|i| format!("{:.0}", at(i)))
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert!(at(4) < 120.0, "col 4 = {}", at(4));
        assert!(
            at(w as usize - 4) > 180.0,
            "col w-4 = {}",
            at(w as usize - 4)
        );
        assert!(cur.iter().all(|&v| v < 250.0), "double-comptage détecté");
    }

    /// div_mod2 : recouvrement pair/impair.
    #[test]
    fn div_mod2_phases() {
        assert_eq!(div_mod2(0), (0, 0));
        assert_eq!(div_mod2(1), (0, 1));
        assert_eq!(div_mod2(2), (1, 0));
        assert_eq!(div_mod2(5), (2, 1));
    }

    /// Identité de reconstruction : lap_l = g_l − expand(g_{l+1}), accumulé
    /// comme dans blend (num) puis normalisé par la pyramide gaussienne du
    /// masque (den) et collapse — doit restituer l'image au centre d'un
    /// masque rectangle (≥ 2^levels du bord : au-delà, le ratio du niveau
    /// grossier tire légitimement vers la moyenne visible).
    #[test]
    fn reconstruction_identite() {
        let (w, h) = (64u32, 32u32);
        let mut img = vec![0.0f32; (w * h * 3) as usize];
        let mut mask = vec![0.0f32; (w * h) as usize];
        for j in 0..h as usize {
            for i in 0..w as usize {
                let o = (j * w as usize + i) * 3;
                // Octave fine ajoutée : un sinus doux seul passait sous la
                // tolérance et ne détectait pas le signe inversé.
                let v = 60.0 + 30.0 * ((i as f32) * 0.2).sin() + 10.0 * ((i as f32) * 1.7).sin();
                img[o] = v;
                img[o + 1] = v;
                img[o + 2] = v;
                mask[j * w as usize + i] = if (8..56).contains(&i) { 1.0 } else { 0.0 };
            }
        }
        let n_levels = 2usize;

        let mut sizes = vec![(w, h)];
        for _ in 0..n_levels {
            let (lw, lh) = sizes.last().unwrap();
            sizes.push((lw.div_ceil(2), lh.div_ceil(2)));
        }
        let mut num: Vec<Vec<f32>> = sizes
            .iter()
            .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize * 3])
            .collect();
        let mut den: Vec<Vec<f32>> = sizes
            .iter()
            .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize])
            .collect();

        let (mut prev_rgb, mut prev_mask) = (img.clone(), mask.clone());
        for l in 0..=n_levels {
            if l == n_levels {
                for (idx, (d, s)) in num[l].iter_mut().zip(&prev_rgb).enumerate() {
                    *d += s * prev_mask[idx / 3];
                }
                den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
                break;
            }
            let next_size = sizes[l + 1];
            let next_rgb = reduce_rgb(&prev_rgb, sizes[l], next_size);
            let next_mask = reduce_1(&prev_mask, sizes[l], next_size);
            let lap = expand_rgb(&next_rgb, next_size, sizes[l]);
            for (idx, (dst, (a, b))) in num[l].iter_mut().zip(lap.iter().zip(&prev_rgb)).enumerate()
            {
                *dst += (b - a) * prev_mask[idx / 3];
            }
            den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
            prev_rgb = next_rgb;
            prev_mask = next_mask;
        }

        // Collapse.
        let mut cur = {
            let n = &num[n_levels];
            let d = &den[n_levels];
            let mut out = vec![0.0f32; n.len()];
            for ((o, &dv), dst) in n.chunks_exact(3).zip(d.iter()).zip(out.chunks_exact_mut(3)) {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] = o[0] / dn;
                dst[1] = o[1] / dn;
                dst[2] = o[2] / dn;
            }
            out
        };
        for l in (0..n_levels).rev() {
            let mut next = expand_rgb(&cur, sizes[l + 1], sizes[l]);
            let n = &num[l];
            let d = &den[l];
            for ((dst, v), &dv) in next
                .chunks_exact_mut(3)
                .zip(n.chunks_exact(3))
                .zip(d.iter())
            {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] += v[0] / dn;
                dst[1] += v[1] / dn;
                dst[2] += v[2] / dn;
            }
            cur = next;
        }

        // Intérieur sûr (≥ 2^levels du bord du masque).
        let o = (8 * w as usize + 32) * 3;
        let want = 60.0 + 30.0 * (32.0f32 * 0.2).sin() + 10.0 * (32.0f32 * 1.7).sin();
        let got = cur[o];
        assert!(
            (got - want).abs() <= 3.0,
            "reconstruction (col 32) : {got:.1} ≠ {want:.1}"
        );
    }
}
