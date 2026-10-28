//! Stitches a ring of guided photos into a 2:1 equirectangular.
//!
//! Take 360 v1: the mobile app captures 16 frames while turning on the spot
//! (sensor guidance), recording for each the pose (yaw, pitch, roll)
//! at the trigger. This crate assembles the frames on the sphere
//! **without interest point detection** — placement comes solely
//! from the poses (assumption: fused sensor, drift accepted in v1).
//!
//! ## Pipeline
//!
//! 1. preparation (rectilinear focal length from the HFOV, world→camera matrix,
//!    covered latitude band) — [`project`];
//! 2. row-wise equirect render, **selection** weighted by distance to the edge
//!    (no averaging → no ghosts) preceded by an inter-frame gain
//!    compensation, bilinear sampling — [`render`];
//! 3. coverage check **within the covered band** (a horizontal
//!    ring only covers a slice of the equirect);
//! 4. pole filling by spreading the nearest covered row
//!    (quick & dirty, on purpose);
//! 5. JPEG encoding + GPano metadata ([`gpano`]) — the backend sniff
//!    then classifies the asset as `panorama` on its own.
//!
//! Pure Rust, no OpenCV, no native dependency: compiles everywhere,
//! testable on host with a synthetic scene ([`tests`] as integration).
//!
//! ## Memory / perf
//!
//! The accumulators are f32 rows (no full-image f32 buffer).
//! Output width: [`Params::out_width`] — 2048 by default (device);
//! 4096 remains possible but is judged on `stitch_ms` measured on the phone
//! (see docs/architecture/media.md).

pub mod align;
pub mod geom;
pub mod gpano;
pub mod jpeg;
pub mod multiband;
#[cfg(feature = "pose-model")]
pub mod pose_model;
pub mod project;
pub mod render;
pub mod rotation;
pub mod seam;

use thiserror::Error;

pub use jpeg::{decode_jpeg_rgb, decode_jpeg_rgb_capped, encode_jpeg, JpegError};

/// Fallback HFOV (degrees) if the Camera2 query fails — main rear
/// wide-angle of current phones.
pub const FALLBACK_HFOV_DEG: f32 = 65.0;

/// Default output width (device) — perf knob: 1024 = guaranteed
/// completion on a phone under memory pressure (finding 2026-09-09:
/// 2048 + swap → stitch never finished); raise towards [`SPEC_OUT_WIDTH`]
/// when perf allows.
pub const DEVICE_OUT_WIDTH: u32 = 1024;

/// Encoding quality of the final panorama. 90 (instead of 85): the sharp
/// selection of the blend creates crisper seams than averaging — the
/// extra JPEG margin avoids ringing at the join, and the device
/// output gains fidelity for ~+25 % file weight.
pub const OUTPUT_JPEG_QUALITY: u8 = 90;

/// A captured frame: RGB8 pixels + pose at the trigger.
#[derive(Debug, Clone)]
pub struct Frame {
    /// RGB8 pixels, `width·height·3` bytes.
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Yaw in degrees, relative to the session start frame.
    pub yaw_deg: f32,
    /// Gaze elevation in degrees (+ = upwards).
    pub pitch_deg: f32,
    /// Roll in degrees around the optical axis.
    pub roll_deg: f32,
}

/// Stitching parameters.
#[derive(Debug, Clone)]
pub struct Params {
    /// Horizontal HFOV of the lens (degrees) — Camera2 query on the
    /// caller side, [`FALLBACK_HFOV_DEG`] as fallback.
    pub hfov_deg: f32,
    /// Output width (height = width/2).
    pub out_width: u32,
    /// Minimum coverage of the **band core** (0-1): fraction of covered pixels
    /// among the rows covered at ≥ 90 % (see
    /// [`render::band_coverage`]). Below this threshold the ring has a hole (missing
    /// frame) and the result is refused.
    pub min_band_coverage: f32,
    /// Render pipeline: [`Pipeline::Legacy`] (selection by centrality,
    /// default — the Android device keeps exactly the V1 behaviour) or
    /// [`Pipeline::Quality`] (alignment + DP seams + multi-band,
    /// intended for the server).
    pub pipeline: Pipeline,
    /// Fine settings of the [`Pipeline::Quality`] pipeline.
    pub quality: QualityParams,
}

