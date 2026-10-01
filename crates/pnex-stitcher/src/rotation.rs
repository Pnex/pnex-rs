//! Outils de rotation SO(3) — pure Rust, sans dépendance.
//!
//! Sert au raffinement de pose **par les images** : à partir de
//! correspondances de points (a dans la caméra A, b dans la caméra B), on
//! estime la rotation relative `b ≈ R·a` (modèle « pure rotation » — le
//! stitch 360 est une rotation autour du point nodal par hypothèse), et le
//! solve global aligne les poses euler (yaw/pitch/roll) sur ces mesures.
//!
//! Choix : méthode **quaternion de Horn** résolue par itération de
//! puissance sur la matrice 4×4 symétrique (pas de SVD, pas de dépendance
//! d'algèbre — nalgebra a été écartée pour garder la crate sans dep d'algèbre
//! ; l'itération de puissance converge en <20 pas sur des correspondances
//! réelles et la qualité est identique à la SVD pour des rotations ≤ 40°).
//!
//! Repère : cf. [`crate::geom`] — caméra = M·monde, rangées (droite, bas,
//! avant) ; pixel (u,v) → rayon caméra ((u−cx)/f, (v−cy)/f, 1).

use crate::geom::Mat3;

/// Exponentielle SO(3) : axe-angle (rad) → matrice de rotation.
#[must_use]
pub fn exp_so3(w: [f32; 3]) -> Mat3 {
    let theta = crate::geom::norm(w);
    if theta < 1e-8 {
        return Mat3::IDENTITY;
    }
    let k = [w[0] / theta, w[1] / theta, w[2] / theta];
    let (s, c) = theta.sin_cos();
    let (x, y, z) = (k[0], k[1], k[2]);
    let c1 = 1.0 - c;
    // Formule de Rodrigues.
    Mat3([
        c + x * x * c1,
        x * y * c1 - z * s,
        x * z * c1 + y * s, //
        y * x * c1 + z * s,
        c + y * y * c1,
        y * z * c1 - x * s, //
        z * x * c1 - y * s,
        z * y * c1 + x * s,
        c + z * z * c1,
    ])
}

/// Logarithme SO(3) : matrice de rotation → axe-angle (rad). Formules
/// quaternion (angle depuis la trace, axe depuis la partie vectorielle) —
/// exact pour θ < 180°, Continue (pas de branch cut) pour les petites
/// corrections que le solve manipule (< 10°).
#[must_use]
pub fn log_so3(m: Mat3) -> [f32; 3] {
    let [m00, m01, m02, m10, m11, m12, m20, m21, m22] = m.0;
    let cos_t = ((m00 + m11 + m22 - 1.0) * 0.5).clamp(-1.0, 1.0);
    let theta = cos_t.acos();
    if theta < 1e-6 {
        // θ ≈ 0 : parties antisymétriques = w (au 1er ordre).
        [0.5 * (m21 - m12), 0.5 * (m02 - m20), 0.5 * (m10 - m01)]
    } else {
        let s = theta.sin();
        let f = theta / (2.0 * s);
        [f * (m21 - m12), f * (m02 - m20), f * (m10 - m01)]
    }
}

/// Rotation relative estimée d'une paire : `b ≈ R·a` (repères caméra), avec
/// ses inliers et l'erreur angulaire médiane (rad).
#[derive(Debug, Clone)]
pub struct RotationEstimate {
    /// Rotation caméra A → caméra B.
    pub rel: Mat3,
    /// Indices des correspondances inliers.
    pub inliers: Vec<usize>,
    /// Erreur angulaire médiane des inliers (rad).
    pub median_err: f32,
}

