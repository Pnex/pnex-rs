//! Audit de cohérence pose ↔ contenu des frames d'un job réel.
//!
//! Pour chaque paire candidate (même pré-filtre géométrique que
//! `build_pairs`), match SuperPoint+LightGlue + rotation Horn/RANSAC, puis
//! compare l'angle de la rotation relative mesurée à l'angle de la rotation
//! relative capteur. Une frame dont les paires contredisent massivement la
//! pose enregistrée = pose aberrante : l'aligneur ne peut ni la raccorder
//! ni la détecter, et elle pollue le rendu à sa pose fausse.
//!
//! Usage : cargo run --release -p pnex-stitcher --features pose-model \
//!   --example audit_poses -- <dir>
//! Env : MODEL=<dir modèles> (défaut deploy/models)

use pnex_stitcher::geom::{euler_to_mat, norm, to_deg};
use pnex_stitcher::pose_model::PoseModel;
use pnex_stitcher::project::{f_px_from_hfov, lat_band, PreparedFrame};
use pnex_stitcher::rotation::{horn_ransac, log_so3};

#[derive(serde::Deserialize)]
struct Pose {
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let hfov: f32 = std::env::args()
        .nth(2)
        .map(|v| v.parse().expect("hfov"))
        .unwrap_or(43.711803);

    let model = PoseModel::load(
        std::env::var("MODEL")
            .unwrap_or_else(|_| "deploy/models".into())
            .as_str(),
    )
    .expect("modèle");

    let poses: Vec<Pose> =
        serde_json::from_slice(&std::fs::read(format!("{dir}/poses.json")).expect("poses.json"))
            .expect("poses.json invalide");
    let n = poses.len();

    // Chargement + préparation (même logique que le stitcher, gains 1.0).
    let frames: Vec<PreparedFrame> = (0..n)
        .map(|k| {
            let path = format!("{dir}/frame_{k:03}.jpg");
            let jpeg = std::fs::read(&path).unwrap_or_else(|e| panic!("{path} : {e}"));
            let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&jpeg).expect("décode");
            let p = &poses[k];
            let f_px = f_px_from_hfov(hfov, w);
            let (lat_min, lat_max) = lat_band(h, f_px, p.pitch_deg);
            PreparedFrame {
                yaw_deg: p.yaw_deg,
                pitch_deg: p.pitch_deg,
                roll_deg: p.roll_deg,
                inv: euler_to_mat(p.yaw_deg, p.pitch_deg, p.roll_deg),
                width: w,
                height: h,
                f_px,
                lat_min,
                lat_max,
                feather_px: 1.0,
                gain: [1.0; 3],
                local_gain: Vec::new(),
                warp: None,
                rgb,
            }
        })
        .collect();

    // Paires candidates : même pré-filtre que build_pairs (recouvrement lat
    // ≥ 5°, |Δlon| ≤ hfov approx, wrap géré).
    let mut pairs = Vec::new();
    for a in 0..n {
        for b in (a + 1)..n {
            let (fa, fb) = (&frames[a], &frames[b]);
            let overlap = fa.lat_max.min(fb.lat_max) - fa.lat_min.max(fb.lat_min);
            if overlap < 5.0 {
                continue;
            }
            let dlon = wrap180(fa.yaw_deg - fb.yaw_deg).abs();
            if dlon > hfov {
                continue;
            }
            pairs.push((a, b));
        }
    }
    println!("{n} frames, {} paires candidates", pairs.len());

    // Match d'une paire : extraction + match + Horn/RANSAC → (inliers, angle°).
    let match_pair = |a: usize, b: usize| -> Option<(usize, f32)> {
        let (fa, fb) = (&frames[a], &frames[b]);
        let fa_t = model.extract(&fa.rgb, fa.width, fa.height, 896).ok()?;
        let fb_t = model.extract(&fb.rgb, fb.width, fb.height, 896).ok()?;
        let matches = model.match_pair(&fa_t, &fb_t).ok()?;
        if matches.len() < 15 {
            return None;
        }
        let ray = |f: &PreparedFrame, ft: &pnex_stitcher::pose_model::Features, p: [f32; 2]| {
            let s = ft.w as f32 / f.width as f32;
            let fpx = f.f_px * s;
            let mut v = [
                (p[0] - ft.w as f32 * 0.5) / fpx,
                (p[1] - ft.h as f32 * 0.5) / fpx,
                1.0,
            ];
            let nn = norm(v);
            v.iter_mut().for_each(|c| *c /= nn);
            v
        };
        let pts_a: Vec<[f32; 3]> = matches
            .iter()
            .map(|m| ray(fa, &fa_t, fa_t.kpts_px[m.ia]))
            .collect();
        let pts_b: Vec<[f32; 3]> = matches
            .iter()
            .map(|m| ray(fb, &fb_t, fb_t.kpts_px[m.ib]))
            .collect();
        let s_a = fa_t.w as f32 / fa.width as f32;
        let s_b = fb_t.w as f32 / fb.width as f32;
        let f8 = 0.5 * (fa.f_px * s_a + fb.f_px * s_b);
        let tol = 3.0 / f8;
        let seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((a as u64) << 32) ^ b as u64;
        let est = horn_ransac(&pts_a, &pts_b, tol, 400, seed)?;
        if est.inliers.len() < 15 || est.median_err > tol {
            return None;
        }
        let angle = to_deg(norm(log_so3(est.rel)));
        Some((est.inliers.len(), angle))
    };

    // Match de chaque paire candidate, synthèse par frame.
    let mut results: Vec<(usize, usize, usize, f32, f32)> = Vec::new();
    for &(a, b) in &pairs {
        let sensor_rel = frames[b].inv.mul_mat(&frames[a].inv.transpose());
        let sensor_angle = to_deg(norm(log_so3(sensor_rel)));
        if let Some((inl, meas_angle)) = match_pair(a, b) {
            results.push((a, b, inl, meas_angle, sensor_angle));
        }
    }

    let mut per_frame: Vec<Vec<(usize, usize, f32, f32)>> = vec![Vec::new(); n];
    for &(a, b, inl, meas, sens) in &results {
        per_frame[a].push((b, inl, meas, sens));
        per_frame[b].push((a, inl, meas, sens));
    }

    println!("\n=== Par frame : paires validées, Δangle max (mesuré vs capteur), contradictions (> 5°) ===");
    for k in 0..n {
        let cand = per_frame[k].len();
        let (yaw, pitch) = (poses[k].yaw_deg, poses[k].pitch_deg);
        if cand == 0 {
            println!("frame {k:02} (yaw {yaw:7.2} pitch {pitch:6.2}) : 0 paire validée");
            continue;
        }
        let mut dmax = 0.0f32;
        for &(_, _, meas, sens) in &per_frame[k] {
            dmax = dmax.max((meas - sens).abs());
        }
        let n_contra = per_frame[k]
            .iter()
            .filter(|&&(_, _, m, s)| (m - s).abs() > 5.0)
            .count();
        println!(
            "frame {k:02} (yaw {yaw:7.2} pitch {pitch:6.2}) : {}/{} paires, Δangle max {dmax:5.2}°, contradictions {n_contra}",
            per_frame[k].len(),
            cand
        );
    }

    println!("\n=== Paires contradictoires (Δangle > 5°) ===");
    for &(a, b, inl, meas, sens) in &results {
        if (meas - sens).abs() > 5.0 {
            println!("({a:02},{b:02}) inliers {inl:3}  mesuré {meas:6.2}° vs capteur {sens:6.2}°");
        }
    }
}

fn wrap180(deg: f32) -> f32 {
    let d = deg.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}
