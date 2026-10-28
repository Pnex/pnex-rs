//! Multi-band blending (Quality pipeline) — exact mechanics of
//! OpenCV's `MultiBandBlender`, in sequential pure Rust.
//!
//! Each frame provides its warped image **masked by the binary ownership
//! map** (weight = mask only: a smooth weight would bias the
//! seam position decided by the DP). We accumulate, per level:
//!
//! - numerator += Laplacian pyramid of `img ⊙ mask`;
//! - denominator += **Gaussian** pyramid of `mask`.
//!
//! then we normalise per level and collapse (upsample + sum from
//! coarse to fine). Fine levels: nearly binary mask → high
//! frequencies switch sharply (no double exposure); coarse levels:
//! wide ramps → exposure/colorimetry melt over a seam
//! width. That is what kills both the averaging ghost and the step
//! of the narrow feather.
//!
//! Denominator ≈ 0 (out of band, holes) → uncovered pixel, left to
//! [`crate::render::fill_band_holes`]/[`crate::render::fill_poles`] **after**
//! the blend (the returned mask drives the fills). Horizontal subsampling
//! wraps (λ = ±180 join), otherwise a vertical seam artefact
//! at column 0.

use crate::project::PreparedFrame;
use crate::render::{lat_of, lon_of, sample_bilinear};
use crate::seam::OwnerMap;

