//! Rectilinear projection of a source frame and pre-filtering by latitude
//! band: for each output row, only the frames whose coverage band
//! contains the latitude are considered (a capture ring
//! shares the same pitch — the pre-filter avoids projecting each
//! ray into the 16 cameras for nothing).
//!
//! Frame of reference: see [`crate::geom`].

use crate::geom::Mat3;

/// Prepared frame: world→camera matrix, dimensions, focal length, RGB buffer.
pub struct PreparedFrame {
    /// Pose at the trigger (degrees) — kept to apply an
    /// alignment correction then re-derive [`Self::refresh_pose`].
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    /// World→camera (rows right, down, forward).
    pub inv: Mat3,
    pub width: u32,
    pub height: u32,
    /// Focal length in pixels: `f = (w/2)/tan(hfov/2)` (rectilinear assumption).
    pub f_px: f32,
    /// Latitude band covered by the frame (degrees), roll margin
    /// included — key of the per-output-row pre-filter.
    pub lat_min: f32,
    pub lat_max: f32,
    /// Feather distance at the edges (px).
    pub feather_px: f32,
    /// Multiplicative exposure gain per RGB channel (inter-frame
    /// compensation); `[1.0; 3]` = neutral. The Legacy pipeline replicates a
    /// scalar gain over the 3 channels.
    pub gain: [f32; 3],
    /// Low-frequency **local** gain (120×60 grid of 3° cells,
    /// ordered by rows from lat −90): flattens the intra-scene illumination
    /// gradients (window vs wall) that per-frame scalar gains
    /// cannot absorb — the cause of visible rectangular
    /// joins. All at 1.0 = neutral (Legacy). Rust equivalent
    /// of the OpenCV GainCompensator "block gains".
    pub local_gain: Vec<f32>,
    /// Local warp (mesh): sampling δ per cell — `None` =
    /// pure pose geometry (Legacy, tests). Filled by
    /// `align::estimate_local_warp` (Quality pipeline).
    pub warp: Option<WarpField>,
    pub rgb: Vec<u8>,
}

/// Dimensions of the local gain grid (3° cells).
pub const LOCAL_GRID_W: usize = 120;
pub const LOCAL_GRID_H: usize = 60;

/// LOCAL warp field of a frame: δlon/δlat (degrees) on a control grid
/// covering lon ∈ [yaw−90, yaw+90] × lat ∈ [pitch−60, pitch+60]
/// (7.5° cells). Carried by [`PreparedFrame::warp`]; applied as a
/// ray perturbation BEFORE projection ([`sample_ray`]) — the frame's
/// content then appears shifted by −δ (see `align::estimate_local_warp`
/// for the direction and the gauge).
#[derive(Clone, Debug, Default)]
pub struct WarpField {
    /// (δlon, δlat) per cell, rows from lat+60, columns from
    /// lon yaw−90.
    pub cells: Vec<(f32, f32)>,
}

/// Warp grid cells (7.5° on each axis).
pub const WARP_CELLS_LON: usize = 24;
pub const WARP_CELLS_LAT: usize = 16;

impl WarpField {
    /// Bilinear interpolation of the field at world point (lon, lat).
    /// Missing grid or inconsistent size → (0, 0).
    #[must_use]
    pub fn delta(&self, frame_yaw: f32, frame_pitch: f32, lon: f32, lat: f32) -> (f32, f32) {
        if self.cells.len() != WARP_CELLS_LON * WARP_CELLS_LAT {
            return (0.0, 0.0);
        }
        let mut rel = lon - frame_yaw;
        rel -= 360.0 * (rel / 360.0).round();
        let u = ((rel + 90.0) / 7.5).clamp(0.0, (WARP_CELLS_LON - 1) as f32);
        let v = ((lat - frame_pitch + 60.0) / 7.5).clamp(0.0, (WARP_CELLS_LAT - 1) as f32);
        let (x, y) = (u.floor() as usize, v.floor() as usize);
        let (x1, y1) = (
            (x + 1).min(WARP_CELLS_LON - 1),
            (y + 1).min(WARP_CELLS_LAT - 1),
        );
        let (fx, fy) = (u - x as f32, v - y as f32);
        let at = |xx: usize, yy: usize| self.cells[yy * WARP_CELLS_LON + xx];
        let (c00, c10) = at(x, y);
        let (c01, c11) = at(x, y1);
        let (d00, d10) = at(x1, y);
        let (d01, d11) = at(x1, y1);
        let top = (c00 * (1.0 - fx) + c01 * fx, d00 * (1.0 - fx) + d01 * fx);
        let bot = (c10 * (1.0 - fx) + c11 * fx, d10 * (1.0 - fx) + d11 * fx);
        (
            top.0 * (1.0 - fy) + bot.0 * fy,
            top.1 * (1.0 - fy) + bot.1 * fy,
        )
    }
}

