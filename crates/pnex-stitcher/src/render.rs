//! Equirectangular render: spherical warp + **selection** (not averaging) +
//! pole filling, preceded by an inter-frame **gain compensation**.
//!
//! For each output pixel `(lon, lat)` → world unit ray → for
//! each eligible frame (latitude band pre-filter) → rectilinear
//! projection → bilinear sampling → weighting by distance to the
//! edge raised to [`SELECT_POWER`]. A high power makes the frame that
//! sees the pixel most centrally dominate (sharp seam), while keeping a
//! narrow feather where two frames are equal. The accumulators are f32 per
//! output row (no inter-row state) — bounded memory.
//!
//! SERIAL RENDER, deliberately WITHOUT rayon: the global rayon pool
//! deadlocks on Android devices (workers futex-blocked at init, finding
//! 2026-09-09 — 4 `pnex-stitch` threads at 0 % CPU). At 1024 px output the
//! serial render takes ~1-2 s on modern ARM.
//!
//! Frame of reference: see [`crate::geom`].

use crate::project::{interior_weight, project_uvc, PreparedFrame};

/// Selection exponent: `weight = interior^P`. The larger it is, the more
/// clearly the most central frame wins (sharp seam, narrow
/// feather); the smaller it is, the more the overlap is averaged (ghost).
pub const SELECT_POWER: f32 = 24.0;

/// Near-saturated sample (any channel ≥ 250): a clipped value carries no
/// exposure ratio — a blown-out window reads 255 in every frame whatever
/// its exposure, so such pairs pulled the gains towards 1 and left whole
/// frames visibly darker or tinted (real capture beb93c42, 2026-09-28).
fn is_clipped(col: [f32; 3]) -> bool {
    col.iter().any(|&v| v >= 250.0)
}

