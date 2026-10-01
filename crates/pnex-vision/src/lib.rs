//! PNeX vision inference (camera-video.md D82) — pure-Rust ONNX detectors
//! on tract, shared by the backend model test endpoint and the flow
//! `vision-detect` node.
//!
//! V1 family: YOLOX (Apache-2.0). Pipeline: decode JPEG/PNG → letterbox
//! (top-left, pad 114) to the model input → BGR 0–255 NCHW → run →
//! grid decode (strides 8/16/32) → score = objectness × class → per-class
//! NMS → boxes scaled back to source pixels.

use std::io::Cursor;
use std::time::Instant;

use pnex_core::vision::{Detection, DetectionResult, ModelInspection, ModelSpec, VisionFamily};
use tract_onnx::prelude::*;
use tract_onnx::tract_hir::infer::Factoid;

#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    #[error("model load failed: {0}")]
    Load(String),
    #[error("image decode failed: {0}")]
    Image(String),
    #[error("inference failed: {0}")]
    Run(String),
    #[error("unexpected model output shape {0:?}")]
    Output(Vec<usize>),
    #[error("the model file expects a {}x{} input, the spec says {}x{}", file.0, file.1, spec.0, spec.1)]
    InputMismatch { file: (u32, u32), spec: (u32, u32) },
}

type Plan = std::sync::Arc<TypedRunnableModel>;

/// Fallback input size used to type a model whose input is dynamic, only to
/// read its output head during [`inspect`].
const PROBE_SIZE: usize = 416;

/// Parses an ONNX file (no optimization).
fn read_model(onnx: &[u8]) -> Result<InferenceModel, VisionError> {
    tract_onnx::onnx()
        .model_for_read(&mut Cursor::new(onnx))
        .map_err(|e| VisionError::Load(e.to_string()))
}

/// Fixed `(width, height)` of an NCHW input, when the file pins both.
fn declared_input(model: &InferenceModel) -> Option<(u32, u32)> {
    let fact = model.input_fact(0).ok()?;
    let dim = |i: usize| -> Option<u32> {
        fact.shape
            .dim(i)?
            .concretize()?
            .as_i64()
            .and_then(|d| u32::try_from(d).ok())
    };
    Some((dim(3)?, dim(2)?))
}

/// What the ONNX file declares (D100): fixed input size and class count of
/// the output head. Dynamic dimensions come back as `None`.
pub fn inspect(onnx: &[u8]) -> Result<ModelInspection, VisionError> {
    let model = read_model(onnx)?;
    let fixed = declared_input(&model);
    let (w, h) = fixed.map_or((PROBE_SIZE, PROBE_SIZE), |(w, h)| (w as usize, h as usize));
    let classes = model
        .with_input_fact(0, f32::fact([1, 3, h, w]).into())
        .and_then(|m| m.into_typed())
        .ok()
        .and_then(|typed| {
            let fact = typed.output_fact(0).ok()?;
            let dims = fact.shape.as_concrete()?;
            // YOLOX head: [1, N, 5 + classes].
            (dims.len() == 3 && dims[2] > 5).then(|| (dims[2] - 5) as u32)
        });
    Ok(ModelInspection {
        input_width: fixed.map(|f| f.0),
        input_height: fixed.map(|f| f.1),
        classes,
    })
}

/// Measured outcome of [`validate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    pub load_ms: u64,
    pub infer_ms: u64,
}

/// Loads the model with `spec` and runs one inference on a neutral image at
/// the input size (D100): proves the pair file + spec actually works on
/// this machine and measures its speed.
pub fn validate(onnx: &[u8], spec: ModelSpec) -> Result<Validation, VisionError> {
    let started = Instant::now();
    let det = Detector::load(onnx, spec)?;
    let load_ms = started.elapsed().as_millis() as u64;
    let img = image::RgbImage::from_pixel(
        det.spec.input_width,
        det.spec.input_height,
        image::Rgb([114, 114, 114]),
    );
    let res = det.detect_image(&img, 1.0)?;
    Ok(Validation {
        load_ms,
        infer_ms: res.took_ms,
    })
}

