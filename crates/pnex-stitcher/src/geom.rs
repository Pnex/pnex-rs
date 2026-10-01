//! Géométrie du stitcher — repère unique, algébriquement verrouillé.
//!
//! ## Repère
//!
//! Monde : orthonormé direct, Z vertical vers le haut. Un rayon de la sphère
//! depuis `(lon, lat)` en degrés :
//!
//! ```text
//! ray = (cos lat·cos lon, cos lat·sin lon, sin lat)
//! ```
//!
//! Caméra : `v_cam = M · v_world`, `M` row-major de rangées
//! *(droite, bas, avant)* — l'axe optique est la 3ᵉ rangée, un point visé a
//! `c = F·v > 0`, un pixel s'écrit `u = cx + f·a/c` (droite positive), `v =
//! cy + f·b/c` (bas positif).
//!
//! Depuis `(yaw, pitch, roll)` :
//! - `F = (cp·cy, cp·sy, sp)` (azimuth = yaw, élévation = pitch)
//! - `R = (sy, −cy, 0)` (droite horizontale = F×Z)
//! - `D = F×R = (sp·cy, sp·sy, −cp)` (composante Z négative ⇒ « bas »)
//! - roll ρ autour de F : `R' = R·cosρ + D·sinρ`, `D' = D·cosρ − R·sinρ`
//!
//! `euler_to_mat` et [`quat_to_euler`] s'inversent exactement : `quat_to_euler`
//! construit la matrice device→monde du rotation vector Android
//! (équivalent `getRotationMatrixFromVector`), la transpose, puis extrait
//! (yaw, pitch, roll) par les mêmes rangées. Les signes référentiel Android
//! (yaw croissant à gauche ou à droite, pitch device vs pitch monde…) ne
//! vivent PAS ici — conversion côté front (`capture360::sensors`), derrière
//! des constantes isolées calibrées au smoke test device.

/// Matrice 3×3 row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3(pub [f32; 9]);

