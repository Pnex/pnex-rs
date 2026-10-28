//! Feature matching by a lightweight AI model (cargo feature `pose-model`,
//! server-only) — SuperPoint + LightGlue ONNX via ort.
//!
//! The device Quality pipeline stays 100 % pure Rust (feature absent: neither
//! `ort`, nor a native binary in the APK); the server loads SuperPoint +
//! LightGlue from `deploy/models/` (committed):
//!
//! - `PoseModel::extract`: RGB frame → keypoints + descriptors (SuperPoint,
//!   grey resized to `work_max_px` on the longest side, multiple of
//!   8 — SuperPoint's /32 pooling);
//! - `PoseModel::match_pair`: LightGlue correspondences (indices + score).
//!
//! The correspondences feed `crate::rotation::horn_ransac` (relative
//! rotation, pure-rotation model) then the global solve
//! `crate::align::refine_poses_model` — the sensor pose is only a
//! prior, the image evidence decides.
//!
//! ## Why these v1 models (fabio-sim/LightGlue-ONNX)
//!
//! Dynamic shapes (adaptive N keypoints): we extract ONCE per frame
//! (28 extractions for 28 frames) and only pay for the transformer per
//! pair. The merged v2/v3 "pipeline" exports would recompute
//! the extraction for BOTH images of EACH pair (~80 extractions for
//! 28 frames — prohibitive on RPi).

#![cfg(feature = "pose-model")]

use std::sync::Mutex;

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use thiserror::Error;

/// Pose model errors.
#[derive(Debug, Error)]
pub enum PoseModelError {
    /// Missing .onnx file or session impossible.
    #[error("loading: {0}")]
    Load(String),
    /// Inference failure.
    #[error("inference: {0}")]
    Inference(String),
}

/// Pose model: SuperPoint (extractor) + LightGlue (matcher) sessions.
/// `Mutex` because `ort` rc.13 requires `&mut Session` for `run` (the C API is
/// thread-safe, the binding doesn't expose it) — within a job the model is
/// used sequentially, across jobs each worker has its own instance.
pub struct PoseModel {
    extractor: Mutex<Session>,
    matcher: Mutex<Session>,
}

/// Features of a frame: keypoints in pixels of the working image
/// (`w`×`h`, multiple of 8) + 256-dim descriptors (N×256, row-major).
#[derive(Debug, Clone)]
pub struct Features {
    /// Width of the working image (px).
    pub w: u32,
    /// Height of the working image (px).
    pub h: u32,
    /// Keypoints (u, v) in pixels of the working image.
    pub kpts_px: Vec<[f32; 2]>,
    /// N×256 row-major descriptors.
    pub desc: Vec<f32>,
}

/// One correspondence: indices into `a.kpts_px` and `b.kpts_px`.
#[derive(Debug, Clone, Copy)]
pub struct PointMatch {
    pub ia: usize,
    pub ib: usize,
    /// LightGlue confidence score (0-1).
    pub score: f32,
}

impl PoseModel {
    /// Loads both sessions from `dir` (`superpoint.onnx` +
    /// `superpoint_lightglue_fused_cpu.onnx`). Level1 optimisation: the
    /// exports are already fused — Level3 brings nothing and slows
    /// loading.
    pub fn load(dir: &str) -> Result<Self, PoseModelError> {
        let load = |name: &str| -> Result<Session, PoseModelError> {
            let builder =
                Session::builder().map_err(|e| PoseModelError::Load(format!("{name} : {e}")))?;
            let mut builder = builder
                .with_optimization_level(GraphOptimizationLevel::Level1)
                .map_err(|e| PoseModelError::Load(format!("{name} : {e}")))?;
            builder
                .commit_from_file(format!("{dir}/{name}"))
                .map_err(|e| PoseModelError::Load(format!("{name} : {e}")))
        };
        let extractor = Mutex::new(load("superpoint.onnx")?);
        let matcher = Mutex::new(load("superpoint_lightglue_fused_cpu.onnx")?);
        Ok(PoseModel { extractor, matcher })
    }

    /// Extracts keypoints + descriptors of an RGB8 frame. The working image
    /// keeps the aspect, longest side ≤ `work_max_px`, bounds rounded to
    /// a multiple of 8 (SuperPoint pooling).
    pub fn extract(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
        work_max_px: u32,
    ) -> Result<Features, PoseModelError> {
        let (w, h) = work_size(width, height, work_max_px);
        let data = resize_gray(rgb, width, height, w, h);
        let input = Tensor::from_array(([1_i64, 1, h as i64, w as i64], data))
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let mut session = self
            .extractor
            .lock()
            .map_err(|_| PoseModelError::Inference("poison".into()))?;
        let outputs = session
            .run(ort::inputs!["image" => input])
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;

        let (kshape, kpts_i64) = outputs["keypoints"]
            .try_extract_tensor::<i64>()
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let (_sshape, _scores) = outputs["scores"]
            .try_extract_tensor::<f32>()
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let (dshape, desc) = outputs["descriptors"]
            .try_extract_tensor::<f32>()
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;

        // keypoints (1, N, 2), scores (1, N), descriptors (1, N, 256).
        let n = kshape[1] as usize;
        let mut kpts_px = Vec::with_capacity(n);
        for k in 0..n {
            kpts_px.push([kpts_i64[2 * k] as f32, kpts_i64[2 * k + 1] as f32]);
        }
        let dim = dshape[2] as usize; // 256
        let mut d = vec![0.0f32; n * dim];
        // (1, N, 256) → N×256 row-major (already contiguous, flat copy).
        d.copy_from_slice(&desc[..n * dim]);

        Ok(Features {
            w,
            h,
            kpts_px,
            desc: d,
        })
    }