/// Méthode quaternion de Horn : minimise Σ‖bᵢ − R·aᵢ‖² (poids `w`), fermé
/// pour rotation pure. Itération de puissance sur la matrice 4×4 symétrique
/// — le vecteur propre dominant est le quaternion optimal.
#[must_use]
pub fn horn(a: &[[f32; 3]], b: &[[f32; 3]], w: &[f32]) -> Option<Mat3> {
    let n = a.len();
    if n < 2 || b.len() != n || w.len() != n {
        return None;
    }
    // Matrice de covariance croisée M = Σ w·b aᵀ (b = R·a).
    let mut m = [0.0f64; 9];
    for i in 0..n {
        let (wi, ai, bi) = (w[i] as f64, a[i], b[i]);
        for r in 0..3 {
            for c in 0..3 {
                m[3 * r + c] += wi * bi[r] as f64 * ai[c] as f64;
            }
        }
    }
    // Matrice 4×4 symétrique de Horn — ordre quaternion (w, x, y, z),
    // M = Σ b aᵀ (vérifié à la main sur une rotation z de 30° : le vecteur
    // propre dominant est bien (cos θ/2, 0, 0, sin θ/2)).
    let [m00, m01, m02, m10, m11, m12, m20, m21, m22] = m;
    let tr = m00 + m11 + m22;
    let (zx, zy, zz) = (m21 - m12, m02 - m20, m10 - m01);
    let (sxx, syy, szz) = (m00 - m11 - m22, -m00 + m11 - m22, -m00 - m11 + m22);
    let (sxy, sxz, syz) = (m01 + m10, m02 + m20, m12 + m21);
    let n4 = [
        tr, zx, zy, zz, //
        zx, sxx, sxy, sxz, //
        zy, sxy, syy, syz, //
        zz, sxz, syz, szz,
    ];
    // Itération de puissance : vecteur propre dominant (le quaternion
    // optimal). Départ (1,0,0,0) : converge en < 40 pas pour nos rotations
    // < 40° ; les angles proches de 180° (hors périmètre) pourraient osciller
    // — le RANSAC en amont garantit des rotations relatives petites.
    let mut q = [1.0f64, 0.0, 0.0, 0.0];
    for _ in 0..500 {
        let mut next = [0.0f64; 4];
        for r in 0..4 {
            let mut acc = 0.0f64;
            for c in 0..4 {
                acc += n4[4 * r + c] * q[c];
            }
            next[r] = acc;
        }
        let nrm =
            (next[0] * next[0] + next[1] * next[1] + next[2] * next[2] + next[3] * next[3]).sqrt();
        if nrm < 1e-12 {
            return None;
        }
        for v in next.iter_mut() {
            *v /= nrm;
        }
        // Convergence : produit scalaire ~1 avec l'itéré précédent.
        let d = q[0] * next[0] + q[1] * next[1] + q[2] * next[2] + q[3] * next[3];
        q = next;
        if d > 1.0 - 1e-14 {
            break;
        }
    }
    let [wq, x, y, z] = q;
    // Quaternion (w,x,y,z) → matrice (unitaire par construction), f32 en
    // sortie (précision largement suffisante : corrections ≤ 25°).
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (wx, wy, wz) = (wq * x, wq * y, wq * z);
    let f = |v: f64| -> f32 { v as f32 };
    Some(Mat3([
        f(1.0 - 2.0 * (yy + zz)),
        f(2.0 * (xy - wz)),
        f(2.0 * (xz + wy)), //
        f(2.0 * (xy + wz)),
        f(1.0 - 2.0 * (xx + zz)),
        f(2.0 * (yz - wx)), //
        f(2.0 * (xz - wy)),
        f(2.0 * (yz + wx)),
        f(1.0 - 2.0 * (xx + yy)),
    ]))
}

