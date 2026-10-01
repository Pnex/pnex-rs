//! Vision contract (camera-video.md D81–D83) — model registry DTOs,
//! inference spec and detections. Pure, wasm-safe.

use serde::{Deserialize, Serialize};

/// Pre/post-processing family of a detector — fixes the input layout, the
/// output decoding and the default input size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionFamily {
    /// YOLOX (Apache-2.0, Megvii): letterbox top-left padded with 114, BGR
    /// 0–255 NCHW, raw head output `[1, N, 5 + classes]` decoded on
    /// strides 8/16/32.
    #[default]
    Yolox,
}

impl VisionFamily {
    pub const ALL: [VisionFamily; 1] = [Self::Yolox];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Yolox => "yolox",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.wire() == s)
    }
}

/// Model task — `detection` only in V1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionTask {
    #[default]
    Detection,
}

/// Inference spec of a registered model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSpec {
    pub family: VisionFamily,
    pub input_width: u32,
    pub input_height: u32,
    /// Class names by index (COCO-80 by default).
    pub labels: Vec<String>,
    pub score_threshold: f32,
    pub nms_iou: f32,
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            family: VisionFamily::Yolox,
            input_width: 416,
            input_height: 416,
            labels: coco_labels(),
            score_threshold: 0.35,
            nms_iou: 0.45,
        }
    }
}

impl ModelSpec {
    /// Field-level validation (machine tokens).
    pub fn check(&self) -> Result<(), Vec<(&'static str, String)>> {
        let mut errs = Vec::new();
        let dim_ok = |d: u32| (32..=2048).contains(&d) && d % 32 == 0;
        if !dim_ok(self.input_width) {
            errs.push(("input_width", "range:32..2048/32".to_string()));
        }
        if !dim_ok(self.input_height) {
            errs.push(("input_height", "range:32..2048/32".to_string()));
        }
        if self.labels.is_empty() || self.labels.len() > 1000 {
            errs.push(("labels", "range:1..1000".to_string()));
        }
        if !(0.0..=1.0).contains(&self.score_threshold) {
            errs.push(("score_threshold", "range:0..1".to_string()));
        }
        if !(0.0..=1.0).contains(&self.nms_iou) {
            errs.push(("nms_iou", "range:0..1".to_string()));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// One detection, in **source image pixels**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    pub label: String,
    pub class_id: u32,
    pub score: f32,
    /// `[x, y, width, height]`, top-left origin.
    pub bbox: [f32; 4],
}

/// Result of one inference (test endpoint, flow node).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectionResult {
    pub width: u32,
    pub height: u32,
    pub took_ms: u64,
    pub detections: Vec<Detection>,
}

/// Lowest score the inference keeps before any threshold filtering (D104):
/// callers filter afterwards, and "seen but below the threshold" stays
/// observable instead of vanishing inside the decoder.
pub const DETECT_FLOOR: f32 = 0.05;

/// What an ONNX file declares about itself (D100) — `None` = dynamic or
/// unreadable dimension.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelInspection {
    #[serde(default)]
    pub input_width: Option<u32>,
    #[serde(default)]
    pub input_height: Option<u32>,
    /// Class count read from the output head (`C − 5` for YOLOX).
    #[serde(default)]
    pub classes: Option<u32>,
}

impl ModelInspection {
    /// Input size imposed by the file, when both dimensions are fixed.
    pub fn fixed_input(&self) -> Option<(u32, u32)> {
        Some((self.input_width?, self.input_height?))
    }
}

/// Outcome of the load + test-inference check of a model (D100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCheckStatus {
    /// Registered before D100, never checked.
    #[default]
    Unchecked,
    Valid,
    Invalid,
}

impl ModelCheckStatus {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Unchecked => "unchecked",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
        }
    }

    pub fn from_wire(s: &str) -> Self {
        match s {
            "valid" => Self::Valid,
            "invalid" => Self::Invalid,
            _ => Self::Unchecked,
        }
    }
}

/// Stored check of a model: status, runtime diagnostic, measured speed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCheck {
    #[serde(default)]
    pub status: ModelCheckStatus,
    /// Loader/inference diagnostic (verbatim runtime text) when invalid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Test inference time on this server, ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<String>,
}

impl ModelCheck {
    /// Sustainable inference rate (images/s) from the measured time.
    pub fn max_fps(&self) -> Option<f64> {
        self.infer_ms.map(|ms| 1000.0 / (ms.max(1) as f64))
    }
}

/// Camera state reported by the live test (D104).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveCameraState {
    pub connected: bool,
    /// `continuous` | `on_demand`.
    pub capture_mode: String,
    /// Frame size code (`qvga`, `vga`…).
    pub framesize: String,
    pub fps: u8,
}

/// One live-test round (D104): the analysed frame, every detection above
/// [`DETECT_FLOOR`], the threshold the model applies and machine warnings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveTestResult {
    /// JPEG bytes, base64 (standard alphabet).
    pub frame_b64: String,
    pub frame_seq: u32,
    /// Age of the frame when analysed, ms.
    pub frame_age_ms: i64,
    pub result: DetectionResult,
    pub threshold: f32,
    pub camera: LiveCameraState,
    /// Machine codes (`camera-frame-stale`, `image-too-dark`…).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Registered model (`GET /api/v1/ml/models`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MlModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub task: VisionTask,
    /// Media asset holding the ONNX bytes (kind `model`).
    pub asset_id: String,
    /// Pinned media version (`None` = the asset's current version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_version: Option<i64>,
    pub spec: ModelSpec,
    /// Load + test-inference check (D100).
    #[serde(default)]
    pub check: ModelCheck,
    pub created_at: String,
    pub updated_at: String,
}