    /// LightGlue correspondences of a frame pair (indices + score).
    pub fn match_pair(
        &self,
        a: &Features,
        b: &Features,
    ) -> Result<Vec<PointMatch>, PoseModelError> {
        let (na, nb) = (a.kpts_px.len(), b.kpts_px.len());
        if na < 8 || nb < 8 {
            return Ok(Vec::new());
        }
        // Keypoints NORMALISED to [0,1] per axis (measured A/B on a real capture:
        // 567 normalised matches vs 44 in raw pixels — the v1 "fused" export
        // expects normalisation, the image size not being an input).
        let flat_a: Vec<f32> = a
            .kpts_px
            .iter()
            .flat_map(|p| [p[0] / a.w as f32, p[1] / a.h as f32])
            .collect();
        let flat_b: Vec<f32> = b
            .kpts_px
            .iter()
            .flat_map(|p| [p[0] / b.w as f32, p[1] / b.h as f32])
            .collect();
        let kpts0 = Tensor::from_array(([1_i64, na as i64, 2], flat_a))
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let kpts1 = Tensor::from_array(([1_i64, nb as i64, 2], flat_b))
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let desc0 = Tensor::from_array(([1_i64, na as i64, 256_i64], a.desc.clone()))
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let desc1 = Tensor::from_array(([1_i64, nb as i64, 256_i64], b.desc.clone()))
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;

        let mut session = self
            .matcher
            .lock()
            .map_err(|_| PoseModelError::Inference("poison".into()))?;
        let outputs = session
            .run(ort::inputs![
                "kpts0" => kpts0,
                "kpts1" => kpts1,
                "desc0" => desc0,
                "desc1" => desc1
            ])
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;

        let (mshape, matches) = outputs["matches0"]
            .try_extract_tensor::<i64>()
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;
        let (_sshape, mscores) = outputs["mscores0"]
            .try_extract_tensor::<f32>()
            .map_err(|e| PoseModelError::Inference(e.to_string()))?;

        // matches0 (M, 2): indices (ia, ib) already filtered (NonZero).
        let m = mshape[0] as usize;
        let mut out = Vec::with_capacity(m);
        for k in 0..m {
            let (ia, ib) = (matches[2 * k], matches[2 * k + 1]);
            if ia < 0 || ib < 0 {
                continue; // -1 = no correspondence (defensive: the
                          // NonZero export is supposed to filter them)
            }
            out.push(PointMatch {
                ia: ia as usize,
                ib: ib as usize,
                score: mscores[k],
            });
        }
        Ok(out)
    }
}

/// Working dimensions: longest side ≤ `max_px`, BOTH sides
/// rounded to a multiple of 8 (SuperPoint's /32 pooling: H, W must
/// be divisible by 8 at the output of the cumulated strides), minimum 64.
#[must_use]
pub fn work_size(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let big = width.max(height);
    let small = width.min(height) as f32;
    // Never upscale: long = min(max_px, longest side).
    let long = max_px.min(big).max(64);
    let short = ((long as f32 * small / big as f32).round() as u32).clamp(64, long);
    let round8 = |v: u32| ((v / 8) * 8).max(64);
    if width >= height {
        (round8(long), round8(short))
    } else {
        (round8(short), round8(long))
    }
}

/// RGB8 → f32 greyscale [0,1] by averaging subsampling
/// (box) — anti-aliased, no dep (`image` has no resize without
/// default-features).
#[must_use]
pub fn resize_gray(rgb: &[u8], width: u32, height: u32, ow: u32, oh: u32) -> Vec<f32> {
    let (iw, ih) = (width as usize, height as usize);
    let (ow, oh) = (ow as usize, oh as usize);
    let mut out = vec![0.0f32; ow * oh];
    for j in 0..oh {
        let y0 = j * ih / oh;
        let y1 = ((j + 1) * ih / oh).max(y0 + 1);
        for i in 0..ow {
            let x0 = i * iw / ow;
            let x1 = ((i + 1) * iw / ow).max(x0 + 1);
            let mut acc = 0.0f32;
            let mut cnt = 0u32;
            for y in y0..y1 {
                let base = y * iw;
                for x in x0..x1 {
                    let o = (base + x) * 3;
                    acc += 0.299 * rgb[o] as f32
                        + 0.587 * rgb[o + 1] as f32
                        + 0.114 * rgb[o + 2] as f32;
                    cnt += 1;
                }
            }
            out[j * ow + i] = (acc / cnt as f32) / 255.0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// work_size: portrait aspect 1080×1920 at max 896 → 504×896 (both
    /// multiples of 8); landscape the reverse; minimum bounded.
    #[test]
    fn working_size() {
        assert_eq!(work_size(1080, 1920, 896), (504, 896));
        assert_eq!(work_size(1920, 1080, 896), (896, 504));
        assert_eq!(work_size(64, 64, 896), (64, 64));
        assert_eq!(work_size(1080, 1920, 512), (288, 512));
    }

    /// resize_gray: constant everywhere = constant everywhere, mean of a
    /// gradient preserved, no overflow of [0,1].
    #[test]
    fn gray_mean() {
        let (w, h) = (64u32, 64u32);
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        for j in 0..h {
            for i in 0..w {
                let o = ((j * w + i) * 3) as usize;
                let v = (i * 4) as u8;
                rgb[o] = v;
                rgb[o + 1] = v;
                rgb[o + 2] = v;
            }
        }
        let g = resize_gray(&rgb, w, h, 16, 16);
        assert!(g.iter().all(|&v| (0.0..=1.0).contains(&v)));
        // Global mean preserved (linear gradient 0..252 → ~0.5).
        let m = g.iter().sum::<f32>() / g.len() as f32;
        assert!((m - 0.492).abs() < 0.05, "mean = {m}");
    }
}