/// Local grid cell index for (lon, lat) in degrees.
#[must_use]
pub fn local_gain_cell(lon: f32, lat: f32) -> usize {
    let ci = (((lon + 180.0) / 3.0).floor().clamp(0.0, 119.0)) as usize;
    let cj = (((lat + 90.0) / 3.0).floor().clamp(0.0, 59.0)) as usize;
    cj * LOCAL_GRID_W + ci
}

impl PreparedFrame {
    /// Local gain at (lon, lat) — no grid (frame without a map): 1.0.
    ///
    /// Bilinear between cell CENTRES (longitude wraps, latitude clamps): a
    /// nearest-cell lookup is piecewise constant and printed the 3° grid as
    /// a visible checkerboard of ~34 px squares at 4096 px (real capture
    /// beb93c42, 2026-09-28).
    #[must_use]
    pub fn local_gain_at(&self, lon: f32, lat: f32) -> f32 {
        if self.local_gain.len() != LOCAL_GRID_W * LOCAL_GRID_H {
            return 1.0;
        }
        let gx = (lon + 180.0) / 3.0 - 0.5;
        let gy = ((lat + 90.0) / 3.0 - 0.5).clamp(0.0, (LOCAL_GRID_H - 1) as f32);
        let x0f = gx.floor();
        let fx = gx - x0f;
        let x0 = (x0f as i32).rem_euclid(LOCAL_GRID_W as i32) as usize;
        let x1 = (x0 + 1) % LOCAL_GRID_W;
        let y0 = gy.floor() as usize;
        let y1 = (y0 + 1).min(LOCAL_GRID_H - 1);
        let fy = gy - y0 as f32;
        let g = |x: usize, y: usize| self.local_gain[y * LOCAL_GRID_W + x];
        let top = g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx;
        let bot = g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx;
        top * (1.0 - fy) + bot * fy
    }
}

impl PreparedFrame {
    /// Re-derives `inv` and the latitude band from the pose (yaw, pitch,
    /// roll). Call after any pose correction: the per-row pre-filter
    /// (`lat_min/lat_max`) and the warp (`inv`) must follow, otherwise
    /// the corrected frames are wrongly excluded from the render.
    pub fn refresh_pose(&mut self) {
        self.inv = crate::geom::euler_to_mat(self.yaw_deg, self.pitch_deg, self.roll_deg);
        let (lat_min, lat_max) = lat_band(self.height, self.f_px, self.pitch_deg);
        self.lat_min = lat_min;
        self.lat_max = lat_max;
    }
}

/// World ray sampled for frame `f` at point (lon, lat):
/// applies the frame's local warp (bilinear δ, see [`WarpField`]) —
/// sampling at (lon+δlon, lat+δlat) makes the content appear shifted by
/// −δ. SINGLE entry point of the render loops (render, multiband,
/// seams, gains): the geometry stays consistent from one stage to the next.
#[must_use]
pub fn sample_ray(f: &PreparedFrame, lon: f32, lat: f32) -> [f32; 3] {
    let (dlon, dlat) = match &f.warp {
        Some(w) => w.delta(f.yaw_deg, f.pitch_deg, lon, lat),
        None => (0.0, 0.0),
    };
    crate::render::ray_of(lon + dlon, lat + dlat)
}

/// Focal length in pixels for a given HFOV (degrees) and width.
#[must_use]
pub fn f_px_from_hfov(hfov_deg: f32, width: u32) -> f32 {
    let half = (width as f32) * 0.5;
    half / to_rad_tan(hfov_deg * 0.5)
}

