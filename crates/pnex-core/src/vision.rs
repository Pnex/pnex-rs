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
    /// Vision Lab project receiving the doubtful frames (M4, active
    /// learning); empty = off.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub review_project: String,
    /// A frame is doubtful when a candidate score lies in
    /// `[review_min, review_max]`.
    #[serde(default = "default_review_min")]
    pub review_min: f32,
    #[serde(default = "default_review_max")]
    pub review_max: f32,
    /// At most one doubtful frame per camera this often.
    #[serde(default = "default_review_every")]
    pub review_every_secs: f64,
}

fn default_detect_fps() -> f64 {
    1.0
}

pub fn default_review_min() -> f32 {
    0.25
}

pub fn default_review_max() -> f32 {
    0.6
}

pub fn default_review_every() -> f64 {
    60.0
}

/// Bounds of `review_every_secs`.
pub const REVIEW_EVERY_SECS: (f64, f64) = (5.0, 3600.0);

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
        if !self.review_project.is_empty() {
            if uuid::Uuid::parse_str(self.review_project.trim()).is_err() {
                return Some((
                    "vision_review_project_invalid",
                    "pick a Vision Lab project".into(),
                ));
            }
            let band_ok = (0.0..=1.0).contains(&self.review_min)
                && (0.0..=1.0).contains(&self.review_max)
                && self.review_min < self.review_max;
            if !band_ok {
                return Some((
                    "vision_review_band_invalid",
                    "the doubt band must be 0 ≤ min < max ≤ 1".into(),
                ));
            }
            let (lo, hi) = REVIEW_EVERY_SECS;
            if !self.review_every_secs.is_finite() || !(lo..=hi).contains(&self.review_every_secs) {
                return Some((
                    "vision_review_every_invalid",
                    format!("send at most one frame every {lo} to {hi} s"),
                ));
            }
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

/// Config of the `vision_track` flow node (D192): per-source object
/// tracking over `vision_detect` output (ByteTrack-style two-stage IoU
/// association), stable `track_id` per object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionTrackConfig {
    /// Detections at or above this score open and confirm tracks; lower
    /// ones only extend existing tracks (ByteTrack second stage).
    #[serde(default = "default_track_high_score")]
    pub high_score: f32,
    /// Minimum box overlap (IoU) to match a detection to a track.
    #[serde(default = "default_track_min_iou")]
    pub min_iou: f32,
    /// Matched frames before a track is confirmed (`appear` event).
    #[serde(default = "default_track_min_hits")]
    pub min_hits: u32,
    /// A track unseen for this long is dropped (`disappear` event).
    #[serde(default = "default_track_lost_secs")]
    pub lost_after_secs: f64,
}

fn default_track_high_score() -> f32 {
    0.5
}
fn default_track_min_iou() -> f32 {
    0.2
}
fn default_track_min_hits() -> u32 {
    2
}
fn default_track_lost_secs() -> f64 {
    3.0
}

impl Default for VisionTrackConfig {
    fn default() -> Self {
        Self {
            high_score: default_track_high_score(),
            min_iou: default_track_min_iou(),
            min_hits: default_track_min_hits(),
            lost_after_secs: default_track_lost_secs(),
        }
    }
}

/// Upper bound of `lost_after_secs` and `min_dwell_secs` (one hour).
pub const TRACK_LOST_MAX_SECS: f64 = 3600.0;

impl VisionTrackConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        let unit = |v: f32| (0.0..=1.0).contains(&v);
        if !unit(self.high_score) || !unit(self.min_iou) || self.min_iou == 0.0 {
            return Some((
                "vision_track_invalid",
                "high score must be in 0..1 and min IoU in ]0..1]".into(),
            ));
        }
        if !(1..=100).contains(&self.min_hits)
            || !self.lost_after_secs.is_finite()
            || !(0.1..=TRACK_LOST_MAX_SECS).contains(&self.lost_after_secs)
        {
            return Some((
                "vision_track_invalid",
                format!(
                    "min hits must be in 1..100 and lost after in 0.1..{TRACK_LOST_MAX_SECS} s"
                ),
            ));
        }
        None
    }
}

/// Point of a box tested against the zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZoneAnchor {
    /// Bottom centre (feet of a person, wheels of a vehicle).
    #[default]
    Bottom,
    Center,
}

/// One zone: a polygon in normalized image coordinates (0–1, top-left
/// origin), so it survives a resolution change of the camera (D193).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionZone {
    pub name: String,
    /// `[x, y]` vertices, 3..=[`ZONE_MAX_POINTS`].
    pub points: Vec<[f32; 2]>,
}

