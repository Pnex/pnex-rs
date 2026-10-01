//! Re-stitch hôte d'une capture réelle (Phase D du plan Take 360 V2) :
//! lit `frame_000.jpg … frame_NNN.jpg` + `poses.json` + `capture.json`
//! (hfov_deg, out_width, pipeline) d'un répertoire, écrit `panorama.jpg`.
//!
//! Usage : cargo run --release -p pnex-stitcher --example restitch -- <dir>
//! Permet d'itérer la qualité du pipeline sur des captures réelles sans
//! roundtrip téléphone.

use pnex_stitcher::{Frame, Params, Pipeline, QualityParams};

#[derive(serde::Deserialize)]
struct Pose {
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
}

#[derive(serde::Deserialize)]
struct Capture {
    hfov_deg: f32,
    #[serde(default = "default_out")]
    out_width: u32,
}

fn default_out() -> u32 {
    4096
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: restitch <dir>");
    let capture: Capture = serde_json::from_slice(
        &std::fs::read(format!("{dir}/capture.json")).expect("capture.json"),
    )
    .expect("capture.json invalide");
    let poses: Vec<Pose> =
        serde_json::from_slice(&std::fs::read(format!("{dir}/poses.json")).expect("poses.json"))
            .expect("poses.json invalide");

    let mut frames: Vec<Frame> = Vec::with_capacity(poses.len());
    for (k, pose) in poses.iter().enumerate() {
        let path = format!("{dir}/frame_{k:03}.jpg");
        let jpeg = std::fs::read(&path).unwrap_or_else(|e| panic!("{path} : {e}"));
        let (rgb, width, height) = pnex_stitcher::decode_jpeg_rgb(&jpeg).expect("décodage");
        frames.push(Frame {
            rgb,
            width,
            height,
            yaw_deg: pose.yaw_deg,
            pitch_deg: pose.pitch_deg,
            roll_deg: pose.roll_deg,
        });
    }
    println!(
        "{} frames ({}×{}), hfov {:.1}°, sortie {}",
        frames.len(),
        frames[0].width,
        frames[0].height,
        capture.hfov_deg,
        capture.out_width
    );

    let params = Params {
        hfov_deg: capture.hfov_deg,
        out_width: std::env::var("OUTW")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(capture.out_width),
        min_band_coverage: 0.70,
        pipeline: if std::env::var("PIPELINE").ok().as_deref() == Some("legacy") {
            Pipeline::Legacy
        } else {
            Pipeline::Quality
        },
        quality: QualityParams {
            seam_half_res: std::env::var("SEAM_FULL").is_err(),
            // Knobs A/B (env) : ALIGN=0 désactive le raffinement de pose,
            // LOCALGAIN=0 les gains locaux, GAINCH=1 gains luminance seule,
            // MODEL=<dir> active le raffinement rotation par modèle IA,
            // LAMBDA_YAW/LAMBDA_PR les priors capteur du chemin modèle,
            // INLIERS le seuil d'acceptation de paire.
            align: std::env::var("ALIGN").ok().as_deref() != Some("0"),
            multiband_levels: std::env::var("LEVELS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(QualityParams::default().multiband_levels),
            gain_channels: std::env::var("GAINCH")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            model_lambda_yaw: std::env::var("LAMBDA_YAW")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.3),
            model_lambda_pitch_roll: std::env::var("LAMBDA_PR")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.5),
            model_min_inliers: std::env::var("INLIERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(QualityParams::default().model_min_inliers),
            ..QualityParams::default()
        },
    };
    // Knob MODEL : raffinement de pose par SuperPoint+LightGlue (feature
    // `pose-model` requise). MOD=vide → chemin NCC historique.
    let model_dir = std::env::var("MODEL").ok().filter(|s| !s.is_empty());
    let t0 = std::time::Instant::now();
    #[cfg(feature = "pose-model")]
    let out = {
        let model = model_dir
            .as_deref()
            .map(pnex_stitcher::pose_model::PoseModel::load)
            .transpose()
            .expect("chargement modèle");
        if model.is_some() {
            println!("modèle : {}", model_dir.as_deref().unwrap());
        }
        let res = pnex_stitcher::stitch_with_model(&frames, &params, model.as_ref());
        res.expect("stitch")
    };
    #[cfg(not(feature = "pose-model"))]
    let out = {
        let _ = model_dir;
        pnex_stitcher::stitch(&frames, &params).expect("stitch")
    };
    println!(
        "OK {}x{} en {:.1} s ({} o), couverture {:.2}",
        out.width,
        out.height,
        t0.elapsed().as_secs_f32(),
        out.jpeg.len(),
        out.band_coverage
    );
    if let Some(report) = &out.align_report {
        println!(
            "align : convergé={} acceptées={} rejetées={} repli={} correction max {:.2}°",
            report.converged,
            report.accepted.len(),
            report.rejected,
            report.fallback,
            report
                .corrections_deg
                .iter()
                .fold(0.0f32, |m, c| m.max(c[0].hypot(c[1])))
        );
    }
    std::fs::write(format!("{dir}/panorama.jpg"), &out.jpeg).expect("écriture panorama");
    println!("→ {dir}/panorama.jpg");

    // DIAG couverture : % de pixels couverts par bande de 5° de latitude —
    // montre exactement où le contenu réel s'arrête (les calottes
    // synthétiques apparaissent comme des bandes < 20 %).
    if std::env::var("PROFILE").is_ok() {
        let img = image::load_from_memory(&out.jpeg).expect("relire panorama");
        let gray = img.to_luma8();
        let (w, h) = (gray.width() as usize, gray.height() as usize);
        println!("\nprofils de contenu par latitude (moyenne locale du gradient) :");
        let band = h / 36; // bandes de 5°
        for b in 0..36 {
            let (y0, y1) = (b * band, (b + 1) * band);
            let lat_hi = 90.0 - 5.0 * b as f32;
            // Gradient horizontal moyen = proxy de contenu réel (une calotte
            // habillée est quasi lisse verticalement, mais le vrai test est
            // le gradient vertical local).
            let mut grad = 0.0f64;
            let mut cnt = 0u64;
            for y in (y0..y1.min(h - 1)).step_by(4) {
                for x in (0..w).step_by(8) {
                    let a = gray.get_pixel(x as u32, y as u32).0[0] as f64;
                    let bl = gray.get_pixel(x as u32, (y + 1) as u32).0[0] as f64;
                    grad += (a - bl).abs();
                    cnt += 1;
                }
            }
            println!(
                "  lat [{:+6.1} → {:+6.1}]  gradient moyen = {:.2}",
                lat_hi - 5.0,
                lat_hi,
                grad / cnt.max(1) as f64
            );
        }
    }
}