/// Estimates a multiplicative exposure gain per frame and per channel by
/// equalising the mean in the overlaps (pure Rust equivalent of OpenCV's
/// `GainCompensator`). `channels = 1` equalises the luminance
/// (Rec.601) and replicates the gain over the 3 channels (Legacy behaviour);
/// `channels = 3` solves R, G and B independently (fixes white balance
/// drifts between frames).
///
/// Coarse grid of rays; for each ray seen by ≥ 2 frames (far
/// from the edges), we accumulate the value per pair. We then solve
/// log-gains by relaxation (Gauss-Seidel), anchored at zero mean, then
/// `gain = exp(G)` bounded to `[0.5, 2.0]`. No algebra dependency.
#[must_use]
pub fn estimate_gains(frames: &[PreparedFrame], channels: u32) -> Vec<[f32; 3]> {
    let n = frames.len();
    let mut gains = vec![[1.0f32; 3]; n];
    if n < 2 {
        return gains;
    }
    let per_channel: bool = channels >= 3;
    // For each processed channel: the compared value (lum or raw channel).
    let channel_of = |col: [f32; 3], c: usize| -> f64 {
        if per_channel {
            col[c] as f64
        } else {
            0.299 * col[0] as f64 + 0.587 * col[1] as f64 + 0.114 * col[2] as f64
        }
    };
    // The result of logical channel `c` is replicated over these RGB channels.
    let mapped = |c: usize| -> usize {
        if per_channel {
            c
        } else {
            0
        }
    };

    for c in 0..if per_channel { 3 } else { 1 } {
        // Per-pair accumulators (a < b): (somme_a, somme_b, compte).
        let mut pair: std::collections::HashMap<(usize, usize), (f64, f64, u32)> =
            std::collections::HashMap::new();

        // Coarse grid on the sphere (3° × 3°).
        let mut lat = -87.0f32;
        while lat <= 87.0 {
            let mut lon = -180.0f32;
            while lon < 180.0 {
                // Frames seeing this ray centrally enough (avoids vignetting/edges).
                let mut seen: Vec<(usize, f64)> = Vec::new();
                for (k, f) in frames.iter().enumerate() {
                    if lat < f.lat_min || lat > f.lat_max {
                        continue;
                    }
                    if let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat))
                    {
                        if interior_weight(f, u, v) > 0.25 {
                            let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                            let val = channel_of(col, c);
                            if val > 1.0 && !is_clipped(col) {
                                seen.push((k, val));
                            }
                        }
                    }
                }
                for i in 0..seen.len() {
                    for j in (i + 1)..seen.len() {
                        let (a, la) = seen[i];
                        let (b, lb) = seen[j];
                        let key = if a < b { (a, b) } else { (b, a) };
                        let (sa, sb) = if a < b { (la, lb) } else { (lb, la) };
                        let e = pair.entry(key).or_insert((0.0, 0.0, 0));
                        e.0 += sa;
                        e.1 += sb;
                        e.2 += 1;
                    }
                }
                lon += 3.0;
            }
            lat += 3.0;
        }

        // Graph edges: delta_{a→b} = ln(mean_b/mean_a), weight = count.
        // We want G_a + ln(mean_a) ≈ G_b + ln(mean_b) (equalised values).
        let mut adj: Vec<Vec<(usize, f32, f32)>> = vec![Vec::new(); n];
        for (&(a, b), &(sa, sb, cnt)) in pair.iter() {
            if cnt < 12 {
                continue; // overlap too thin → noise
            }
            let (ma, mb) = (sa / cnt as f64, sb / cnt as f64);
            if ma <= 0.0 || mb <= 0.0 {
                continue;
            }
            let w = cnt as f32;
            let d_ab = (mb / ma).ln() as f32;
            adj[a].push((b, w, d_ab));
            adj[b].push((a, w, -d_ab));
        }

        // IN-PLACE Gauss-Seidel relaxation (Jacobi iteration oscillates
        // forever on bipartite graphs — even ring of frames —
        // and never converges: gains stayed neutral in V1, finding
        // 2026-09-10). Zero-mean anchoring at each sweep.
        let mut g = vec![0.0f32; n];
        for _ in 0..80 {
            for k in 0..n {
                let mut num = 0.0f32;
                let mut den = 0.0f32;
                for &(nb, w, d) in &adj[k] {
                    num += w * (g[nb] + d);
                    den += w;
                }
                if den > 0.0 {
                    g[k] = num / den;
                }
            }
            let mean = g.iter().sum::<f32>() / n as f32;
            for x in g.iter_mut() {
                *x -= mean;
            }
        }

        let dst = mapped(c);
        for k in 0..n {
            let gain = g[k].exp().clamp(0.5, 2.0);
            if per_channel {
                gains[k][dst] = gain;
            } else {
                gains[k] = [gain; 3];
            }
        }
    }

    // Colour bounding: when a frame sees a weakly coloured area (white
    // ceiling), per-channel equalisation DECOUPLES and the R/G/B correction
    // goes off the charts → washed-out image (real capture finding
    // 2026-09-10). The deviation of each channel is brought back to ±15 % of the
    // luminance gain (geometric mean of the channels) — white balance
    // correction stays possible, drift does not.
    if per_channel {
        for g3 in gains.iter_mut() {
            let lum = (g3[0] * g3[1] * g3[2]).cbrt();
            let (lo, hi) = (lum * 0.85, lum * 1.15);
            g3[0] = g3[0].clamp(lo, hi);
            g3[1] = g3[1].clamp(lo, hi);
            g3[2] = g3[2].clamp(lo, hi);
        }
    }
    gains
}