impl Mat3 {
    /// Matrice identité.
    pub const IDENTITY: Mat3 = Mat3([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    /// Produit matrice-vecteur.
    pub fn mul_vec(&self, v: [f32; 3]) -> [f32; 3] {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        [
            a * v[0] + b * v[1] + c * v[2],
            d * v[0] + e * v[1] + f * v[2],
            g * v[0] + h * v[1] + i * v[2],
        ]
    }

    /// Transposée — colonnes ⇄ rangées.
    pub fn transpose(&self) -> Mat3 {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        Mat3([a, d, g, b, e, h, c, f, i])
    }

    /// Produit matrice × matrice.
    #[must_use]
    pub fn mul_mat(&self, rhs: &Mat3) -> Mat3 {
        let a = &self.0;
        let b = &rhs.0;
        let mut out = [0.0f32; 9];
        for r in 0..3 {
            for c in 0..3 {
                out[3 * r + c] =
                    a[3 * r] * b[c] + a[3 * r + 1] * b[3 + c] + a[3 * r + 2] * b[6 + c];
            }
        }
        Mat3(out)
    }
}

/// Produit vectoriel.
pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Produit scalaire.
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Norme euclidienne.
pub fn norm(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

/// (yaw, pitch, roll) en degrés → matrice monde→caméra `M` (rangées
/// *droite, bas, avant*), cf. docs du module.
#[must_use]
pub fn euler_to_mat(yaw: f32, pitch: f32, roll: f32) -> Mat3 {
    let (p, y, r) = (to_rad(pitch), to_rad(yaw), to_rad(roll));
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();

    let forward = [cp * cy, cp * sy, sp];
    let r_base = [sy, -cy, 0.0];
    let d_base = cross(forward, r_base);

    // Roll ρ autour de l'axe optique : R' = R·cosρ + D·sinρ, D' = F×R'.
    let right = [
        r_base[0] * cr + d_base[0] * sr,
        r_base[1] * cr + d_base[1] * sr,
        r_base[2] * cr + d_base[2] * sr,
    ];
    let down = [
        d_base[0] * cr - r_base[0] * sr,
        d_base[1] * cr - r_base[1] * sr,
        d_base[2] * cr - r_base[2] * sr,
    ];

    Mat3([
        right[0], right[1], right[2], //
        down[0], down[1], down[2], //
        forward[0], forward[1], forward[2],
    ])
}

/// Quaternion (x, y, z, w) → matrice de rotation R (device→monde,
/// row-major — convention rotation vector Android,
/// `getRotationMatrixFromVector`). Normalise le quaternion (les capteurs
/// livrent parfois une norme ≠ 1).
#[must_use]
pub fn quat_to_mat(q: [f32; 4]) -> Mat3 {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n < 1e-6 {
        return Mat3::IDENTITY;
    }
    let [x, y, z, w] = [q[0] / n, q[1] / n, q[2] / n, q[3] / n];

    Mat3([
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y - z * w),
        2.0 * (x * z + y * w),
        2.0 * (x * y + z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z - x * w),
        2.0 * (x * z - y * w),
        2.0 * (y * z + x * w),
        1.0 - 2.0 * (x * x + y * y),
    ])
}

/// (yaw, pitch, roll) en degrés extraits d'une matrice monde→caméra `M`
/// (rangées *droite, bas, avant*) — exactement l'inverse de
/// [`euler_to_mat`] : yaw = atan2(F_y, F_x), pitch = asin(F_z), roll via
/// la base sans roll reconstruite depuis (yaw, pitch).
#[must_use]
pub fn euler_from_mat(m: Mat3) -> (f32, f32, f32) {
    let right = [m.0[0], m.0[1], m.0[2]];
    let forward = [m.0[6], m.0[7], m.0[8]];

    let yaw = f32::atan2(forward[1], forward[0]);
    let pitch = f32::asin(forward[2].clamp(-1.0, 1.0));

    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let r_base = [sy, -cy, 0.0];
    let d_base = cross([cp * cy, cp * sy, sp], r_base);

    let roll = f32::atan2(dot(right, d_base), dot(right, r_base));
    (to_deg(yaw), to_deg(pitch), to_deg(roll))
}

/// Quaternion (x, y, z, w) → (yaw, pitch, roll) en degrés — pour un
/// rotation vector Android interprété SANS retournement de caméra
/// (hypothèse : axe +Z device = axe optique). Pour la caméra ARRIÈRE
/// (axe optique = −Z device), passer par [`quat_to_mat`] + le
/// retournement documenté dans `capture360/sensors.rs`.
#[must_use]
pub fn quat_to_euler(q: [f32; 4]) -> (f32, f32, f32) {
    euler_from_mat(quat_to_mat(q).transpose())
}

/// Radians → degrés.
#[must_use]
pub fn to_deg(rad: f32) -> f32 {
    rad * (180.0 / std::f32::consts::PI)
}

/// Degrés → radians.
#[must_use]
pub fn to_rad(deg: f32) -> f32 {
    deg * (std::f32::consts::PI / 180.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pose identité : regard +X, droite = −Y, bas = −Z (cf. docs module).
    #[test]
    fn pose_identite() {
        let m = euler_to_mat(0.0, 0.0, 0.0);
        assert_eq!(
            m,
            Mat3([
                0.0, -1.0, 0.0, //
                0.0, 0.0, -1.0, //
                1.0, 0.0, 0.0,
            ])
        );
    }

    /// Cohérence euler ⇄ quaternion : `euler_to_mat(quat_to_euler(q))` doit
    /// reconstruire la matrice monde→caméra associée au quaternion, et le
    /// quaternion identité correspond à la caméra regardant +Z (pitch 90°).
    #[test]
    fn coherence_euler_quaternion() {
        // Quaternion identité : R = I ⇒ M = Rᵀ = I ⇒ caméra alignée sur le
        // monde : regard +Z, droite +X, bas +Y.
        let (y, p, r) = quat_to_euler([0.0, 0.0, 0.0, 1.0]);
        let reconstruite = euler_to_mat(y, p, r);
        for (a, b) in reconstruite.0.iter().zip(Mat3::IDENTITY.0.iter()) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
        assert!((p - 90.0).abs() < 1e-4, "pitch = {p}, attendu 90");
    }

    /// Round-trip euler → quaternion → euler sur une grille de poses.
    #[test]
    fn aller_retour_euler() {
        for yaw in [-170.0_f32, -45.0, 0.0, 30.0, 120.0, 179.0] {
            for pitch in [-80.0_f32, -30.0, 0.0, 15.0, 60.0] {
                for roll in [-90.0_f32, -20.0, 0.0, 10.0, 85.0] {
                    let attendu = euler_to_mat(yaw, pitch, roll);
                    let (y2, p2, r2) = quat_to_euler(mat_vers_quat(attendu.transpose()));
                    assert!(
                        (yaw - y2).abs() < 1e-3
                            && (pitch - p2).abs() < 1e-3
                            && (roll - r2).abs() < 1e-3,
                        "round-trip {yaw}/{pitch}/{roll} → {y2}/{p2}/{r2}"
                    );
                    // Et la matrice reconstruite colle (dérive f32 du
                    // passage par le quaternion tolérée).
                    let reconstruite = euler_to_mat(y2, p2, r2);
                    for (a, b) in reconstruite.0.iter().zip(attendu.0.iter()) {
                        assert!((a - b).abs() < 1e-4, "{a} vs {b}");
                    }
                }
            }
        }
    }

    /// Quaternion unitaire d'une matrice de rotation R (device→monde) —
    /// outillage du round-trip : M = Rᵀ ⇒ on passe R = Mᵀ.
    fn mat_vers_quat(m: Mat3) -> [f32; 4] {
        let [a, b, c, d, e, f, g, h, i] = m.0;
        let tr = a + e + i;
        let (x, y, z, w);
        if tr > 0.0 {
            let s = (tr + 1.0).sqrt() * 2.0;
            w = 0.25 * s;
            x = (h - f) / s;
            y = (c - g) / s;
            z = (d - b) / s;
        } else if a > e && a > i {
            let s = (1.0 + a - e - i).sqrt() * 2.0;
            w = (h - f) / s;
            x = 0.25 * s;
            y = (b + d) / s;
            z = (c + g) / s;
        } else if e > i {
            let s = (1.0 + e - a - i).sqrt() * 2.0;
            w = (c - g) / s;
            x = (b + d) / s;
            y = 0.25 * s;
            z = (f + h) / s;
        } else {
            let s = (1.0 + i - a - e).sqrt() * 2.0;
            w = (d - b) / s;
            x = (c + g) / s;
            y = (f + h) / s;
            z = 0.25 * s;
        }
        [x, y, z, w]
    }

    /// Valeur connue : la caméra à yaw 90° voit le rayon lon 90° au centre
    /// de son capteur.
    #[test]
    fn valeur_connue_yaw_90() {
        let m = euler_to_mat(90.0, 0.0, 0.0);
        let (lon, lat) = (90.0_f32.to_radians(), 0.0_f32);
        let ray = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        let v = m.mul_vec(ray);
        assert!(v[0].abs() < 1e-5 && v[1].abs() < 1e-5, "v = {v:?}");
        assert!((v[2] - 1.0).abs() < 1e-5, "axe optique → c = {0}", v[2]);
    }
}
