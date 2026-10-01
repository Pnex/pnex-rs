//! Projection rectiligne d'une frame source et pré-filtrage par bande de
//! latitude : pour chaque ligne de sortie, seules les frames dont la bande
//! de couverture contient la latitude sont considérées (un anneau de
//! capture partage le même pitch — le pré-filtre évite de projeter chaque
//! rayon dans les 16 caméras pour rien).
//!
//! Repère : cf. [`crate::geom`].

use crate::geom::Mat3;

/// Frame préparée : matrice monde→caméra, dimensions, focale, buffer RGB.
pub struct PreparedFrame {
    /// Pose au déclenchement (degrés) — conservée pour appliquer une
    /// correction d'alignement puis re-dériver [`Self::refresh_pose`].
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub roll_deg: f32,
    /// Monde→caméra (rangées droite, bas, avant).
    pub inv: Mat3,
    pub width: u32,
    pub height: u32,
    /// Focale en pixels : `f = (w/2)/tan(hfov/2)` (hypothèse rectiligne).
    pub f_px: f32,
    /// Bande de latitude couverte par la frame (degrés), marge de roll
    /// incluse — clé du pré-filtre par ligne de sortie.
    pub lat_min: f32,
    pub lat_max: f32,
    /// Distance de fondu aux bords (px).
    pub feather_px: f32,
    /// Gain d'exposition multiplicatif par canal RGB (compensation
    /// inter-frames) ; `[1.0; 3]` = neutre. Le pipeline Legacy réplique un
    /// gain scalaire sur les 3 canaux.
    pub gain: [f32; 3],
    /// Gain **local** basse fréquence (grille 120×60 cellules de 3°,
    /// ordonnée par rangées depuis lat −90) : aplani les gradients
    /// d'illumination intra-scène (fenêtre vs mur) que les gains scalaires
    /// par frame ne peuvent pas absorber — la cause des jointures en
    /// rectangles visibles. Tout à 1.0 = neutre (Legacy). Équivalent Rust
    /// des « block gains » du GainCompensator OpenCV.
    pub local_gain: Vec<f32>,
    /// Warp local (mesh) : δ échantillonnage par cellule — `None` =
    /// géométrie pure de la pose (Legacy, tests). Rempli par
    /// `align::estimate_local_warp` (pipeline Quality).
    pub warp: Option<WarpField>,
    pub rgb: Vec<u8>,
}

/// Taille des dimensions de la grille de gain local (cellules de 3°).
pub const LOCAL_GRID_W: usize = 120;
pub const LOCAL_GRID_H: usize = 60;

/// Champ de warp LOCAL d'une frame : δlon/δlat (degrés) sur une grille de
/// contrôle couvrant lon ∈ [yaw−90, yaw+90] × lat ∈ [pitch−60, pitch+60]
/// (cellules de 7,5°). Porté par [`PreparedFrame::warp`] ; appliqué comme
/// perturbation du rayon AVANT projection ([`sample_ray`]) — le contenu de
/// la frame apparaît alors décalé de −δ (cf. `align::estimate_local_warp`
/// pour le sens et la jauge).
#[derive(Clone, Debug, Default)]
pub struct WarpField {
    /// (δlon, δlat) par cellule, rangées depuis lat+60, colonnes depuis
    /// lon yaw−90.
    pub cells: Vec<(f32, f32)>,
}

/// Cellules de la grille de warp (7,5° sur chaque axe).
pub const WARP_CELLS_LON: usize = 24;
pub const WARP_CELLS_LAT: usize = 16;