/// Multi-band blend of all frames according to the ownership map.
///
/// Returns the final RGB8 and the coverage mask (`den > 0` at the fine
/// level) — the fills run afterwards, on this mask.
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

    // Sizes per level (ceiling division).
    let sizes: Vec<(u32, u32)> = (0..=n_levels)
        .scan((w, h), |s, _| {
            let cur = *s;
            *s = (s.0.div_ceil(2), s.1.div_ceil(2));
            Some(cur)
        })
        .collect();

    // Accumulators: RGB f32 numerator + weight denominator, per level.
    let mut num: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize * 3])
        .collect();
    let mut den: Vec<Vec<f32>> = sizes
        .iter()
        .map(|&(lw, lh)| vec![0.0f32; (lw * lh) as usize])
        .collect();
    // Flat path accumulators (levels ≤ 1: feather crossfade).
    let mut flat_num = vec![0.0f32; npix * 3];
    let mut flat_den = vec![0.0f32; npix];

    for (k, f) in frames.iter().enumerate() {
        // 1) Binary ownership mask (see module docs).
        let mut img = vec![0.0f32; npix * 3];
        let mut mask = vec![0.0f32; npix];
        let j_top =
            (((90.0 - f.lat_max) / 180.0 * h as f32).floor() as i64).clamp(0, h as i64 - 1) as u32;
        let j_bot =
            (((90.0 - f.lat_min) / 180.0 * h as f32).ceil() as i64).clamp(0, h as i64 - 1) as u32;
        for j in j_top..=j_bot {
            let lat = lat_of(j, h);
            if lat < f.lat_min || lat > f.lat_max {
                continue;
            }
            for i in 0..w {
                if owner.owner[(j * w + i) as usize] == k as u16 {
                    mask[(j * w + i) as usize] = 1.0;
                }
            }
        }
        // 2) NARROW feather along the seam (~4×SEAM_FEATHER_PX of
        // ramp): binary masks produced HARD cuts of
        // objects (chair/screen/radiator crossed by the seam) and
        // blocky exposure steps — wide multi-band diffusion,
        // for its part, recreates ghosts (removed 2026-09-11). The short feather
        // crossfades the two frames over a width where their residual
        // disagreement (~a few px) stays invisible.
        feather_mask(&mut mask, w, h, SEAM_FEATHER_PX);
        // 3) Content wherever the pyramid can reach, not only where the
        // weight is > 0: Laplacian/Gaussian levels spread each pixel over
        // ~2^levels px, so an image that turns black right past its weight
        // leaks that black edge into the seam and the normalisation no
        // longer reconstructs (hard seams at 3 levels, white halos along
        // every seam at 5 — real capture beb93c42, 2026-09-28). Like
        // OpenCV's MultiBandBlender, the IMAGE extends over the footprint
        // and only the WEIGHTS are masked.
        // Projection impossible (ray off the sensor) → weight zeroed.
        let reach = if n_levels <= 1 { 0 } else { 2u32 << n_levels };
        let near = dilate_support(&mask, w, h, reach);
        for j in 0..h {
            let lat = lat_of(j, h);
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

        // Levels ≤ 1: PURE weighted mean (narrow feather crossfade).
        // The Laplacian pyramid AMPLIFIES a narrow ramp (the high-frequency
        // residue of the discontinuity divided by a small weight
        // → light/dark bands along the seams, finding
        // 2026-09-11) — it only makes sense with wide ramps (≥2).
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

        // Pyramid walk: we only keep 2 consecutive levels at a
        // time (the memory peak stays the accumulators, not the pyramids).
        let (mut prev_rgb, mut prev_mask) = (img, mask);
        for l in 0..=n_levels {
            if l == n_levels {
                // G[top] weighted by the Gaussian of the frame's mask.
                for (idx, (d, s)) in num[l].iter_mut().zip(&prev_rgb).enumerate() {
                    *d += s * prev_mask[idx / 3];
                }
                den[l].iter_mut().zip(&prev_mask).for_each(|(d, s)| *d += s);
                break;
            }
            let next_size = sizes[l + 1];
            let next_rgb = reduce_rgb(&prev_rgb, sizes[l], next_size);
            let next_mask = reduce_1(&prev_mask, sizes[l], next_size);
            // Laplacian L[l] = G[l] − expand(G[l+1]) = prev_rgb − lap — the
            // sign was INVERTED (lap − prev_rgb = −L: the collapse
            // SUBTRACTED the detail, wrong reconstruction as soon as there is
            // HF). Weighted by the mask's Gaussian: otherwise each
            // frame pours its FULL content into the overlap
            // (double counting → 100+200 "=" 300 → saturated white halos
            // around lights, finding 2026-09-11; diagnosed by an
            // isolated Python port of the pyramid).
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

    // Flat path: direct normalisation (no collapse).
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
            // LOW threshold: with feathered masks, the outer ring of the
            // ramp has a weight of 0.1-0.5 but REAL content — declaring it
            // uncovered hands it to the fill which spreads it as a white band
            // (finding 2026-09-11). 0.5 only held for binary masks.
            covered[o] = dv > 0.05;
        }
        return (rgb, covered);
    }

    // Per-level normalisation then collapse (from coarse to fine).
    let mut cur = {
        let n = &num[n_levels];
        let d = &den[n_levels];
        let mut out = vec![0.0f32; n.len()];
        for ((o, &dv), dst) in n
            .as_chunks::<3>()
            .0
            .iter()
            .zip(d.iter())
            .zip(out.as_chunks_mut::<3>().0)
        {
            let dn = if dv > 1e-6 { dv } else { 1.0 };
            dst[0] = o[0] / dn;
            dst[1] = o[1] / dn;
            dst[2] = o[2] / dn;
        }
        out
    };
    for l in (0..n_levels).rev() {
        // expand produces a NEW buffer at the size of level `l` (the
        // collapse cannot be done in place: the sizes differ).
        let mut next = expand_rgb(&cur, sizes[l + 1], sizes[l]);
        let n = &num[l];
        let d = &den[l];
        for ((dst, v), &dv) in next
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(n.as_chunks::<3>().0)
            .zip(d.iter())
        {
            let dn = if dv > 1e-6 { dv } else { 1.0 };
            dst[0] += v[0] / dn;
            dst[1] += v[1] / dn;
            dst[2] += v[2] / dn;
        }
        cur = next;
    }

    for (px, src) in rgb
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(cur.as_chunks::<3>().0)
    {
        px[0] = (src[0].round().clamp(0.0, 255.0)) as u8;
        px[1] = (src[1].round().clamp(0.0, 255.0)) as u8;
        px[2] = (src[2].round().clamp(0.0, 255.0)) as u8;
    }
    // Coverage: fine-level denominator (accumulated binary mask).
    for (c, &dv) in covered.iter_mut().zip(&den[0]) {
        *c = dv > 0.5;
    }
    (rgb, covered)
}

// ─────────────────────────── pyramids ───────────────────────────

/// Half-width of the seam feather (px): the ramp is ~2× this value.
/// Narrow by choice — a wide feather recreates the ghosting diffusion
/// (see LEVELS≥2 defect, removed 2026-09-11).
pub const SEAM_FEATHER_PX: u32 = 6;