/// Mean luma (0–255, BT.601) of an encoded image — "too dark to detect"
/// diagnostics of the live test.
pub fn mean_luma(encoded: &[u8]) -> Option<f32> {
    let img = image::load_from_memory(encoded).ok()?.to_rgb8();
    let n = (img.width() as u64 * img.height() as u64).max(1);
    let sum: f64 = img
        .pixels()
        .map(|p| 0.299 * f64::from(p[0]) + 0.587 * f64::from(p[1]) + 0.114 * f64::from(p[2]))
        .sum();
    Some((sum / n as f64) as f32)
}

/// A loaded, optimized detector — `Send + Sync`, cheap to share behind an
/// `Arc` (tract plans are immutable; `run` allocates its own state).
pub struct Detector {
    plan: Plan,
    spec: ModelSpec,
}

impl Detector {
    /// Loads and optimizes an ONNX model for a fixed input size. A file that
    /// pins another input size is refused with an explicit message (tract
    /// alone fails deep in the graph, e.g. on a `Slice` node).
    pub fn load(onnx: &[u8], spec: ModelSpec) -> Result<Self, VisionError> {
        let model = read_model(onnx)?;
        if let Some((fw, fh)) = declared_input(&model) {
            if (fw, fh) != (spec.input_width, spec.input_height) {
                return Err(VisionError::InputMismatch {
                    file: (fw, fh),
                    spec: (spec.input_width, spec.input_height),
                });
            }
        }
        let (w, h) = (spec.input_width as usize, spec.input_height as usize);
        let plan = model
            .with_input_fact(0, f32::fact([1, 3, h, w]).into())
            .map_err(|e| VisionError::Load(e.to_string()))?
            .into_optimized()
            .map_err(|e| VisionError::Load(e.to_string()))?
            .into_runnable()
            .map_err(|e| VisionError::Load(e.to_string()))?;
        Ok(Self { plan, spec })
    }

    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    /// Runs the detector on an encoded image (JPEG/PNG), keeping detections
    /// at or above the model threshold.
    pub fn detect(&self, encoded: &[u8]) -> Result<DetectionResult, VisionError> {
        self.detect_with_floor(encoded, self.spec.score_threshold)
    }

    /// Same as [`Detector::detect`] with an explicit score floor — callers
    /// that must see what falls below the model threshold pass
    /// [`pnex_core::vision::DETECT_FLOOR`] and filter afterwards.
    pub fn detect_with_floor(
        &self,
        encoded: &[u8],
        floor: f32,
    ) -> Result<DetectionResult, VisionError> {
        let img = image::load_from_memory(encoded)
            .map_err(|e| VisionError::Image(e.to_string()))?
            .to_rgb8();
        self.detect_image(&img, floor)
    }

    fn detect_image(
        &self,
        img: &image::RgbImage,
        floor: f32,
    ) -> Result<DetectionResult, VisionError> {
        let started = Instant::now();
        let (src_w, src_h) = img.dimensions();
        let (input, ratio) = letterbox_bgr(img, self.spec.input_width, self.spec.input_height);
        let outputs = self
            .plan
            .run(tvec!(input.into()))
            .map_err(|e| VisionError::Run(e.to_string()))?;
        let out = outputs[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| VisionError::Run(e.to_string()))?;
        let shape = out.shape().to_vec();
        let detections = match self.spec.family {
            VisionFamily::Yolox => {
                if shape.len() != 3 || shape[0] != 1 || shape[2] < 6 {
                    return Err(VisionError::Output(shape));
                }
                let flat: Vec<f32> = out.iter().copied().collect();
                decode_yolox(
                    &flat, shape[1], shape[2], &self.spec, floor, ratio, src_w, src_h,
                )
            }
        };
        Ok(DetectionResult {
            width: src_w,
            height: src_h,
            took_ms: started.elapsed().as_millis() as u64,
            detections,
        })
    }
}

