//! Sonde paire réelle : extraction+matching SuperPoint/LightGlue + rotation
//! Horn sur deux frames voisines du job réel. Compare avec la pose capteur.
//! Usage : cargo run --release -p pnex-stitcher --features pose-model \
//!   --example probe_pair -- <dir> [i] [j]
use pnex_stitcher::geom::{euler_to_mat, norm, to_deg};
use pnex_stitcher::pose_model::PoseModel;
use pnex_stitcher::project::{f_px_from_hfov, PreparedFrame};
use pnex_stitcher::rotation::horn_ransac;
use pnex_stitcher::rotation::log_so3;

fn main() {
    let dir = std::env::args().nth(1).unwrap();
    let (i, j) = (
        std::env::args()
            .nth(2)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0usize),
        std::env::args()
            .nth(3)
            .and_then(|v| v.parse().ok())
            .unwrap_or(1usize),
    );
    let model = PoseModel::load("deploy/models").expect("modèle");
    // Chargement des 2 frames
    let load = |k: usize| -> (Vec<u8>, u32, u32, f32) {
        let p = format!("{dir}/frame_{k:03}.jpg");
        let jpeg = std::fs::read(&p).unwrap_or_else(|e| panic!("{p} : {e}"));
        let (rgb, w, h) = pnex_stitcher::decode_jpeg_rgb(&jpeg).expect("décode");
        eprintln!("frame {k} : {w}x{h}");
        (rgb, w, h, 43.711803_f32)
    };
    let (ra, wa, ha, hfov) = load(i);
    let (rb, wb, hb, _) = load(j);
    println!("extraction…");
    let fa = model.extract(&ra, wa, ha, 896).expect("extract a");
    let fb = model.extract(&rb, wb, hb, 896).expect("extract b");
    println!(
        "a : {} kpts {}x{}, b : {} kpts",
        fa.kpts_px.len(),
        fa.w,
        fa.h,
        fb.kpts_px.len()
    );
    // A/B : échelle des keypoints envoyés au matcher (KPSCALE=<largeur virtuelle>).
    let kpscale: f32 = std::env::var("KPSCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let mut fa2 = fa.clone();
    let mut fb2 = fb.clone();
    for p in fa2.kpts_px.iter_mut() {
        p[0] *= kpscale / fa2.w as f32;
        p[1] *= kpscale / fa2.h as f32;
    }
    for p in fb2.kpts_px.iter_mut() {
        p[0] *= kpscale / fb2.w as f32;
        p[1] *= kpscale / fb2.h as f32;
    }
    let matches = model.match_pair(&fa2, &fb2).expect("match");
    println!(
        "matches : {} (kpscale {kpscale} → coords max ~({:.0},{:.0}))",
        matches.len(),
        kpscale,
        kpscale
    );
    // Rayons caméra
    let ray = |f: &PreparedFrame, ft: &pnex_stitcher::pose_model::Features, p: [f32; 2]| {
        let s = ft.w as f32 / f.width as f32;
        let fpx = f.f_px * s;
        let mut v = [
            (p[0] - ft.w as f32 * 0.5) / fpx,
            (p[1] - ft.h as f32 * 0.5) / fpx,
            1.0,
        ];
        let n = norm(v);
        v.iter_mut().for_each(|c| *c /= n);
        v
    };
    let mk = |rgb: Vec<u8>, w: u32, h: u32| -> PreparedFrame {
        let f_px = f_px_from_hfov(hfov, w);
        PreparedFrame {
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
            inv: euler_to_mat(0.0, 0.0, 0.0),
            width: w,
            height: h,
            f_px,
            lat_min: -45.0,
            lat_max: 45.0,
            feather_px: 1.0,
            gain: [1.0; 3],
            local_gain: Vec::new(),
            warp: None,
            rgb,
        }
    };
    let pa = mk(ra, wa, ha);
    let pb = mk(rb, wb, hb);
    let pts_a: Vec<[f32; 3]> = matches
        .iter()
        .map(|m| ray(&pa, &fa, fa.kpts_px[m.ia]))
        .collect();
    let pts_b: Vec<[f32; 3]> = matches
        .iter()
        .map(|m| ray(&pb, &fb, fb.kpts_px[m.ib]))
        .collect();
    let f8 =
        0.5 * (pa.f_px * fa.w as f32 / pa.width as f32 + pb.f_px * fb.w as f32 / pb.width as f32);
    let tol = 3.0 / f8;
    println!("tolérance = {:.4}° (f8={f8:.0})", to_deg(tol));
    let est = horn_ransac(&pts_a, &pts_b, tol, 400, 7).expect("ransac");
    println!(
        "inliers = {} err médiane = {:.4}°",
        est.inliers.len(),
        to_deg(est.median_err)
    );
    // Rotation relative attendue (capteur) : yaw_b - yaw_a = -20.17°
    println!(
        "rotation relative estimée : axe-angle = {:?} rad",
        log_so3(est.rel)
    );
    // axe-angle → décomposition yaw/pitch/roll approx : angle total
    let ang = to_deg(norm(log_so3(est.rel)));
    println!("angle total = {ang:.2}° (capteur attendu ≈ 20,17°)");
}