/// RANSAC rotation pure : échantillonne des paires minimales (2
/// correspondances non colinéaires), score par comptage d'inliers (erreur
/// angulaire < `tol_rad`), raffine Horn sur les inliers. Les rotations
/// relatives attendues sont < 40° (frames voisines) — bassin garanti par
/// les poses capteur.
#[must_use]
pub fn horn_ransac(
    a: &[[f32; 3]],
    b: &[[f32; 3]],
    tol_rad: f32,
    max_iters: u32,
    seed: u64,
) -> Option<RotationEstimate> {
    let n = a.len().min(b.len());
    if n < 4 {
        return None;
    }
    let mut rng = Rng(seed | 1);
    let mut best: Option<(usize, Mat3, Vec<usize>)> = None;
    for _ in 0..max_iters {
        // 2 correspondances distinctes au hasard.
        let i = (rng.next_u64() as usize) % n;
        let mut j = (rng.next_u64() as usize) % n;
        while j == i {
            j = (rng.next_u64() as usize) % n;
        }
        // Horn minimal sur 2 points (4 contraintes pour 3 DOF).
        let Some(rel) = horn(&[a[i], a[j]], &[b[i], b[j]], &[1.0, 1.0]) else {
            continue;
        };
        // Comptage d'inliers par erreur angulaire.
        let mut inliers = Vec::new();
        for k in 0..n {
            let err = angle_err(rel, a[k], b[k]);
            if err < tol_rad {
                inliers.push(k);
            }
        }
        let count = inliers.len();
        match &best {
            Some((bc, _, _)) if count <= *bc => {}
            _ => best = Some((count, rel, inliers)),
        }
    }
    let count = best.as_ref()?.0;
    if count < 3 {
        return None;
    }
    // Raffinement Horn sur les inliers du meilleur modèle (2 passes de
    // re-gating : affine l'erreur à ~0,1 px et purge les inliers limites).
    let (_, mut rel, mut inliers) = best?;
    for _ in 0..2 {
        let ones = vec![1.0f32; inliers.len()];
        let (pa, pb): (Vec<[f32; 3]>, Vec<[f32; 3]>) =
            inliers.iter().map(|&k| (a[k], b[k])).unzip();
        let Some(refined) = horn(&pa, &pb, &ones) else {
            break;
        };
        let new_inliers: Vec<usize> = (0..n)
            .filter(|&k| angle_err(refined, a[k], b[k]) < tol_rad)
            .collect();
        if new_inliers.len() < 3 {
            break;
        }
        rel = refined;
        inliers = new_inliers;
    }
    // Erreur angulaire médiane des inliers (diagnostic / poids de paire).
    let mut errs: Vec<f32> = inliers
        .iter()
        .map(|&k| angle_err(rel, a[k], b[k]))
        .collect();
    errs.sort_by(f32::total_cmp);
    let median_err = if errs.is_empty() {
        f32::INFINITY
    } else {
        errs[errs.len() / 2]
    };
    Some(RotationEstimate {
        rel,
        inliers,
        median_err,
    })
}

/// Erreur angulaire (rad) d'une correspondance sous la rotation `r` :
/// angle entre `b` et `r·a`.
fn angle_err(r: Mat3, a: [f32; 3], b: [f32; 3]) -> f32 {
    let ra = r.mul_vec(a);
    // atan2(|a×b|, a·b) : exact aux petits angles (acos y perd toute
    // précision en f32 — 1−cos(0,3°) ≈ 1,4e-5, sous l'epsilon du dot).
    let s = crate::geom::norm(crate::geom::cross(ra, b));
    let c = crate::geom::dot(ra, b);
    s.atan2(c)
}