/// `DebugMessage.format` marking a node status on the engine debug channel
/// (D103) — the runtime forwards it as feed source `pnex-status`.
pub const NODE_STATUS_FORMAT: &str = "pnex-status";

/// Severity of a node status (badge colour in the editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeStatusLevel {
    #[default]
    Ok,
    Warn,
    Error,
}

/// Status published by the camera/vision nodes (D103): a machine `code`
/// (UI key `flow-status-<code>`), an optional verbatim runtime `detail` and
/// counters since the node started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeStatus {
    pub level: NodeStatusLevel,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub stats: std::collections::BTreeMap<String, u64>,
}

impl NodeStatus {
    pub fn new(level: NodeStatusLevel, code: &str) -> Self {
        Self {
            level,
            code: code.to_string(),
            ..Default::default()
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn stat(mut self, key: &str, value: u64) -> Self {
        self.stats.insert(key.to_string(), value);
        self
    }
}

/// COCO-80 class names (YOLOX/COCO order).
pub fn coco_labels() -> Vec<String> {
    COCO_80.iter().map(|s| (*s).to_string()).collect()
}

pub const COCO_80: [&str; 80] = [
    "person",
    "bicycle",
    "car",
    "motorcycle",
    "airplane",
    "bus",
    "train",
    "truck",
    "boat",
    "traffic light",
    "fire hydrant",
    "stop sign",
    "parking meter",
    "bench",
    "bird",
    "cat",
    "dog",
    "horse",
    "sheep",
    "cow",
    "elephant",
    "bear",
    "zebra",
    "giraffe",
    "backpack",
    "umbrella",
    "handbag",
    "tie",
    "suitcase",
    "frisbee",
    "skis",
    "snowboard",
    "sports ball",
    "kite",
    "baseball bat",
    "baseball glove",
    "skateboard",
    "surfboard",
    "tennis racket",
    "bottle",
    "wine glass",
    "cup",
    "fork",
    "knife",
    "spoon",
    "bowl",
    "banana",
    "apple",
    "sandwich",
    "orange",
    "broccoli",
    "carrot",
    "hot dog",
    "pizza",
    "donut",
    "cake",
    "chair",
    "couch",
    "potted plant",
    "bed",
    "dining table",
    "toilet",
    "tv",
    "laptop",
    "mouse",
    "remote",
    "keyboard",
    "cell phone",
    "microwave",
    "oven",
    "toaster",
    "sink",
    "refrigerator",
    "book",
    "clock",
    "vase",
    "scissors",
    "teddy bear",
    "hair drier",
    "toothbrush",
];

/// Config of the `vision-detect` flow node (D83).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionDetectConfig {
    /// `ml_models.id` (UUID string).
    pub model_id: String,
    /// Keep only these labels (empty = all).
    #[serde(default)]
    pub labels: Vec<String>,
    /// Overrides the model threshold when > 0.
    #[serde(default)]
    pub min_score: f32,
    /// `on_detection` (default): emit only when something matched.
    #[serde(default)]
    pub emit: VisionEmit,
    /// Inference rate cap per camera (0 = every incoming frame).
    #[serde(default = "default_detect_fps")]
    pub max_fps: f64,
    /// Store the matched boxes in OpenObserve as an annotation layer of the
    /// camera (D105) — drawn over the recordings in Cameras, boxes only.
    #[serde(default)]
    pub record_layer: bool,
}

fn default_detect_fps() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisionEmit {
    #[default]
    OnDetection,
    Always,
}

impl VisionDetectConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.model_id.trim().is_empty() {
            return Some(("vision_model_missing", "select a model".into()));
        }
        if !(0.0..=1.0).contains(&self.min_score) {
            return Some((
                "vision_score_invalid",
                "min score must be between 0 and 1".into(),
            ));
        }
        if !self.max_fps.is_finite()
            || self.max_fps < 0.0
            || self.max_fps > crate::CAMERA_NODE_MAX_FPS
        {
            return Some((
                "camera_fps_invalid",
                format!(
                    "max fps must be between 0 (every frame) and {}",
                    crate::CAMERA_NODE_MAX_FPS
                ),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_is_valid() {
        let s = ModelSpec::default();
        assert!(s.check().is_ok());
        assert_eq!(s.labels.len(), 80);
        assert_eq!(s.labels[0], "person");
    }

    #[test]
    fn spec_rejects_bad_dims() {
        let s = ModelSpec {
            input_width: 100,
            score_threshold: 2.0,
            ..Default::default()
        };
        let errs = s.check().unwrap_err();
        assert!(errs.iter().any(|(f, _)| *f == "input_width"));
        assert!(errs.iter().any(|(f, _)| *f == "score_threshold"));
    }

    #[test]
    fn detect_config_check() {
        let c = VisionDetectConfig {
            model_id: String::new(),
            labels: vec![],
            min_score: 0.0,
            emit: VisionEmit::OnDetection,
            max_fps: 1.0,
            record_layer: false,
        };
        assert_eq!(c.check().unwrap().0, "vision_model_missing");
    }
}