/// Estimates the low-frequency **local** gain per frame (3° grid): for
/// each cell seen by ≥ 2 frames, each frame is brought back to the mean
/// of the frames covering the cell — flattens the intra-scene illumination
/// gradients (bright window vs dark wall) responsible for the rectangular
/// joins. Two 3×3 smoothings on the grid, clamp [0.7, 1.4].
pub fn estimate_local_gains(frames: &mut [PreparedFrame]) {
    let n = frames.len();
    if n < 2 {
        return;
    }
    for f in frames.iter_mut() {
        f.local_gain = vec![1.0; crate::project::LOCAL_GRID_W * crate::project::LOCAL_GRID_H];
    }
    // Gained luminance accumulated per cell and per frame.
    let cells = crate::project::LOCAL_GRID_W * crate::project::LOCAL_GRID_H;
    let mut sums = vec![vec![0.0f64; n]; cells];
    let mut counts = vec![vec![0u32; n]; cells];
    let mut covered_by = vec![vec![false; n]; cells];

    let mut lat = -88.5f32;
    while lat <= 88.5 {
        let mut lon = -178.5f32;
        while lon < 180.0 {
            let cell = crate::project::local_gain_cell(lon, lat);
            for (k, f) in frames.iter().enumerate() {
                if lat < f.lat_min || lat > f.lat_max {
                    continue;
                }
                if let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat)) {
                    if interior_weight(f, u, v) > 0.25 {
                        let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                        if is_clipped(col) {
                            continue;
                        }
                        let g = f.gain;
                        let lum = (0.299 * (col[0] * g[0]).min(255.0)
                            + 0.587 * (col[1] * g[1]).min(255.0)
                            + 0.114 * (col[2] * g[2]).min(255.0))
                            as f64;
                        if lum > 1.0 {
                            sums[cell][k] += lum;
                            counts[cell][k] += 1;
                            covered_by[cell][k] = true;
                        }
                    }
                }
            }
            lon += 3.0;
        }
        lat += 3.0;
    }

    // Correction per frame and cell: bring each frame back to the geometric
    // mean of the frames covering the cell (≥ 2 frames).
    let mut raw = vec![vec![1.0f32; n]; cells];
    for cell in 0..cells {
        let mut n_cov = 0usize;
        let mut log_mean = 0.0f64;
        for k in 0..n {
            if covered_by[cell][k] && counts[cell][k] > 0 {
                log_mean += (sums[cell][k] / counts[cell][k] as f64).ln();
                n_cov += 1;
            }
        }
        if n_cov < 2 {
            continue;
        }
        log_mean /= n_cov as f64;
        for k in 0..n {
            if covered_by[cell][k] {
                let own = (sums[cell][k] / counts[cell][k] as f64).ln();
                raw[cell][k] = (log_mean - own).exp().clamp(0.7, 1.4) as f32;
            }
        }
    }

    // Smoothing: 2 3×3 box passes on the grid (uncovered cells
    // keep 1.0 in the mean).
    for (k, frame) in frames.iter_mut().enumerate() {
        let (gw, gh) = (crate::project::LOCAL_GRID_W, crate::project::LOCAL_GRID_H);
        let mut cur = raw_cell_grid(&raw, k, gw, gh);
        for _ in 0..2 {
            let mut next = cur.clone();
            for j in 0..gh {
                for i in 0..gw {
                    let mut acc = 0.0f32;
                    let mut cnt = 0u32;
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let y = (j as i32 + dy).clamp(0, gh as i32 - 1) as usize;
                            let x = (i as i32 + dx).rem_euclid(gw as i32) as usize;
                            acc += cur[y * gw + x];
                            cnt += 1;
                        }
                    }
                    next[j * gw + i] = acc / cnt as f32;
                }
            }
            cur = next;
        }
        frame.local_gain = cur;
    }
}

/// Extracts the flat grid of a frame from the raw per-cell values.
fn raw_cell_grid(raw: &[Vec<f32>], k: usize, gw: usize, gh: usize) -> Vec<f32> {
    let mut out = vec![1.0f32; gw * gh];
    for (cell, grid) in out.iter_mut().enumerate() {
        *grid = raw[cell][k];
    }
    let _ = (gw, gh);
    out
}