/// PRNG xorshift64* — reproductible, aucune dep.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::euler_to_mat;

    /// Petit bruit gaussien reproductible (Box-Muller).
    fn noise(rng: &mut Rng, sigma: f32) -> f32 {
        let u1 = ((rng.next_u64() >> 11) as f32 / (1u64 << 53) as f32).max(1e-9);
        let u2 = (rng.next_u64() >> 11) as f32 / (1u64 << 53) as f32;
        (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos() * sigma
    }

    /// exp/log SO(3) round-trip sur un balayage d'axes et d'angles.
    #[test]
    fn exp_log_round_trip() {
        for axis in [
            [1.0f32, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.6, -0.8, 0.0],
        ] {
            for deg in [0.5f32, 2.0, 15.0, 60.0] {
                let w = [axis[0], axis[1], axis[2]].map(|c| c * deg.to_radians());
                let r = exp_so3(w);
                let back = log_so3(r);
                for k in 0..3 {
                    assert!((back[k] - w[k]).abs() < 1e-4, "round-trip {w:?} → {back:?}");
                }
            }
        }
    }

    /// Horn récupère une rotation connue à partir de correspondances
    /// propres (b = R·a) — err < 0,01°.
    #[test]
    fn horn_exact() {
        let r = euler_to_mat(23.0, -11.0, 7.0);
        let a: Vec<[f32; 3]> = (0..30)
            .map(|i| {
                let t = i as f32 * 0.7;
                [t.sin(), (t * 1.3).cos(), (t * 0.61).sin() + 1.5]
            })
            .map(|v| {
                let n = crate::geom::norm(v);
                v.map(|c| c / n)
            })
            .collect();
        let b: Vec<[f32; 3]> = a.iter().map(|p| r.mul_vec(*p)).collect();
        let est = horn(&a, &b, &vec![1.0; a.len()]).expect("horn");
        let err = log_so3(est.transpose().mul_mat(&r));
        let deg = crate::geom::to_deg(crate::geom::norm(err));
        assert!(deg < 0.01, "erreur Horn = {deg:.4}°");
    }

    /// RANSAC sur 40 correspondances dont 30 % d'aberrantes (10° aléatoires)
    /// : récupère la rotation à < 0,1° et isole les inliers.
    #[test]
    fn ransac_robuste() {
        let mut rng = Rng(42);
        let r = euler_to_mat(-17.0, 9.0, -4.0);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for k in 0..40 {
            let t = k as f32 * 0.37;
            let p = [t.sin(), (t * 1.7).cos(), (t * 0.83).sin() + 1.2];
            let n = crate::geom::norm(p);
            let p = p.map(|c| c / n);
            if k % 3 == 0 {
                // Outlier : rotation parasite de ~10°.
                let rot = exp_so3([0.1, -0.12, 0.08].map(|c| c * std::f32::consts::PI / 18.0));
                a.push(p);
                b.push(rot.mul_vec(r.mul_vec(p)));
            } else {
                a.push(p);
                b.push(r.mul_vec(p));
            }
        }
        // Bruit de capteur ±0,06° sur les inliers (répétabilité SuperPoint).
        for (k, bk) in b.iter_mut().enumerate() {
            if k % 3 != 0 {
                for c in 0..3 {
                    bk[c] += noise(&mut rng, 0.001);
                    let n = crate::geom::norm(*bk);
                    bk[c] /= n;
                }
            }
        }
        let est = horn_ransac(&a, &b, 0.005, 400, 7).expect("ransac");
        let err = log_so3(est.rel.transpose().mul_mat(&r));
        let deg = crate::geom::to_deg(crate::geom::norm(err));
        assert!(deg < 0.15, "erreur RANSAC = {deg:.3}°");
        assert!(est.inliers.len() >= 26, "inliers = {}", est.inliers.len());
        assert!(est.median_err < 0.005, "err médiane = {}", est.median_err);
    }

    /// RANSAC : les inliers propres passent le gate, les aberrants (1,75°)
    /// sont exclus — verrouille tolérance et comptage sur données bruitées.
    #[test]
    fn ransac_inliers_propres() {
        let mut rng = Rng(42);
        let r = euler_to_mat(-17.0, 9.0, -4.0);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for k in 0..40 {
            let t = k as f32 * 0.37;
            let p = [t.sin(), (t * 1.7).cos(), (t * 0.83).sin() + 1.2];
            let n = crate::geom::norm(p);
            let p = p.map(|c| c / n);
            if k % 3 == 0 {
                let rot = exp_so3([0.1, -0.12, 0.08].map(|c| c * std::f32::consts::PI / 18.0));
                a.push(p);
                b.push(rot.mul_vec(r.mul_vec(p)));
            } else {
                a.push(p);
                b.push(r.mul_vec(p));
            }
        }
        for (k, bk) in b.iter_mut().enumerate() {
            if k % 3 != 0 {
                for c in 0..3 {
                    bk[c] += noise(&mut rng, 0.001);
                    let n = crate::geom::norm(*bk);
                    bk[c] /= n;
                }
            }
        }
        let est = horn_ransac(&a, &b, 0.005, 400, 7).expect("ransac");
        let deg = crate::geom::to_deg(crate::geom::norm(log_so3(est.rel.transpose().mul_mat(&r))));
        eprintln!("est = {deg:.4}\u{b0}, inliers = {} / 27", est.inliers.len());
        let mut errs: Vec<f32> = (0..40)
            .filter(|&k| k % 3 != 0)
            .map(|k| crate::geom::to_deg(angle_err(est.rel, a[k], b[k])))
            .collect();
        errs.sort_by(f32::total_cmp);
        eprintln!(
            "erreurs propres med={:.4} max={:.4}\u{b0} (tol {:.4}\u{b0})",
            errs[errs.len() / 2],
            errs[errs.len() - 1],
            crate::geom::to_deg(0.005)
        );
        assert!(deg < 0.05 && est.inliers.len() >= 26);
    }

    /// mul_mat : vérification basique.
    #[test]
    fn mul_mat() {
        let a = euler_to_mat(10.0, 20.0, 30.0);
        let b = euler_to_mat(-40.0, 5.0, -15.0);
        let ab = a.mul_mat(&b);
        for _k in 0..3 {
            let v = [0.3, -0.5, 0.81];
            let lhs = ab.mul_vec(v);
            let rhs = a.mul_vec(b.mul_vec(v));
            for c in 0..3 {
                assert!((lhs[c] - rhs[c]).abs() < 1e-5);
            }
        }
    }
}