/// tan(d°) — small wrapper for the readability of focal length formulas.
fn to_rad_tan(deg: f32) -> f32 {
    deg.to_radians().tan()
}

/// Latitude band covered by a frame (degrees, margin included).
/// The vfov is derived from the aspect; we add a 6° margin for
/// poses with roll/pose blur at the trigger.
#[must_use]
pub fn lat_band(height: u32, f_px: f32, pitch_deg: f32) -> (f32, f32) {
    let half_vfov = (height as f32 * 0.5 / f_px).atan().to_degrees();
    const MARGE_DEG: f32 = 6.0;
    (
        pitch_deg - half_vfov - MARGE_DEG,
        pitch_deg + half_vfov + MARGE_DEG,
    )
}

/// Projects a world ray into the frame: `Some((u, v, c))` where `c = cos`
/// of the ray↔optical axis angle (centrality, in (0,1] for a targeted
/// point), `None` outside the sensor. `ray` must be unit.
#[must_use]
pub fn project_uvc(frame: &PreparedFrame, ray: [f32; 3]) -> Option<(f32, f32, f32)> {
    let [a, b, c] = frame.inv.mul_vec(ray);
    if c <= f32::EPSILON {
        return None;
    }
    let u = frame.width as f32 * 0.5 + frame.f_px * a / c;
    let v = frame.height as f32 * 0.5 + frame.f_px * b / c;
    // Half-pixel off-edge tolerance: bilinear sampling stays
    // defined and the feather weights vanish at the edge.
    if u < -0.5 || v < -0.5 || u > frame.width as f32 - 0.5 || v > frame.height as f32 - 0.5 {
        return None;
    }
    // c = axis·ray (both unit) = cos of the angle to the axis.
    Some((u, v, c))
}

/// Projects a world ray into the frame; `None` outside the sensor.
/// `ray` must be unit. (Keeps the historical (u, v) API.)
#[must_use]
pub fn project(frame: &PreparedFrame, ray: [f32; 3]) -> Option<(f32, f32)> {
    project_uvc(frame, ray).map(|(u, v, _)| (u, v))
}