impl WarpField {
    /// Interpolation bilinéaire du champ au point (lon, lat) monde.
    /// Grille absente ou taille incohérente → (0, 0).
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

/// Index de cellule de la grille locale pour (lon, lat) en degrés.
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
    /// Re-dérive `inv` et la bande de latitude depuis la pose (yaw, pitch,
    /// roll). À appeler après toute correction de pose : le pré-filtre par
    /// ligne (`lat_min/lat_max`) et le warp (`inv`) doivent suivre, sinon
    /// les frames corrigées sont exclues à tort du rendu.
    pub fn refresh_pose(&mut self) {
        self.inv = crate::geom::euler_to_mat(self.yaw_deg, self.pitch_deg, self.roll_deg);
        let (lat_min, lat_max) = lat_band(self.height, self.f_px, self.pitch_deg);
        self.lat_min = lat_min;
        self.lat_max = lat_max;
    }
}

/// Rayon monde échantillonné pour la frame `f` au point (lon, lat) :
/// applique le warp local de la frame (δ bilinéaire, cf. [`WarpField`]) —
/// sampler en (lon+δlon, lat+δlat) fait apparaître le contenu décalé de
/// −δ. Point d'entrée UNIQUE des boucles de rendu (render, multiband,
/// seams, gains) : la géométrie reste cohérente d'un étage à l'autre.
#[must_use]
pub fn sample_ray(f: &PreparedFrame, lon: f32, lat: f32) -> [f32; 3] {
    let (dlon, dlat) = match &f.warp {
        Some(w) => w.delta(f.yaw_deg, f.pitch_deg, lon, lat),
        None => (0.0, 0.0),
    };
    crate::render::ray_of(lon + dlon, lat + dlat)
}

/// Focale en pixels pour une HFOV (degrés) et une largeur donnée.
#[must_use]
pub fn f_px_from_hfov(hfov_deg: f32, width: u32) -> f32 {
    let half = (width as f32) * 0.5;
    half / to_rad_tan(hfov_deg * 0.5)
}

/// tan(d°) — petit wrapper pour la lisibilité des formules de focale.
fn to_rad_tan(deg: f32) -> f32 {
    deg.to_radians().tan()
}

/// Bande de latitude couverte par une frame (degrés, marge incluse).
/// La vfov est déduite de l'aspect ; on ajoute une marge de 6° pour les
/// poses avec roll/du flou de pose au déclenchement.
#[must_use]
pub fn lat_band(height: u32, f_px: f32, pitch_deg: f32) -> (f32, f32) {
    let half_vfov = (height as f32 * 0.5 / f_px).atan().to_degrees();
    const MARGE_DEG: f32 = 6.0;
    (
        pitch_deg - half_vfov - MARGE_DEG,
        pitch_deg + half_vfov + MARGE_DEG,
    )
}

/// Projette un rayon monde dans la frame : `Some((u, v, c))` où `c = cos`
/// de l'angle rayon↔axe optique (centralité, dans (0,1] pour un point
/// visé), `None` hors du capteur. `ray` doit être unitaire.
#[must_use]
pub fn project_uvc(frame: &PreparedFrame, ray: [f32; 3]) -> Option<(f32, f32, f32)> {
    let [a, b, c] = frame.inv.mul_vec(ray);
    if c <= f32::EPSILON {
        return None;
    }
    let u = frame.width as f32 * 0.5 + frame.f_px * a / c;
    let v = frame.height as f32 * 0.5 + frame.f_px * b / c;
    // Tolérance d'un demi-pixel hors-bord : le sampling bilinéaire reste
    // défini et les poids de fondu s'annulent au bord.
    if u < -0.5 || v < -0.5 || u > frame.width as f32 - 0.5 || v > frame.height as f32 - 0.5 {
        return None;
    }
    // c = axe·rayon (les deux unitaires) = cos de l'angle à l'axe.
    Some((u, v, c))
}

/// Projette un rayon monde dans la frame ; `None` hors du capteur.
/// `ray` doit être unitaire. (Conserve l'API historique (u, v).)
#[must_use]
pub fn project(frame: &PreparedFrame, ray: [f32; 3]) -> Option<(f32, f32)> {
    project_uvc(frame, ray).map(|(u, v, _)| (u, v))
}