/// Render pipeline choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pipeline {
    /// V1 "selection" blend (interior^P), raw sensor poses. Default:
    /// historical behaviour byte for byte.
    #[default]
    Legacy,
    /// Quality pipeline: per-channel gains → image-based alignment →
    /// DP seams → multi-band blending → fills.
    Quality,
}

/// Quality pipeline settings (default values = initial tuning,
/// see docs/architecture/media.md).
#[derive(Debug, Clone)]
pub struct QualityParams {
    /// Image-based pose refinement (A/B tests: `false` = raw sensor
    /// poses).
    pub align: bool,
    /// Working width of the grey equirect strips for alignment.
    pub align_work_width: u32,
    /// Per-pair offset search window (degrees).
    pub align_search_deg: f32,
    /// Minimum NCC threshold, same-pitch pairs.
    pub align_min_ncc_intra: f32,
    /// Minimum NCC threshold, inter-ring pairs (content stretched ×1/cos φ →
    /// structurally lower peak).
    pub align_min_ncc_inter: f32,
    /// Tikhonov regularisation towards the sensor pose (1/rad²) — it is the
    /// gauge of the solve; ≈ (σ_measure/σ_pose)² with σ_measure ≈ 0.15° and
    /// σ_pose ≈ 1.5°.
    pub align_lambda_reg: f32,
    /// Maximum Gauss-Newton iterations.
    pub align_max_iters: u32,
    /// Levels of the Laplacian pyramid (1-8). DEFAULT 3 (history
    /// 2026-09-11: 2 → 1 → 3 — at 2, the inverted-Laplacian-sign bugs +
    /// numerator not weighted by the mask turned "blending" into a
    /// double exposure; fixed, 3 levels melt wide without
    /// ghosting). 1 = flat crossfade path + narrow feather (fallback).
    pub multiband_levels: u32,
    /// DP seams at half resolution (2× faster, natural smoothing).
    pub seam_half_res: bool,
    /// Gain compensation channels: 1 = replicated luminance
    /// (Legacy), 3 = independent R/G/B (white balance drifts).
    pub gain_channels: u32,
    // ─── Model-based pose refinement (`pose-model`, server) ───
    /// Longest side of the SuperPoint working image (multiple of 8 in
    /// practice via `pose_model::work_size`). 896 ≈ 0.44 Mpx → extraction
    /// ~0.3 s/pair on RPi.
    pub model_work_px: u32,
    /// Minimum RANSAC inliers to accept a pair (a pair with 10
    /// inliers on fine text can be a mirage; 15 is cautious without
    /// being stingy).
    pub model_min_inliers: u32,
    /// RANSAC tolerance in pixels of the working image (equivalent angular
    /// error via the working focal length). 3 px ≈ 0.27° at 504×896.
    pub model_max_err_px: f32,
    /// Sensor yaw prior — WEAK: the indoor magnetometer is the
    /// main source of error, we let the solve move the yaw
    /// freely (gauge ensured by the pair graph).
    pub model_lambda_yaw: f64,
    /// Sensor pitch/roll prior — stronger: gravity (accelerometer) is
    /// reliable, the solve should only touch them marginally.
    pub model_lambda_pitch_roll: f64,
}

