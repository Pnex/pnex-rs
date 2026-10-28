//! Content-routed seams (Quality pipeline).
//!
//! The "selection" blend of the Legacy pipeline implicitly cuts at the
//! bisector of the centralities: a fixed seam, blind to content, that
//! slices objects in parallax. Here we **route** the cut: per pair
//! with enough overlap, a per-row dynamic programming search finds
//! the minimal-cost path — pixel cost = colour difference between the
//! two (gained) frames — with implicit smoothing `|Δs| ≤ 1` per row
//! (graph-cut/Dijkstra family of OpenCV and stitchEm, as sequential DP).
//!
//! Output: [`OwnerMap`], the "each pixel belongs to ONE frame" map
//! consumed by [`crate::multiband::blend`] (binary mask per frame).
//! Out-of-band pixels / holes → [`NO_OWNER`].

use std::collections::HashMap;

use crate::align::build_pairs;
use crate::project::{interior_weight, project_uvc, PreparedFrame};
use crate::render::{lat_of, lon_of, sample_bilinear};
use crate::QualityParams;

/// "Nobody covers this pixel" value in [`OwnerMap::owner`].
pub const NO_OWNER: u16 = u16::MAX;

/// Ownership map: for each pixel of the output equirect, the frame
/// that provides it (input index) or [`NO_OWNER`].
#[derive(Debug, Clone)]
pub struct OwnerMap {
    pub width: u32,
    pub height: u32,
    /// `width·height` frame indices, row-major.
    pub owner: Vec<u16>,
}

impl OwnerMap {
    /// Number of covered pixels per row (band coverage metric).
    #[must_use]
    pub fn covered_per_row(&self) -> Vec<u32> {
        self.owner
            .chunks(self.width as usize)
            .map(|row| row.iter().filter(|&&o| o != NO_OWNER).count() as u32)
            .collect()
    }
}

/// Cut found for one row of a pair: double-coverage interval
/// `[x0, x1)` (columns seen by both frames) and cut column
/// `s` (to `a` for `col < s`, to `b` otherwise). Absolute columns at the
/// seams' working resolution.
#[derive(Debug, Clone, Copy)]
struct SeamRow {
    x0: u32,
    x1: u32,
    seam: u32,
}

/// Seam tables per pair (key `(a, b)` with a < b).
type SeamTables = HashMap<(usize, usize), Vec<Option<SeamRow>>>;

/// Computes the per-pair seams then resolves the ownership map.
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

    // Single pass over the equirect: single coverage → obvious owner;
    // double coverage → cut by the pair's seam table (the two
    // frames with the highest interior weight); fallback = max weight (Legacy
    // behaviour) outside the tabulated double-coverage regions.
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
            // Project into the eligible frames, keep the 2 best weights.
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
            // Double coverage: does the pair (a<b) have a cut here?
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
            owner[o] = b1 as u16; // fallback: maximal interior weight
        }
    }

    OwnerMap {
        width: w,
        height: h,
        owner,
    }
}

/// Per-pair DP seam at resolution `seam_w`.
fn compute_seam_tables(frames: &[PreparedFrame], seam_w: u32) -> SeamTables {
    let mut tables = SeamTables::new();
    let seam_h = (seam_w / 2).max(1);
    for (a, b, _kind) in build_pairs(frames) {
        let (fa, fb) = (&frames[a], &frames[b]);
        // Azimuthal window of the pair: from the middle of the axes ± (half-sum
        // hfov + margin). Absolute columns (wrap handled by rem_euclid).
        let mid = 0.5 * (fa.yaw_deg + fb.yaw_deg - wrap_signed(fa.yaw_deg - fb.yaw_deg));
        let hfov_a = 2.0 * (fa.width as f32 * 0.5 / fa.f_px).atan().to_degrees();
        let hfov_b = 2.0 * (fb.width as f32 * 0.5 / fb.f_px).atan().to_degrees();
        let half_span = (hfov_a + hfov_b) * 0.5 + 4.0;
        let dpp = 360.0 / seam_w as f32;
        let span_cols = ((half_span / dpp).ceil() as i64).min(seam_w as i64 / 2);
        let mid_col = (((mid + 180.0).rem_euclid(360.0) / 360.0) * seam_w as f32) as i64;

        // Band overlap rows, at the seams' resolution.
        let j0 = (((90.0 - fa.lat_max.min(fb.lat_max)) / 180.0 * seam_h as f32).floor() as i64)
            .clamp(0, seam_h as i64 - 1);
        let j1 = (((90.0 - fa.lat_min.max(fb.lat_min)) / 180.0 * seam_h as f32).ceil() as i64)
            .clamp(0, seam_h as i64 - 1);
        if j1 <= j0 {
            continue;
        }

        // Per-pixel cost and DP: `|Δs| ≤ 1` per row (two-pass min-plus).
        let cols = (span_cols * 2) as usize;
        let rows = (j1 - j0) as usize;
        let inf = f32::INFINITY;
        let mut dp_prev = vec![inf; cols];
        let mut dp_cur = vec![0.0f32; cols];
        // Parent choice: 0 = x−1, 1 = x, 2 = x+1 (relative to the previous
        // row), stored per row for backtracking.
        let mut choice = vec![0u8; rows * cols];
        // Double-coverage interval per row (absolute columns).
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

        // Backtrack from the minimum of the last row.
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
                        0 => 1,                    // parent came from x−1
                        2 => cols.wrapping_sub(1), // parent came from x+1
                        _ => 0,
                    };
                    c = (c + off) % cols;
                }
            }
        }

        // Per-row table: double-coverage interval + cut column.
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

/// wrap into [−180, 180].
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

    /// wrap_signed: circle bounds.
    #[test]
    fn wrap_signed_bornes() {
        assert!((wrap_signed(190.0) - (-170.0)).abs() < 1e-6);
        assert!((wrap_signed(-170.0) - (-170.0)).abs() < 1e-6);
        assert!((wrap_signed(90.0) - 90.0).abs() < 1e-6);
    }
}