/// Letterbox to `w×h` (top-left aligned, pad 114), BGR 0–255 NCHW tensor.
/// Returns the tensor and the resize ratio (source → input).
fn letterbox_bgr(img: &image::RgbImage, w: u32, h: u32) -> (Tensor, f32) {
    let (sw, sh) = img.dimensions();
    let ratio = (w as f32 / sw as f32).min(h as f32 / sh as f32);
    let (nw, nh) = (
        ((sw as f32 * ratio) as u32).clamp(1, w),
        ((sh as f32 * ratio) as u32).clamp(1, h),
    );
    let resized = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
    let (w, h) = (w as usize, h as usize);
    let plane = w * h;
    let mut data = vec![114.0f32; 3 * plane];
    for (x, y, px) in resized.enumerate_pixels() {
        let i = y as usize * w + x as usize;
        // BGR channel order (OpenCV training pipeline).
        data[i] = f32::from(px[2]);
        data[plane + i] = f32::from(px[1]);
        data[2 * plane + i] = f32::from(px[0]);
    }
    let tensor = tract_ndarray::Array4::from_shape_vec((1, 3, h, w), data)
        .expect("shape matches buffer")
        .into();
    (tensor, ratio)
}

/// YOLOX head decoding + score floor + per-class NMS.
#[allow(clippy::too_many_arguments)]
fn decode_yolox(
    out: &[f32],
    n: usize,
    stride_len: usize,
    spec: &ModelSpec,
    floor: f32,
    ratio: f32,
    src_w: u32,
    src_h: u32,
) -> Vec<Detection> {
    let (iw, ih) = (spec.input_width as usize, spec.input_height as usize);
    // Grid cells in head order: stride 8, then 16, then 32.
    let mut grid: Vec<(f32, f32, f32)> = Vec::with_capacity(n);
    for s in [8usize, 16, 32] {
        let (gw, gh) = (iw / s, ih / s);
        for gy in 0..gh {
            for gx in 0..gw {
                grid.push((gx as f32, gy as f32, s as f32));
            }
        }
    }
    if grid.len() != n {
        // Model exported with a different head layout (e.g. p6): refuse
        // rather than emit shifted boxes.
        return Vec::new();
    }
    let classes = stride_len - 5;
    let mut candidates: Vec<Detection> = Vec::new();
    for (i, (gx, gy, s)) in grid.into_iter().enumerate() {
        let row = &out[i * stride_len..(i + 1) * stride_len];
        let obj = row[4];
        if obj < floor {
            continue;
        }
        let (mut best, mut best_score) = (0usize, 0.0f32);
        for (c, &p) in row[5..].iter().enumerate() {
            if p > best_score {
                best = c;
                best_score = p;
            }
        }
        let score = obj * best_score;
        if score < floor || best >= classes {
            continue;
        }
        let cx = (row[0] + gx) * s;
        let cy = (row[1] + gy) * s;
        let bw = row[2].exp() * s;
        let bh = row[3].exp() * s;
        // Back to source pixels, clamped to the image.
        let x0 = ((cx - bw / 2.0) / ratio).clamp(0.0, src_w as f32);
        let y0 = ((cy - bh / 2.0) / ratio).clamp(0.0, src_h as f32);
        let x1 = ((cx + bw / 2.0) / ratio).clamp(0.0, src_w as f32);
        let y1 = ((cy + bh / 2.0) / ratio).clamp(0.0, src_h as f32);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        candidates.push(Detection {
            label: spec
                .labels
                .get(best)
                .cloned()
                .unwrap_or_else(|| format!("class_{best}")),
            class_id: best as u32,
            score,
            bbox: [x0, y0, x1 - x0, y1 - y0],
        });
    }
    nms(candidates, spec.nms_iou)
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let (ax1, ay1, bx1, by1) = (a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]);
    let iw = (ax1.min(bx1) - a[0].max(b[0])).max(0.0);
    let ih = (ay1.min(by1) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// Greedy per-class non-maximum suppression, highest score first.
fn nms(mut dets: Vec<Detection>, thr: f32) -> Vec<Detection> {
    dets.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in dets {
        if kept
            .iter()
            .all(|k| k.class_id != d.class_id || iou(&k.bbox, &d.bbox) < thr)
        {
            kept.push(d);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(class_id: u32, score: f32, bbox: [f32; 4]) -> Detection {
        Detection {
            label: String::new(),
            class_id,
            score,
            bbox,
        }
    }

    #[test]
    fn nms_keeps_best_per_class() {
        let kept = nms(
            vec![
                det(0, 0.9, [0.0, 0.0, 10.0, 10.0]),
                det(0, 0.8, [1.0, 1.0, 10.0, 10.0]),
                det(1, 0.7, [1.0, 1.0, 10.0, 10.0]),
                det(0, 0.6, [50.0, 50.0, 10.0, 10.0]),
            ],
            0.45,
        );
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[0].score, 0.9);
    }

    #[test]
    fn iou_basics() {
        assert!((iou(&[0.0, 0.0, 10.0, 10.0], &[0.0, 0.0, 10.0, 10.0]) - 1.0).abs() < 1e-6);
        assert_eq!(iou(&[0.0, 0.0, 1.0, 1.0], &[5.0, 5.0, 1.0, 1.0]), 0.0);
    }

    fn test_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(
            std::env::var("PNEX_VISION_TEST_DIR").expect("PNEX_VISION_TEST_DIR"),
        )
    }

    /// The nano release pins 416x416 and 80 COCO classes; loading it with
    /// another size must name both sizes instead of a tract graph error.
    #[test]
    #[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx"]
    fn inspect_reads_the_fixed_input_and_classes() {
        let onnx = std::fs::read(test_dir().join("yolox_nano.onnx")).expect("model");
        let info = inspect(&onnx).expect("inspect");
        assert_eq!(info.fixed_input(), Some((416, 416)));
        assert_eq!(info.classes, Some(80));
        let spec = ModelSpec {
            input_width: 640,
            input_height: 640,
            ..Default::default()
        };
        match Detector::load(&onnx, spec) {
            Err(VisionError::InputMismatch { file, spec }) => {
                assert_eq!((file, spec), ((416, 416), (640, 640)));
            }
            Err(e) => panic!("unexpected error {e}"),
            Ok(_) => panic!("a 640 spec must not load a 416 model"),
        }
    }

    #[test]
    #[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx"]
    fn validate_measures_a_working_model() {
        let onnx = std::fs::read(test_dir().join("yolox_nano.onnx")).expect("model");
        let v = validate(&onnx, ModelSpec::default()).expect("valid");
        eprintln!("{v:?}");
        assert!(v.infer_ms < 10_000);
    }

    #[test]
    fn mean_luma_of_flat_images() {
        let mut buf = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0]))
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode");
        assert!(mean_luma(&buf).expect("luma") < 1.0);
        let mut buf = Vec::new();
        image::RgbImage::from_pixel(8, 8, image::Rgb([255, 255, 255]))
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode");
        assert!(mean_luma(&buf).expect("luma") > 254.0);
    }

    /// Real model check — set `PNEX_VISION_TEST_DIR` to a folder holding
    /// `yolox_nano.onnx` (YOLOX 0.1.1rc0 release) and `dog.jpg` (YOLOX repo
    /// asset): expects a dog, a bicycle and a car/truck.
    #[test]
    #[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx + dog.jpg"]
    fn yolox_nano_detects_the_reference_scene() {
        let dir = std::path::PathBuf::from(std::env::var("PNEX_VISION_TEST_DIR").expect("dir"));
        let onnx = std::fs::read(dir.join("yolox_nano.onnx")).expect("model");
        let jpg = std::fs::read(dir.join("dog.jpg")).expect("image");
        let det = Detector::load(&onnx, ModelSpec::default()).expect("load");
        let res = det.detect(&jpg).expect("detect");
        let labels: Vec<&str> = res.detections.iter().map(|d| d.label.as_str()).collect();
        eprintln!("took {} ms: {:?}", res.took_ms, res.detections);
        assert_eq!((res.width, res.height), (768, 576));
        assert!(labels.contains(&"dog"), "{labels:?}");
        assert!(labels.contains(&"bicycle"), "{labels:?}");
        assert!(
            labels.iter().any(|l| *l == "car" || *l == "truck"),
            "{labels:?}"
        );
    }
}