/// Selection weight: normalised distance to the nearest edge, in
/// `[0, 1]` (1 at the frame centre, 0 on the rim). Raised to a
/// power on the render side, it makes the frame that sees the pixel the most
/// centrally "win" → sharp selection instead of a ghosting average, with a
/// narrow feather only where two frames are equal (the seam).
#[must_use]
pub fn interior_weight(frame: &PreparedFrame, u: f32, v: f32) -> f32 {
    let dist = u
        .min(v)
        .min(frame.width as f32 - u)
        .min(frame.height as f32 - v);
    let half_min = (frame.width.min(frame.height) as f32) * 0.5;
    if half_min <= 0.0 {
        return 0.0;
    }
    (dist / half_min).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::euler_to_mat;

    fn frame_yields(width: u32, height: u32, hfov: f32) -> PreparedFrame {
        let f = f_px_from_hfov(hfov, width);
        PreparedFrame {
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            inv: euler_to_mat(0.0, 0.0, 0.0),
            width,
            height,
            f_px: f,
            lat_min: -1.0,
            lat_max: 1.0,
            feather_px: 0.0,
            gain: [1.0; 3],
            local_gain: Vec::new(),
            warp: None,
            rgb: vec![0; (width * height * 3) as usize],
        }
    }

    /// refresh_pose: changing the pose then re-deriving must move the
    /// latitude band and rebuild `inv` identically to a direct
    /// construction.
    #[test]
    fn refresh_pose_redrive() {
        let mut fr = frame_yields(1000, 750, 90.0);
        fr.yaw_deg = 30.0;
        fr.pitch_deg = 20.0;
        fr.refresh_pose();
        let half_vfov = ((fr.height as f32 * 0.5) / fr.f_px).atan().to_degrees();
        assert!((fr.lat_min - (20.0 - half_vfov - 6.0)).abs() < 1e-4);
        assert!((fr.lat_max - (20.0 + half_vfov + 6.0)).abs() < 1e-4);
        let direct = euler_to_mat(30.0, 20.0, 0.0);
        assert_eq!(fr.inv, direct);
    }

    /// Local gains are continuous across cell borders (no checkerboard),
    /// exact at cell centres, and wrap across the ±180° seam.
    #[test]
    fn local_gain_is_bilinear_between_cell_centres() {
        let mut fr = frame_yields(100, 100, 60.0);
        fr.local_gain = vec![1.0; LOCAL_GRID_W * LOCAL_GRID_H];
        // Cell (60, 30) = lon [0°, 3°), lat [0°, 3°), centre (1.5°, 1.5°).
        fr.local_gain[30 * LOCAL_GRID_W + 60] = 1.3;
        assert!((fr.local_gain_at(1.5, 1.5) - 1.3).abs() < 1e-5);
        // Straddling the cell's left border: no jump.
        let (left, right) = (fr.local_gain_at(-0.01, 1.5), fr.local_gain_at(0.01, 1.5));
        assert!((left - right).abs() < 0.01, "jump {left} → {right}");
        // Halfway between two centres: the average.
        assert!((fr.local_gain_at(3.0, 1.5) - 1.15).abs() < 1e-5);
        // Longitude wraps: last and first columns blend at ±180°.
        fr.local_gain[30 * LOCAL_GRID_W + (LOCAL_GRID_W - 1)] = 1.2;
        let (west, east) = (
            fr.local_gain_at(-179.99, 1.5),
            fr.local_gain_at(179.99, 1.5),
        );
        assert!((west - east).abs() < 0.01, "seam {west} vs {east}");
    }

    /// Focal length: full hfov → the left/right edges at ±w/2 from the centre.
    #[test]
    fn focal_hfov() {
        let f = f_px_from_hfov(90.0, 1000);
        assert!((f - 500.0).abs() < 1e-3); // tan(45°) = 1
        let fr = frame_yields(1000, 750, 90.0);
        // Axis ray → exact centre.
        let (u, v) = project(&fr, [1.0, 0.0, 0.0]).unwrap();
        assert!((u - 500.0).abs() < 1e-3 && (v - 375.0).abs() < 1e-3);
        // Ray at 44° on the right side (lon −44° see frame) — NOT exactly 45°:
        // the exact edge falls on u = w, excluded by the half-pixel tolerance.
        let (lon, lat) = (-44.0_f32.to_radians(), 0.0_f32);
        let ray = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        let (u, _) = project(&fr, ray).unwrap();
        let expected = 500.0 + 500.0 * 44.0_f32.to_radians().tan();
        assert!((u - expected).abs() < 1.0, "u = {u}, expected {expected}");
    }

    /// Centrality: axis ray → c ≈ 1; oblique ray → smaller c.
    #[test]
    fn centrality_in_unit_range() {
        let fr = frame_yields(1000, 750, 90.0);
        let (_, _, c0) = project_uvc(&fr, [1.0, 0.0, 0.0]).unwrap();
        assert!((c0 - 1.0).abs() < 1e-4, "c axe = {c0}");
        let (lon, lat) = (-30.0_f32.to_radians(), 0.0_f32);
        let ray = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        let (_, _, c1) = project_uvc(&fr, ray).unwrap();
        assert!(c1 < c0 && c1 > 0.5, "c oblique = {c1}");
    }

    /// interior_weight: ~1 at the centre, 0 at the edge.
    #[test]
    fn interior_weight_centre_to_edge() {
        let fr = frame_yields(800, 600, 65.0);
        assert!((interior_weight(&fr, 400.0, 300.0) - 1.0).abs() < 1e-6);
        assert!(interior_weight(&fr, 0.0, 300.0) < 1e-6);
        assert!(interior_weight(&fr, 400.0, 600.0) < 1e-6);
    }

    /// Out of view: behind the camera or beyond the sensor → None.
    #[test]
    fn out_of_view() {
        let fr = frame_yields(1000, 750, 60.0);
        assert!(project(&fr, [-1.0, 0.0, 0.0]).is_none());
        // lon 90° is far outside the hfov 60 → None
        assert!(project(&fr, [0.0, 1.0, 0.0]).is_none());
    }
}
