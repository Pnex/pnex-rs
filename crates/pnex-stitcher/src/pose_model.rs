//! Matching de features par modèle IA léger (feature cargo `pose-model`,
//! server-only) — SuperPoint + LightGlue ONNX via ort.
//!
//! Le pipeline Quality du device reste 100 % pure Rust (feature absente : ni
//! `ort`, ni binaire natif dans l'APK) ; le serveur charge SuperPoint +
//! LightGlue depuis `deploy/models/` (committés) :
//!
//! - `PoseModel::extract` : frame RGB → keypoints + descripteurs (SuperPoint,
//!   gris redimensionné à `work_max_px` sur le plus grand côté, multiple de
//!   8 — pooling /32 de SuperPoint) ;
//! - `PoseModel::match_pair` : correspondances LightGlue (indices + score).
//!
//! Les correspondances alimentent `crate::rotation::horn_ransac` (rotation
//! relative, modèle rotation pure) puis le solve global
//! `crate::align::refine_poses_model` — la pose capteur n'est plus qu'un
//! prior, la preuve image décide.
//!
//! ## Pourquoi ces modèles v1 (fabio-sim/LightGlue-ONNX)
//!
//! Formes dynamiques (N keypoints adaptatif) : on extrait UNE fois par frame
//! (28 extractions pour 28 frames) et on ne paie le transformer que par
//! paire. Les exports « pipeline » fusionnés v2/v3 recalculeraient
//! l'extraction pour les DEUX images de CHAQUE paire (~80 extractions pour
//! 28 frames — rédhibitoire sur RPi).

#![cfg(feature = "pose-model")]

use std::sync::Mutex;

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use thiserror::Error;

/// Erreurs du modèle de pose.
#[derive(Debug, Error)]
pub enum PoseModelError {
    /// Fichier .onnx absent ou session infaisable.
    #[error("chargement : {0}")]
    Load(String),
    /// Échec d'inférence.
    #[error("inférence : {0}")]
    Inference(String),
}

/// Modèle de pose : sessions SuperPoint (extracteur) + LightGlue (matcher).
/// `Mutex` car `ort` rc.13 exige `&mut Session` pour `run` (l'API C est
/// thread-safe, le binding ne l'expose pas) — dans un job le modèle est
/// utilisé séquentiellement, entre jobs chaque worker a son instance.
pub struct PoseModel {
    extractor: Mutex<Session>,
    matcher: Mutex<Session>,
}

/// Features d'une frame : keypoints en pixels de l'image de travail
/// (`w`×`h`, multiple de 8) + descripteurs 256-dim (N×256, row-major).
#[derive(Debug, Clone)]
pub struct Features {
    /// Largeur de l'image de travail (px).
    pub w: u32,
    /// Hauteur de l'image de travail (px).
    pub h: u32,
    /// Keypoints (u, v) en pixels de l'image de travail.
    pub kpts_px: Vec<[f32; 2]>,
    /// Descripteurs N×256 row-major.
    pub desc: Vec<f32>,
}

/// Une correspondance : indices dans `a.kpts_px` et `b.kpts_px`.
#[derive(Debug, Clone, Copy)]
pub struct PointMatch {
    pub ia: usize,
    pub ib: usize,
    /// Score de confiance LightGlue (0-1).
    pub score: f32,
}

impl PoseModel {
    /// Charge les deux sessions depuis `dir` (`superpoint.onnx` +
    /// `superpoint_lightglue_fused_cpu.onnx`). Optimisation Level1 : les
    /// exports sont déjà fused — Level3 n'apporte rien et ralentit le
    /// chargement.
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

    /// Extrait keypoints + descripteurs d'une frame RGB8. L'image de travail
    /// garde l'aspect, plus grand côté ≤ `work_max_px`, bornes arrondies à
    /// un multiple de 8 (pooling SuperPoint).
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
        // (1, N, 256) → N×256 row-major (déjà contigu, copie plane).
        d.copy_from_slice(&desc[..n * dim]);

        Ok(Features {
            w,
            h,
            kpts_px,
            desc: d,
        })
    }

    /// Correspondances LightGlue d'une paire de frames (indices + score).
    pub fn match_pair(
        &self,
        a: &Features,
        b: &Features,
    ) -> Result<Vec<PointMatch>, PoseModelError> {
        let (na, nb) = (a.kpts_px.len(), b.kpts_px.len());
        if na < 8 || nb < 8 {
            return Ok(Vec::new());
        }
        // Keypoints NORMALISÉS [0,1] par axe (mesuré A/B sur capture réelle :
        // 567 matches normalisés vs 44 en pixels bruts — l'export v1 "fused"
        // attend la normalisation, la tailles d'image n'étant pas une entrée).
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

        // matches0 (M, 2) : indices (ia, ib) déjà filtrés (NonZero).
        let m = mshape[0] as usize;
        let mut out = Vec::with_capacity(m);
        for k in 0..m {
            let (ia, ib) = (matches[2 * k], matches[2 * k + 1]);
            if ia < 0 || ib < 0 {
                continue; // -1 = pas de correspondance (défensif : l'export
                          // NonZero est censé les filtrer)
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

/// Dimensions de travail : plus grand côté ≤ `max_px`, les DEUX côtés
/// arrondis à un multiple de 8 (pooling /32 de SuperPoint : H, W doivent
/// être divisibles par 8 en sortie des strides cumulés), minimum 64.
#[must_use]
pub fn work_size(width: u32, height: u32, max_px: u32) -> (u32, u32) {
    let big = width.max(height);
    let small = width.min(height) as f32;
    // Jamais d'agrandissement : long = min(max_px, plus grand côté).
    let long = max_px.min(big).max(64);
    let short = ((long as f32 * small / big as f32).round() as u32).clamp(64, long);
    let round8 = |v: u32| ((v / 8) * 8).max(64);
    if width >= height {
        (round8(long), round8(short))
    } else {
        (round8(short), round8(long))
    }
}

/// RGB8 → niveaux de gris f32 [0,1] par sous-échantillonnage moyenneur
/// (box) — anti-aliasé, aucune dep (`image` n'a pas le resize sans
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

    /// work_size : aspect portrait 1080×1920 à max 896 → 504×896 (les deux
    /// multiples de 8) ; paysage inverse ; minimum borné.
    #[test]
    fn taille_travail() {
        assert_eq!(work_size(1080, 1920, 896), (504, 896));
        assert_eq!(work_size(1920, 1080, 896), (896, 504));
        assert_eq!(work_size(64, 64, 896), (64, 64));
        assert_eq!(work_size(1080, 1920, 512), (288, 512));
    }

    /// resize_gray : constante partout = constante partout, moyenne d'un
    /// dégradé préservée, pas de débordement [0,1].
    #[test]
    fn gris_moyenne() {
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
        // Moyenne globale préservée (dégradé linéaire 0..252 → ~0.5).
        let m = g.iter().sum::<f32>() / g.len() as f32;
        assert!((m - 0.492).abs() < 0.05, "moyenne = {m}");
    }
}