/// Poids de sélection : distance normalisée au bord le plus proche, dans
/// `[0, 1]` (1 au centre de la frame, 0 sur le pourtour). Élevée à une
/// puissance côté rendu, elle fait « gagner » la frame qui voit le pixel le
/// plus au centre → sélection nette au lieu d'un moyennage fantôme, avec un
/// fondu étroit uniquement là où deux frames se valent (la couture).
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

/// Poids de blend « tente » : plateau au centre, chute linéaire sur
/// `feather_px` vers les bords. Garantit un poids 0 sur le pourtour.
/// (Conservé pour compat ; le rendu utilise désormais [`interior_weight`].)
#[must_use]
pub fn edge_weight(frame: &PreparedFrame, u: f32, v: f32) -> f32 {
    let dist = u
        .min(v)
        .min(frame.width as f32 - u)
        .min(frame.height as f32 - v);
    if frame.feather_px < 1.0 {
        return 1.0;
    }
    (dist / frame.feather_px).clamp(0.0, 1.0)
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

    /// refresh_pose : modifier la pose puis re-dériver doit déplacer la
    /// bande de latitude et reconstruire `inv` à l'identique d'un montage
    /// direct.
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

    /// Focale : hfov complet → les bords gauche/droit à ±w/2 du centre.
    #[test]
    fn focale_hfov() {
        let f = f_px_from_hfov(90.0, 1000);
        assert!((f - 500.0).abs() < 1e-3); // tan(45°) = 1
        let fr = frame_yields(1000, 750, 90.0);
        // Rayon d'axe → centre exact.
        let (u, v) = project(&fr, [1.0, 0.0, 0.0]).unwrap();
        assert!((u - 500.0).abs() < 1e-3 && (v - 375.0).abs() < 1e-3);
        // Rayon à 44° du côté droit (lon −44° cf. repère) — PAS à 45° pile :
        // le bord exact tombe sur u = w, exclu par la tolérance demi-pixel.
        let (lon, lat) = (-44.0_f32.to_radians(), 0.0_f32);
        let ray = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        let (u, _) = project(&fr, ray).unwrap();
        let attendu = 500.0 + 500.0 * 44.0_f32.to_radians().tan();
        assert!((u - attendu).abs() < 1.0, "u = {u}, attendu {attendu}");
    }

    /// Centralité : rayon d'axe → c ≈ 1 ; rayon oblique → c plus petit.
    #[test]
    fn centralite() {
        let fr = frame_yields(1000, 750, 90.0);
        let (_, _, c0) = project_uvc(&fr, [1.0, 0.0, 0.0]).unwrap();
        assert!((c0 - 1.0).abs() < 1e-4, "c axe = {c0}");
        let (lon, lat) = (-30.0_f32.to_radians(), 0.0_f32);
        let ray = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        let (_, _, c1) = project_uvc(&fr, ray).unwrap();
        assert!(c1 < c0 && c1 > 0.5, "c oblique = {c1}");
    }

    /// interior_weight : ~1 au centre, 0 au bord.
    #[test]
    fn poids_interieur() {
        let fr = frame_yields(800, 600, 65.0);
        assert!((interior_weight(&fr, 400.0, 300.0) - 1.0).abs() < 1e-6);
        assert!(interior_weight(&fr, 0.0, 300.0) < 1e-6);
        assert!(interior_weight(&fr, 400.0, 600.0) < 1e-6);
    }

    /// Hors-champ : derrière la caméra ou au-delà du capteur → None.
    #[test]
    fn hors_champ() {
        let fr = frame_yields(1000, 750, 60.0);
        assert!(project(&fr, [-1.0, 0.0, 0.0]).is_none());
        // lon 90° est loin hors du hfov 60 → None
        assert!(project(&fr, [0.0, 1.0, 0.0]).is_none());
    }
}