/// 2:1 equirectangular render of width `out_width`.
///
/// Returns the RGB8 buffer, the number of covered pixels per row and the
/// per-pixel coverage mask. The frames may be empty (black render,
/// zero coverage) — `stitch` validates upstream.
#[must_use]
pub fn render(frames: &[PreparedFrame], out_width: u32) -> (Vec<u8>, Vec<u32>, Vec<bool>) {
    let out_h = (out_width / 2).max(1);
    let mut rgb = vec![0u8; (out_width * out_h * 3) as usize];
    let mut covered = vec![0u32; out_h as usize];
    let mut mask = vec![false; (out_width * out_h) as usize];

    // Per-row pre-filter: eligible frame indices, computed once.
    let eligible: Vec<Vec<usize>> = (0..out_h)
        .map(|j| {
            let lat = lat_of(j, out_h);
            frames
                .iter()
                .enumerate()
                .filter(|(_, f)| lat >= f.lat_min && lat <= f.lat_max)
                .map(|(k, _)| k)
                .collect()
        })
        .collect();

    let row_len = (out_width * 3) as usize;
    // Serial row-by-row render (see module docs: rayon forbidden on
    // device). The loop returns (covered count, pixel mask) per row.
    let per_row: Vec<(u32, Vec<bool>)> = rgb
        .chunks_mut(row_len)
        .enumerate()
        .map(|(j, row)| {
            let lat = lat_of(j as u32, out_h);
            let elig = &eligible[j];
            let mut sum = vec![0.0_f32; (out_width * 3) as usize];
            let mut wsum = vec![0.0_f32; out_width as usize];
            let mut row_mask = vec![false; out_width as usize];

            for &k in elig {
                let f = &frames[k];
                for i in 0..out_width as usize {
                    let lon = lon_of(i as u32, out_width);
                    let Some((u, v, _)) = project_uvc(f, crate::project::sample_ray(f, lon, lat))
                    else {
                        continue;
                    };
                    // Selection: distance to the edge ^ P. The frame that sees the
                    // pixel most centrally dominates; narrow feather at the seam.
                    let iw = interior_weight(f, u, v);
                    if iw <= 0.0 {
                        continue;
                    }
                    let w = iw.powf(SELECT_POWER);
                    if w <= 0.0 {
                        continue;
                    }
                    let col = sample_bilinear(&f.rgb, f.width, f.height, u, v);
                    let g = f.gain;
                    sum[i * 3] += w * (col[0] * g[0]).min(255.0);
                    sum[i * 3 + 1] += w * (col[1] * g[1]).min(255.0);
                    sum[i * 3 + 2] += w * (col[2] * g[2]).min(255.0);
                    wsum[i] += w;
                }
            }

            let mut n_couverts = 0u32;
            for (i, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                if wsum[i] > 0.0 {
                    px[0] = (sum[i * 3] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    px[1] = (sum[i * 3 + 1] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    px[2] = (sum[i * 3 + 2] / wsum[i]).round().clamp(0.0, 255.0) as u8;
                    row_mask[i] = true;
                    n_couverts += 1;
                }
            }
            (n_couverts, row_mask)
        })
        .collect();

    for (j, (count, row_mask)) in per_row.into_iter().enumerate() {
        covered[j] = count;
        let base = j * out_width as usize;
        mask[base..base + out_width as usize].copy_from_slice(&row_mask);
    }

    (rgb, covered, mask)
}

/// Dresses the top and bottom outside the **solid band** (the latitudes not
/// covered by the capture ring) cleanly, instead of vertically spreading
/// the band edge (which gave streaks and a wavy "broken"
/// join).
///
/// A single-ring capture only covers a slice of the equirect: we cannot
/// invent the caps. So we render them *regular* — edge colour
/// smoothed horizontally (end of the streaks), soft feather from the band
/// (end of the wavy hard edge), convergence towards the mean edge hue towards
/// the pole (plain sky/floor). Result: sharp content band + plain
/// caps, rather than a torn letterbox.
pub fn fill_poles(rgb: &mut [u8], out_width: u32, covered: &[u32]) {
    let out_h = covered.len();
    let w = out_width as usize;
    if out_h == 0 || w == 0 {
        return;
    }
    let row_len = w * 3;

    // "Solid" band: widely covered rows (decreasing threshold if
    // coverage is low). Everything above/below is dressed.
    let first_where = |t: f32| {
        covered
            .iter()
            .position(|&c| c as f32 >= t * out_width as f32)
    };
    let last_where = |t: f32| {
        covered
            .iter()
            .rposition(|&c| c as f32 >= t * out_width as f32)
    };
    let (top, bot) = [0.9f32, 0.5, 0.01]
        .iter()
        .find_map(|&t| Some((first_where(t)?, last_where(t)?)))
        .unwrap_or((0, out_h - 1));

    // Horizontally smoothed copy (blur with circular wrap — the
    // longitude wraps) of the edge row: kills the vertical streaks.
    let smoothed = |rgb: &[u8], j: usize| -> Vec<f32> {
        let r = (w / 32).max(1) as i32;
        let base = j * row_len;
        let mut out = vec![0.0f32; row_len];
        for i in 0..w {
            let mut acc = [0.0f32; 3];
            for d in -r..=r {
                let x = ((i as i32 + d).rem_euclid(w as i32)) as usize;
                for k in 0..3 {
                    acc[k] += rgb[base + x * 3 + k] as f32;
                }
            }
            let n = (2 * r + 1) as f32;
            for k in 0..3 {
                out[i * 3 + k] = acc[k] / n;
            }
        }
        out
    };
    let mean_of = |row: &[f32]| -> [f32; 3] {
        let mut m = [0.0f32; 3];
        for i in 0..w {
            for k in 0..3 {
                m[k] += row[i * 3 + k];
            }
        }
        [m[0] / w as f32, m[1] / w as f32, m[2] / w as f32]
    };

    // Top dressing: from the band (smoothed edge) towards the pole (plain hue).
    if top > 0 {
        let edge = smoothed(rgb, top);
        let mean = mean_of(&edge);
        for j in 0..top {
            let a = j as f32 / top as f32; // 0 at the pole, 1 at the band edge
            let dst = j * row_len;
            for i in 0..w {
                for k in 0..3 {
                    let v = mean[k] * (1.0 - a) + edge[i * 3 + k] * a;
                    rgb[dst + i * 3 + k] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    // Bottom dressing: symmetric.
    if bot < out_h - 1 {
        let edge = smoothed(rgb, bot);
        let mean = mean_of(&edge);
        let denom = (out_h - 1 - bot) as f32;
        for j in (bot + 1)..out_h {
            let a = (out_h - 1 - j) as f32 / denom; // 1 below the band, 0 at the pole
            let dst = j * row_len;
            for i in 0..w {
                for k in 0..3 {
                    let v = mean[k] * (1.0 - a) + edge[i * 3 + k] * a;
                    rgb[dst + i * 3 + k] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Coverage of the **covered band**: fraction of really covered pixels
/// among the rows containing at least one covered pixel, measured
/// BEFORE filling (intra-band holes — rectilinear corner insets,
/// missing frame — lower the metric, that is intended).
///
/// A horizontal ring only covers a slice of the equirect: the threshold
/// is judged WITHIN the band, never over the whole sphere (the poles are
/// filled afterwards by spreading).
#[must_use]
pub fn band_coverage(out_width: u32, covered: &[u32]) -> f32 {
    let total: u64 = covered.iter().filter(|&&c| c > 0).map(|&c| c as u64).sum();
    let rows = covered.iter().filter(|&&c| c > 0).count() as u64;
    if rows == 0 {
        return 0.0;
    }
    total as f32 / (rows * out_width as u64) as f32
}

/// Fills **intra-band** holes by **vertical extension**: every uncovered
/// pixel receives the blend of the nearest covered neighbours
/// above and below (same column), weighted by the inverse of the
/// distances. Vertical structures (walls, frames, ceiling→wall)
/// stay credible where the former horizontal fill (left/right
/// mean) stretched the edges into "ladder" streaks — finding from a
/// real 3-ring capture 2026-09-10 (equator azimuthal hole ~33°:
/// the ring did not close, 15 frames × ~20° + hfov 43.7 < 360°).
pub fn fill_band_holes(rgb: &mut [u8], out_width: u32, mask: &[bool]) {
    let w = out_width as usize;
    let h = mask.len() / w;
    if h == 0 || w == 0 {
        return;
    }
    let row_len = w * 3;

    // Column-by-column vertical extension (O(h) memory): dist_up /
    // dist_dn = distances to the covered neighbours above/below
    // (-1 = none).
    for i in 0..w {
        let mut dist_up = vec![-1i32; h];
        let mut dist_dn = vec![-1i32; h];
        let mut last: i32 = -1;
        for j in 0..h {
            if mask[j * w + i] {
                last = j as i32;
                continue;
            }
            dist_up[j] = if last >= 0 { j as i32 - last } else { -1 };
        }
        let mut last: i32 = -1;
        for j in (0..h).rev() {
            if mask[j * w + i] {
                last = j as i32;
                continue;
            }
            dist_dn[j] = if last >= 0 { last - j as i32 } else { -1 };
        }
        // 1/distance weighted blend (the nearest neighbour dominates).
        for j in 0..h {
            let (du, dd) = (dist_up[j], dist_dn[j]);
            if du < 0 && dd < 0 {
                continue;
            }
            let wa = if du >= 0 { 1.0 / du as f32 } else { 0.0 };
            let wb = if dd >= 0 { 1.0 / dd as f32 } else { 0.0 };
            let s = wa + wb;
            let dst = j * row_len + i * 3;
            let mut v = [0.0f32; 3];
            if du >= 0 {
                let sa = (j - du as usize) * row_len + i * 3;
                for c in 0..3 {
                    v[c] += wa * rgb[sa + c] as f32;
                }
            }
            if dd >= 0 {
                let sb = (j + dd as usize) * row_len + i * 3;
                for c in 0..3 {
                    v[c] += wb * rgb[sb + c] as f32;
                }
            }
            for c in 0..3 {
                rgb[dst + c] = (v[c] / s).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    // Horizontal smoothing of the filled pixels (radius 2): the column-by-column
    // extension creates fine vertical striping (extended neighbours from
    // edge rows with slightly different content). Short radius — we
    // keep the transition to the real content at the hole edges.
    let r = 2usize;
    let out = rgb.to_vec();
    for j in 0..h {
        let base = j * row_len;
        for i in 0..w {
            if mask[j * w + i] {
                continue;
            }
            for c in 0..3 {
                let mut acc = 0.0f32;
                let mut cnt = 0u32;
                for d in -(r as i64)..=(r as i64) {
                    let x = (i as i64 + d).rem_euclid(w as i64) as usize;
                    if mask[j * w + x] {
                        continue;
                    }
                    acc += out[base + x * 3 + c] as f32;
                    cnt += 1;
                }
                if cnt > 0 {
                    rgb[base + i * 3 + c] = (acc / cnt as f32).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

// ─────────────────────────── sphere helpers ───────────────────────────

/// Horizontal mirror of the equirect buffer (rgb + mask) — the standard
/// display handedness (u grows when turning RIGHT, clockwise seen from the
/// sky) is obtained by inverting the internal mapping (u grows with ENU
/// lon). A single inversion in the whole chain, at the render
/// boundary: the whole internal pipeline (NCC align + probe, solve, seams,
/// multiband, gains) keeps the historical convention locked by
/// the probes. Device finding 2026-09-11: without this mirror, the panorama
/// comes out "clean but mirrored" left-right.
pub(crate) fn mirror_rows(rgb: &mut [u8], out_width: u32) {
    let w = out_width as usize;
    let h = rgb.len() / (w * 3);
    for j in 0..h {
        let base = j * w * 3;
        for i in 0..w / 2 {
            for c in 0..3 {
                rgb.swap(base + i * 3 + c, base + (w - 1 - i) * 3 + c);
            }
        }
    }
}

/// Horizontal mirror of the per-pixel coverage mask — must accompany
/// [`mirror_rows`]: `fill_band_holes` consumes rgb and mask together.
pub(crate) fn mirror_mask(mask: &mut [bool], out_width: u32) {
    let w = out_width as usize;
    let h = mask.len() / w;
    for j in 0..h {
        let base = j * w;
        for i in 0..w / 2 {
            mask.swap(base + i, base + (w - 1 - i));
        }
    }
}

/// Latitude (degrees) of the centre of output row `j`.
pub(crate) fn lat_of(j: u32, out_h: u32) -> f32 {
    90.0 - (j as f32 + 0.5) * 180.0 / out_h as f32
}

/// Longitude (degrees) of the centre of the pixel in column `i`.
///
/// INTERNAL convention (align + probe: u grows with ENU longitude).
/// The standard display handedness (u grows when turning RIGHT,
/// clockwise seen from the sky) is applied on return from [`render`] by
/// mirroring the rows — see `mirror_rows` + probe `rendu_handedness`.
/// Without this output mirror, the render was MIRRORED left-right:
/// self-consistent (clean seams) but inverted vs. the real scene,
/// device finding 2026-09-11.
pub(crate) fn lon_of(i: u32, out_width: u32) -> f32 {
    -180.0 + (i as f32 + 0.5) * 360.0 / out_width as f32
}

/// World unit ray from (lon, lat) in degrees.
pub(crate) fn ray_of(lon_deg: f32, lat_deg: f32) -> [f32; 3] {
    let (lo, la) = (lon_deg.to_radians(), lat_deg.to_radians());
    let (sl, cl) = lo.sin_cos();
    let (sp, cp) = la.sin_cos();
    [cp * cl, cp * sl, sp]
}

/// Bilinear RGB8 sampling with edge clamping.
pub(crate) fn sample_bilinear(buf: &[u8], w: u32, h: u32, u: f32, v: f32) -> [f32; 3] {
    let x = u.clamp(0.0, (w - 1) as f32);
    let y = v.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let at = |xx: u32, yy: u32| -> [f32; 3] {
        let o = ((yy * w + xx) * 3) as usize;
        [buf[o] as f32, buf[o + 1] as f32, buf[o + 2] as f32]
    };
    let c00 = at(x0, y0);
    let c10 = at(x1, y0);
    let c01 = at(x0, y1);
    let c11 = at(x1, y1);

    let mut out = [0.0_f32; 3];
    for k in 0..3 {
        let top = c00[k] * (1.0 - fx) + c10[k] * fx;
        let bot = c01[k] * (1.0 - fx) + c11[k] * fx;
        out[k] = top * (1.0 - fy) + bot * fy;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sphere conventions: row 0 = north pole, increasing u = turning
    /// RIGHT (lon decreasing from 180 → −180) — standard equirect
    /// handedness (see lon_of doc).
    /// Sphere conventions (internal): row 0 = north pole, lon increasing
    /// from left to right on the grid (−180 → +180). The display
    /// handedness is applied at the boundary by `mirror_rows` (see
    /// probe `rendu_handedness`).
    #[test]
    fn conventions_sphere() {
        assert!((lat_of(0, 1024) - 89.9).abs() < 0.1);
        assert!((lat_of(1023, 1024) + 89.9).abs() < 0.1);
        assert!((lon_of(0, 2048) + 180.0).abs() < 0.1);
        assert!((lon_of(2047, 2048) - 180.0).abs() < 0.1);
        assert!((lon_of(1023, 2048)).abs() < 0.2);
        let r = ray_of(0.0, 90.0);
        assert!((r[2] - 1.0).abs() < 1e-6);
    }
    /// Per-channel gains: a ×1.4 frame must be brought back ≈ ×1/1.4 by
    /// the overlap equalisation (independent channels).
    #[test]
    fn gains_per_channel_egalise() {
        use crate::geom::euler_to_mat;
        use crate::project::{f_px_from_hfov, lat_band, PreparedFrame};
        fn frame(k: usize, expo: f32) -> PreparedFrame {
            let (w, h, hfov) = (128u32, 96u32, 65.0_f32);
            let f_px = f_px_from_hfov(hfov, w);
            let yaw = k as f32 * 40.0;
            let m = euler_to_mat(yaw, 0.0, 0.0);
            let mut rgb = vec![0u8; (w * h * 3) as usize];
            for j in 0..h {
                for i in 0..w {
                    let a = (i as f32 + 0.5 - w as f32 / 2.0) / f_px;
                    let b = (j as f32 + 0.5 - h as f32 / 2.0) / f_px;
                    let world = m.transpose().mul_vec([a, b, 1.0]);
                    let lon = world[1].atan2(world[0]).to_degrees();
                    let n =
                        (world[0] * world[0] + world[1] * world[1] + world[2] * world[2]).sqrt();
                    let lat = (world[2] / n).asin().to_degrees();
                    let (lo, la) = (lon.to_radians(), lat.to_radians());
                    let mut v = 0.0f32;
                    for &(cy, cx, amp, p, q) in &[
                        (3, 2, 0.30, 0.7, 1.3),
                        (7, 5, 0.25, 2.1, 0.4),
                        (13, 9, 0.22, 4.7, 2.9),
                        (23, 17, 0.15, 1.8, 5.2),
                    ] {
                        v += amp * ((cy as f32 * lo + p).sin() * (cx as f32 * la + q).sin());
                    }
                    let val = (128.0 + 55.0 * v.clamp(-0.92, 0.92)) * expo;
                    let o = ((j * w + i) * 3) as usize;
                    rgb[o] = val.round().clamp(0.0, 255.0) as u8;
                    rgb[o + 1] = val.round().clamp(0.0, 255.0) as u8;
                    rgb[o + 2] = val.round().clamp(0.0, 255.0) as u8;
                }
            }
            PreparedFrame {
                yaw_deg: yaw,
                pitch_deg: 0.0,
                roll_deg: 0.0,
                inv: m,
                width: w,
                height: h,
                f_px,
                lat_min: lat_band(h, f_px, 0.0).0,
                lat_max: lat_band(h, f_px, 0.0).1,
                feather_px: 1.0,
                gain: [1.0; 3],
                local_gain: Vec::new(),
                warp: None,
                rgb,
            }
        }
        let frames = vec![frame(0, 1.0), frame(1, 1.4)];
        let gains = estimate_gains(&frames, 3);
        // The gauge (log-mean anchoring) splits the correction between the two
        // frames; it is the RATIO that must cancel the exposure gap.
        let ratio = gains[1][0] / gains[0][0];
        eprintln!("DIAG gains = {gains:?}");
        assert!(
            (ratio - 1.0 / 1.4).abs() <= 0.03,
            "gain ratio = {ratio:.3}, expected ≈ {:+.3}",
            1.0 / 1.4
        );
    }

    /// Band coverage: full, partially holed, empty.
    #[test]
    fn couverture_bande() {
        let w = 100u32;
        assert!((band_coverage(w, &[0, 10, 100, 100, 100, 10, 0]) - 0.64).abs() < 1e-6);
        assert!((band_coverage(w, &[100, 100, 92, 100]) - 0.98).abs() < 1e-6);
        assert!(band_coverage(w, &[0, 0]).abs() < 1e-6);
    }
}