impl Default for QualityParams {
    fn default() -> Self {
        QualityParams {
            align: true,
            align_work_width: 512,
            align_search_deg: 4.0,
            align_min_ncc_intra: 0.5,
            align_min_ncc_inter: 0.35,
            align_lambda_reg: 0.05,
            align_max_iters: 5,
            // 3 (history 2026-09-11: 2 → 1 → 3). At 2, the blend
            // actually "softened" through TWO bugs (inverted Laplacian
            // sign + numerator not weighted by the mask:
            // double counting of full content in the overlap) —
            // hence saturated white halos, fringes and ghosting diffusion.
            // Fixed (tests recouvrement_constant_sans_double_comptage +
            // fine-octave reconstruction), the multi-band does its job
            // : wide feather without ghosting. LEVELS=1 = flat path
            // crossfade + narrow feather (fallback knob).
            // 5 since 2026-09-28: once each frame's image extends past its
            // weight (multiband.rs, `dilate_support`), wide levels no longer
            // halo — seams and per-frame exposure steps melt, no added
            // ghosting (real capture beb93c42; ~250 s vs ~140 s at 4096).
            multiband_levels: 5,
            seam_half_res: true,
            gain_channels: 3,
            model_work_px: 896,
            // 8 (was 15): textureless areas (white wall, whiteboard,
            // ceiling) left their pairs rejected → no image
            // constraint → raw sensor poses → DOUBLED content at the seams
            // (~30 px at 4096, real finding 2026-09-11 capture 52fdd79b,
            // whiteboard area). Horn/RANSAC stays guarded by
            // median_err ≤ model_max_err_px — 8 inliers is enough.
            model_min_inliers: 8,
            model_max_err_px: 3.0,
            model_lambda_yaw: 0.3,
            model_lambda_pitch_roll: 1.5,
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Params {
            hfov_deg: FALLBACK_HFOV_DEG,
            out_width: DEVICE_OUT_WIDTH,
            min_band_coverage: 0.98,
            pipeline: Pipeline::Legacy,
            quality: QualityParams::default(),
        }
    }
}

/// Result of a successful stitching.
#[derive(Debug, Clone)]
pub struct Stitched {
    /// Complete JPEG (GPano metadata included).
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Measured coverage of the covered band (0-1).
    pub band_coverage: f32,
    /// Stitching duration (render + encoding), milliseconds.
    pub stitch_ms: u32,
    /// Alignment report (Quality pipeline only): applied pose
    /// corrections, per-pair residuals — server logging.
    pub align_report: Option<align::AlignReport>,
}

/// Stitching errors.
#[derive(Debug, Error)]
pub enum StitchError {
    /// No frame provided.
    #[error("no frame")]
    NoFrames,
    /// Inconsistent frame (buffer ≠ w·h·3).
    #[error("frame {index} inconsistent: {got} bytes, expected {expected}")]
    InvalidFrame {
        /// Index of the faulty frame.
        index: usize,
        /// Received size.
        got: usize,
        /// Expected size (w·h·3).
        expected: usize,
    },
    /// Out-of-bounds parameter.
    #[error("invalid parameter: {0}")]
    Params(String),
    /// Ring with a hole: insufficient band coverage.
    #[error("insufficient band coverage: {0:.0} %")]
    Coverage(f32),
    /// Panorama encoding failure.
    #[error("encoding: {0}")]
    Encode(#[from] JpegError),
}

/// Optional reference to the pose model — phantom type to share
/// `stitch_inner` between the device (no model) and server
/// (feature `pose-model`) builds.
#[cfg(feature = "pose-model")]
pub type ModelRef<'a> = pose_model::PoseModel;
/// Device variant: no model, the alias exists for the signature only.
#[cfg(not(feature = "pose-model"))]
pub type ModelRef<'a> = ();

/// Assembles the frames into a 2:1 equirectangular JPEG (GPano included).
///
/// # Errors
/// - [`StitchError::NoFrames`] / [`StitchError::InvalidFrame`]: inputs;
/// - [`StitchError::Params`]: absurd hfov or width;
/// - [`StitchError::Coverage`]: hole in the ring (see
///   [`Params::min_band_coverage`]);
/// - [`StitchError::Encode`]: JPEG failure.
pub fn stitch(frames: &[Frame], params: &Params) -> Result<Stitched, StitchError> {
    stitch_inner(frames, params, None)
}

/// Like [`stitch`], with pose refinement by AI model (SuperPoint +
/// LightGlue) — feature `pose-model`, server only. `None` = historical
/// NCC path.
///
/// # Errors
/// Same as [`stitch`].
#[cfg(feature = "pose-model")]
pub fn stitch_with_model(
    frames: &[Frame],
    params: &Params,
    model: Option<&pose_model::PoseModel>,
) -> Result<Stitched, StitchError> {
    stitch_inner(frames, params, model)
}

/// Stitching core — `model` = image-based pose refinement
/// (3 DOF rotation) when provided, otherwise 2 DOF NCC translation refinement.
fn stitch_inner(
    frames: &[Frame],
    params: &Params,
    #[allow(unused_variables)] model: Option<&ModelRef<'_>>,
) -> Result<Stitched, StitchError> {
    let start = std::time::Instant::now();

    if frames.is_empty() {
        return Err(StitchError::NoFrames);
    }
    if !(1.0..=179.0).contains(&params.hfov_deg) {
        return Err(StitchError::Params(format!("hfov {}", params.hfov_deg)));
    }
    let out_width = params.out_width;
    if out_width < 64 || !out_width.is_multiple_of(2) || out_width > 8192 {
        return Err(StitchError::Params(format!("out_width {out_width}")));
    }
    let out_height = out_width / 2;

    // Preparation: validation + focal length + matrix + latitude band.
    let mut prepared: Vec<project::PreparedFrame> = Vec::with_capacity(frames.len());
    for (k, f) in frames.iter().enumerate() {
        let expected = f.width as usize * f.height as usize * 3;
        if f.rgb.len() != expected || f.width == 0 || f.height == 0 {
            return Err(StitchError::InvalidFrame {
                index: k,
                got: f.rgb.len(),
                expected,
            });
        }
        let f_px = project::f_px_from_hfov(params.hfov_deg, f.width);
        let (lat_min, lat_max) = project::lat_band(f.height, f_px, f.pitch_deg);
        prepared.push(project::PreparedFrame {
            yaw_deg: f.yaw_deg,
            pitch_deg: f.pitch_deg,
            roll_deg: f.roll_deg,
            inv: geom::euler_to_mat(f.yaw_deg, f.pitch_deg, f.roll_deg),
            width: f.width,
            height: f.height,
            f_px,
            lat_min,
            lat_max,
            // Feather: ~10 % of the shortest side, bounded to stay useful.
            feather_px: (f.width.min(f.height) as f32 * 0.10).max(1.0),
            gain: [1.0; 3],
            local_gain: Vec::new(),
            warp: None,
            rgb: f.rgb.clone(),
        });
    }

    let mut align_report: Option<align::AlignReport> = None;
    let (mut rgb, covered, mut mask);
    if params.pipeline == Pipeline::Quality {
        let qp = &params.quality;
        // 1) gains per-channel grossiers (poses brutes).
        let gains = render::estimate_gains(&prepared, qp.gain_channels);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        // 2) image-based alignment. With a model (server): 3 DOF relative
        //    rotations (SuperPoint+LightGlue → Horn/RANSAC → SO(3) geodesic
        //    solve); the sensor pose is only a prior. Otherwise
        //    (device / missing model / too few pairs): historical 2 DOF
        //    NCC translation. Re-derives inv/lat band via refresh_pose.
        // Sensor poses (for the model path fallback — cfg pose-model).
        #[cfg_attr(not(feature = "pose-model"), allow(unused_variables))]
        let sensor_poses: Vec<[f32; 3]> = prepared
            .iter()
            .map(|f| [f.yaw_deg, f.pitch_deg, f.roll_deg])
            .collect();
        let report = match model {
            #[cfg(feature = "pose-model")]
            Some(m) => {
                let rep = align::refine_poses_model(&mut prepared, qp, m);
                // NCC fallback if the model did not anchor enough pairs
                // (textureless scene, failing model): we restart from the
                // sensor poses, never from a half-state.
                let gate = (prepared.len() / 3).max(2);
                if rep.converged && rep.accepted.len() >= gate {
                    rep
                } else {
                    for (f, p) in prepared.iter_mut().zip(sensor_poses) {
                        [f.yaw_deg, f.pitch_deg, f.roll_deg] = p;
                        f.refresh_pose();
                    }
                    let mut ncc = align::refine_poses(&mut prepared, qp);
                    ncc.fallback = true;
                    ncc
                }
            }
            _ => align::refine_poses(&mut prepared, qp),
        };
        align_report = Some(report);
        // 2ter) Local warp (mesh): sticks the overlaps together at the
        // local level (~0.1-0.3° of residual the global 3 DOF align does not
        // see) — copper fringes on wooden edges and "torn" objects
        // disappear. HISTORY 2026-09-11: disabled in the evening because
        // "hyper contrasted" — but the real culprit was the
        // multi-band double counting (sign + weighting bugs,
        // fixed that same evening) which amplified everything; re-enabled after
        // full-frame validation of warp × fixed pyramid. LOCALWARP=0:
        // A/B knob.
        if std::env::var("LOCALWARP").ok().as_deref() != Some("0") {
            align::estimate_local_warp(&mut prepared);
        }
        // 3) final gains on corrected poses.
        let gains = render::estimate_gains(&prepared, qp.gain_channels);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        // 3bis) low-frequency local gains (flatten the illumination
        // gradients — otherwise visible rectangular joins).
        // LOCALGAIN=0: A/B knob (3-ring capture, finding 2026-09-10).
        if std::env::var("LOCALGAIN").ok().as_deref() != Some("0") {
            render::estimate_local_gains(&mut prepared);
        }
        // 4) per-pair DP seams → ownership map.
        let owner = seam::find_seams(&prepared, out_width, qp);
        // 5) multi-band blending (binary masks), 6) fills AFTER.
        let coverage = render::band_coverage(out_width, &owner.covered_per_row());
        if coverage < params.min_band_coverage {
            return Err(StitchError::Coverage(coverage * 100.0));
        }
        let (blended, covered_mask) = multiband::blend(&prepared, &owner, qp.multiband_levels);
        rgb = blended;
        covered = owner.covered_per_row();
        mask = covered_mask;
    } else {
        // Legacy pipeline: replicated luminance gains + selection render.
        let gains = render::estimate_gains(&prepared, 1);
        for (pf, g) in prepared.iter_mut().zip(gains) {
            pf.gain = g;
        }
        let (rendered, counts, mask_px) = render::render(&prepared, out_width);
        rgb = rendered;
        covered = counts;
        mask = mask_px;
    }

    // Display handedness: the internal mapping (u grows with ENU lon)
    // renders a mirrored image when displayed "from the inside" — the
    // output mirror (rgb + mask, `covered` is invariant) restores the
    // equirect standard: u grows when turning RIGHT. A single inversion
    // in the chain, at the boundary — the internal pipeline (NCC align +
    // probe, solve, seams, multiband) keeps its historical convention.
    // Device finding 2026-09-11: "clean but mirrored" left-right.
    render::mirror_rows(&mut rgb, out_width);
    render::mirror_mask(&mut mask, out_width);

    // Band coverage check (BEFORE any filling: the metric
    // must reflect the real capture, not the dressing).
    let coverage = render::band_coverage(out_width, &covered);
    if coverage < params.min_band_coverage {
        return Err(StitchError::Coverage(coverage * 100.0));
    }

    // Intra-band holes (corner insets), then poles by spreading.
    render::fill_band_holes(&mut rgb, out_width, &mask);
    render::fill_poles(&mut rgb, out_width, &covered);

    // Encoding + GPano.
    let plain = encode_jpeg(&rgb, out_width, out_height, OUTPUT_JPEG_QUALITY)?;
    let jpeg = gpano::write_gpano_xmp(&plain, out_width, out_height);

    Ok(Stitched {
        jpeg,
        width: out_width,
        height: out_height,
        band_coverage: coverage,
        stitch_ms: start.elapsed().as_millis() as u32,
        align_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A single frame facing the lon 0 ray: renders without error, coverage
    /// ≈ hfov/360 (band full in latitude, not in longitude — a single
    /// frame does not make a ring), 2:1 dimensions, GPano at the head.
    #[test]
    fn frame_unique() {
        let (w, h) = (64u32, 48u32);
        let frame = Frame {
            rgb: vec![128; (w * h * 3) as usize],
            width: w,
            height: h,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        let params = Params {
            min_band_coverage: 0.0,
            ..Params::default()
        };
        let out = stitch(&[frame], &params).unwrap();
        assert_eq!(out.width, DEVICE_OUT_WIDTH);
        assert_eq!(out.height, DEVICE_OUT_WIDTH / 2);
        // A single frame: band coverage ≈ hfov/360 (band full
        // in latitude, holed in longitude — it is not a ring).
        assert!(
            (out.band_coverage - 65.0 / 360.0).abs() < 0.02,
            "coverage = {}",
            out.band_coverage
        );
        assert!(out.stitch_ms < 60_000);
        // Sniff-like: the GPano literals are indeed within the first 128 KB.
        let head = &out.jpeg[..out.jpeg.len().min(128 * 1024)];
        assert!(head.windows(6).any(|w| w == b"GPano:"));
        assert!(head.windows(15).any(|w| w == b"equirectangular"));
    }

    /// Handedness PROBE: the content on the left of a frame's image
    /// (lon > the frame's yaw, see euler_to_mat: image right = SMALLER
    /// lon) must come out to the LEFT of centre in the final equirect —
    /// standard: u grows when turning right. Locks the output mirror of
    /// [`stitch`] (device finding 2026-09-11: L-R mirror).
    #[test]
    fn rendu_handedness() {
        let (w, h) = (512u32, 384u32);
        // Left half of the image white (lon > 30), right black (lon < 30).
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for j in 0..h {
            for i in 0..w {
                let v = if (i as f32) < w as f32 / 2.0 { 240 } else { 10 };
                let o = ((j * w + i) * 3) as usize;
                rgb[o] = v;
                rgb[o + 1] = v;
                rgb[o + 2] = v;
            }
        }
        let frame = Frame {
            rgb,
            width: w,
            height: h,
            yaw_deg: 30.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        let params = Params {
            out_width: 360,
            hfov_deg: 40.0,
            min_band_coverage: 0.0,
            ..Params::default()
        };
        let out = stitch(&[frame], &params).unwrap();
        let (px, dec_w, dec_h) = jpeg::decode_jpeg_rgb(&out.jpeg).expect("decode");
        assert_eq!((dec_w, dec_h), (360, 180));
        let mid = 90usize; // lat ≈ 0
        let at = |col: usize| px[(mid * 360 + col) * 3];
        // lon 30 → internal column 210 → mirror: 359−210 = 149. The white
        // area (lon > 30) ends to the LEFT of 149, the black one to the right.
        let blanc = at(145);
        let noir = at(155);
        assert!(
            blanc > 200 && noir < 60,
            "handedness : blanc(col145)={blanc} noir(col155)={noir}"
        );
    }

    /// Absurd parameters → named errors.
    #[test]
    fn parametres_invalides() {
        let frame = Frame {
            rgb: vec![0; 64 * 48 * 3],
            width: 64,
            height: 48,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        assert!(matches!(
            stitch(&[], &Params::default()),
            Err(StitchError::NoFrames)
        ));
        let bad = Params {
            hfov_deg: 200.0,
            ..Params::default()
        };
        assert!(matches!(
            stitch(std::slice::from_ref(&frame), &bad),
            Err(StitchError::Params(_))
        ));
        let bad = Params {
            out_width: 1001, // odd → invalid
            ..Params::default()
        };
        assert!(matches!(
            stitch(&[frame], &bad),
            Err(StitchError::Params(_))
        ));
    }

    /// Inconsistent frame → `InvalidFrame` with the index.
    #[test]
    fn frame_incoherente() {
        let frame = Frame {
            rgb: vec![0; 10],
            width: 64,
            height: 48,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        assert!(matches!(
            stitch(&[frame], &Params::default()),
            Err(StitchError::InvalidFrame { index: 0, .. })
        ));
    }
}