/// Spreads a binary mask into a soft ramp: `passes` erosion-dilations
/// approximated by a 3×3 box on the front (the ramp ends in a soft slope
/// 0→1 over ~2×radius). Horizontal wrapping (longitude wraps),
/// vertical clamped.
fn feather_mask(mask: &mut [f32], w: u32, h: u32, radius: u32) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    // 2 box passes (circular H + clamped V): the 0→1 ramp spans
    // ~2×(2×radius) px. `cur` = input of each pass, result in cur.
    let r = radius as i64;
    let (uw, uh) = (w as usize, h as usize);
    let mut cur = mask.to_vec();
    let mut next = vec![0.0f32; mask.len()];
    for _ in 0..2 {
        // horizontal wrap: cur → next
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
        // vertical clamp: next → cur
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

/// Burt kernel [1,4,6,4,1]/16.
const H: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// ×2 subsampling (1 plane): circular horizontal (longitude
/// wraps), clamped vertical.
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

/// Fine position → (coarse index, phase 0|1): x = 2·c + f.
fn div_mod2(x: i64) -> (i64, i64) {
    (x.div_euclid(2), x.rem_euclid(2))
}

fn expand_rgb(src: &[f32], size: (u32, u32), dst_size: (u32, u32)) -> Vec<f32> {
    // 3 interleaved channels: we interpolate each channel through the
    // shared indexing (the same weights apply).
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

    /// Unit gain of the pyramid operations: reduce and expand
    /// reproduce a constant image (condition lap = g − expand∘reduce).
    #[test]
    fn unit_gain_pyramid() {
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

    /// Two CONSTANT frames in full overlap: the output must be
    /// the crossfade 100 → 150 → 200 — NEVER ~300 (double counting of
    /// full content in the overlap, bug of the numerator not weighted
    /// by the mask, finding 2026-09-11).
    #[test]
    fn constant_overlap_without_double_counting() {
        let (w, h) = (64u32, 32u32);
        let frames: Vec<(f32, Vec<f32>, Vec<f32>)> = (0..2)
            .map(|k| {
                let v = if k == 0 { 100.0 } else { 200.0 };
                let img = vec![v; (w * h * 3) as usize];
                // Complementary binary masks per half, feathered by
                // the pyramid: full overlap of the two frames.
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
        // Accumulation (identical to blend) then collapse.
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
            for ((o, &dv), dst) in n
                .as_chunks::<3>()
                .0
                .iter()
                .zip(d.iter())
                .zip(out.as_chunks_mut::<3>().0)
            {
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
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(n.as_chunks::<3>().0)
                .zip(d.iter())
            {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] += v[0] / dn;
                dst[1] += v[1] / dn;
                dst[2] += v[2] / dn;
            }
            cur = next;
        }
        // Column 0 (full mask frame 0) = 100; column w−1 = 200;
        // the centre = between the two. NEVER ≥ 250 (double counting).
        let at = |i: usize| cur[(16 * w as usize + i) * 3];
        println!(
            "profile: {}",
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
        assert!(cur.iter().all(|&v| v < 250.0), "double counting detected");
    }

    /// div_mod2: even/odd overlap.
    #[test]
    fn div_mod2_phases() {
        assert_eq!(div_mod2(0), (0, 0));
        assert_eq!(div_mod2(1), (0, 1));
        assert_eq!(div_mod2(2), (1, 0));
        assert_eq!(div_mod2(5), (2, 1));
    }

    /// Reconstruction identity: lap_l = g_l − expand(g_{l+1}), accumulated
    /// as in blend (num) then normalised by the mask's Gaussian pyramid
    /// (den) and collapsed — must restore the image at the centre of a
    /// rectangular mask (≥ 2^levels from the edge: beyond that, the coarse level's
    /// ratio legitimately pulls towards the visible mean).
    #[test]
    fn identity_reconstruction() {
        let (w, h) = (64u32, 32u32);
        let mut img = vec![0.0f32; (w * h * 3) as usize];
        let mut mask = vec![0.0f32; (w * h) as usize];
        for j in 0..h as usize {
            for i in 0..w as usize {
                let o = (j * w as usize + i) * 3;
                // Fine octave added: a smooth sine alone passed under the
                // tolerance and did not detect the inverted sign.
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
            for ((o, &dv), dst) in n
                .as_chunks::<3>()
                .0
                .iter()
                .zip(d.iter())
                .zip(out.as_chunks_mut::<3>().0)
            {
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
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(n.as_chunks::<3>().0)
                .zip(d.iter())
            {
                let dn = if dv > 1e-6 { dv } else { 1.0 };
                dst[0] += v[0] / dn;
                dst[1] += v[1] / dn;
                dst[2] += v[2] / dn;
            }
            cur = next;
        }

        // Safe interior (≥ 2^levels from the mask edge).
        let o = (8 * w as usize + 32) * 3;
        let want = 60.0 + 30.0 * (32.0f32 * 0.2).sin() + 10.0 * (32.0f32 * 1.7).sin();
        let got = cur[o];
        assert!(
            (got - want).abs() <= 3.0,
            "reconstruction (col 32): {got:.1} ≠ {want:.1}"
        );
    }
}