pub const ZONE_MAX_POINTS: usize = 32;
pub const ZONE_MAX_COUNT: usize = 16;

/// Config of the `vision_zone` flow node (D193): enter / exit events of
/// tracked objects in image zones, with the zone occupancy.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VisionZoneConfig {
    #[serde(default)]
    pub zones: Vec<VisionZone>,
    /// Keep only these labels (empty = all).
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub anchor: ZoneAnchor,
    /// Time an object must stay in a zone before `enter` fires (0 = at
    /// once) — "present for more than N s".
    #[serde(default)]
    pub min_dwell_secs: f64,
    /// Camera shown behind the zone editor (`device_registries.id`) —
    /// editor only, the runtime applies the zones to every source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_device: Option<i64>,
}

impl VisionZoneConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.zones.is_empty() {
            return Some(("vision_zone_empty", "draw at least one zone".into()));
        }
        if self.zones.len() > ZONE_MAX_COUNT {
            return Some((
                "vision_zone_invalid",
                format!("at most {ZONE_MAX_COUNT} zones"),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for z in &self.zones {
            let name = z.name.trim();
            if name.is_empty() || name.chars().count() > 64 || !names.insert(name) {
                return Some((
                    "vision_zone_invalid",
                    "each zone needs a unique name (1..64 chars)".into(),
                ));
            }
            let in_unit = |p: &[f32; 2]| p.iter().all(|c| (0.0..=1.0).contains(c));
            if !(3..=ZONE_MAX_POINTS).contains(&z.points.len()) || !z.points.iter().all(in_unit) {
                return Some((
                    "vision_zone_invalid",
                    format!("zone {name}: 3..{ZONE_MAX_POINTS} points inside the image"),
                ));
            }
        }
        if !self.min_dwell_secs.is_finite()
            || !(0.0..=TRACK_LOST_MAX_SECS).contains(&self.min_dwell_secs)
        {
            return Some((
                "vision_zone_invalid",
                format!("min dwell must be in 0..{TRACK_LOST_MAX_SECS} s"),
            ));
        }
        None
    }
}

/// Even-odd point-in-polygon test (normalized coordinates).
pub fn point_in_polygon(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_config_check() {
        assert!(VisionTrackConfig::default().check().is_none());
        let bad = VisionTrackConfig {
            min_iou: 0.0,
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "vision_track_invalid");
        let bad = VisionTrackConfig {
            lost_after_secs: 0.0,
            ..Default::default()
        };
        assert_eq!(bad.check().unwrap().0, "vision_track_invalid");
    }

    #[test]
    fn zone_config_check() {
        let zone = |name: &str, points: Vec<[f32; 2]>| VisionZone {
            name: name.into(),
            points,
        };
        let square = vec![[0.1, 0.1], [0.5, 0.1], [0.5, 0.5], [0.1, 0.5]];
        let mut c = VisionZoneConfig::default();
        assert_eq!(c.check().unwrap().0, "vision_zone_empty");
        c.zones = vec![zone("dock", square.clone())];
        assert!(c.check().is_none());
        c.zones.push(zone("dock", square.clone()));
        assert_eq!(
            c.check().unwrap().0,
            "vision_zone_invalid",
            "duplicate name"
        );
        c.zones = vec![zone("a", vec![[0.0, 0.0], [1.0, 0.0]])];
        assert_eq!(c.check().unwrap().0, "vision_zone_invalid", "two points");
        c.zones = vec![zone("a", vec![[0.0, 0.0], [1.2, 0.0], [0.5, 0.5]])];
        assert_eq!(c.check().unwrap().0, "vision_zone_invalid", "outside");
    }

    #[test]
    fn polygon_membership() {
        let tri = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!(point_in_polygon([0.2, 0.2], &tri));
        assert!(!point_in_polygon([0.8, 0.8], &tri));
        assert!(!point_in_polygon([0.5, 0.5], &[]));
    }

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
            review_project: String::new(),
            review_min: default_review_min(),
            review_max: default_review_max(),
            review_every_secs: default_review_every(),
        };
        assert_eq!(c.check().unwrap().0, "vision_model_missing");
        let c = VisionDetectConfig {
            model_id: "m".into(),
            review_project: "not-a-uuid".into(),
            ..c
        };
        assert_eq!(c.check().unwrap().0, "vision_review_project_invalid");
        let c = VisionDetectConfig {
            review_project: "00000000-0000-0000-0000-000000000001".into(),
            review_min: 0.7,
            ..c
        };
        assert_eq!(c.check().unwrap().0, "vision_review_band_invalid");
    }
}
