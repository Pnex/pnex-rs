//! Vision Lab contract (media-vision-studio.md §4.6, D198–D200): projects
//! of object classes, images of the media library annotated with boxes,
//! frozen dataset versions and their COCO export. Pure, wasm-safe.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Classes of one project.
pub const LAB_MAX_CLASSES: usize = 50;
/// Boxes of one image.
pub const LAB_MAX_BOXES: usize = 200;
pub const LAB_NAME_MAX: usize = 120;
pub const LAB_CLASS_MAX: usize = 64;
/// Indicative floor of annotated images per class before training (§4.6.2).
pub const LAB_MIN_PER_CLASS: usize = 20;
/// Indicative floor of validation images.
pub const LAB_MIN_VAL_IMAGES: usize = 5;
/// One source in [`LAB_VAL_EVERY`] goes to validation (≈ 20 %).
pub const LAB_VAL_EVERY: u64 = 5;

/// A project (`GET /api/v1/lab/projects`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabProject {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Class names; a box refers to its class by index.
    pub classes: Vec<String>,
    #[serde(default)]
    pub image_count: u64,
    #[serde(default)]
    pub annotated_count: u64,
    #[serde(default)]
    pub version_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Create / update body of a project.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LabProjectInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub classes: Vec<String>,
}

impl LabProjectInput {
    /// Field-level validation (machine tokens).
    pub fn check(&self) -> Result<(), Vec<(&'static str, String)>> {
        let mut errs = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            errs.push(("name", crate::err_codes::FIELD_REQUIRED.to_string()));
        } else if name.chars().count() > LAB_NAME_MAX {
            errs.push(("name", format!("max_length:{LAB_NAME_MAX}")));
        }
        if self.description.chars().count() > 2000 {
            errs.push(("description", "max_length:2000".to_string()));
        }
        let mut seen = std::collections::BTreeSet::new();
        let classes_ok = self.classes.len() <= LAB_MAX_CLASSES
            && self.classes.iter().all(|c| {
                let t = c.trim();
                !t.is_empty() && t.chars().count() <= LAB_CLASS_MAX && seen.insert(t)
            });
        if !classes_ok {
            errs.push(("classes", format!("range:0..{LAB_MAX_CLASSES}")));
        }
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// One box: class index + `[x, y, w, h]` normalized to the image (0–1,
/// top-left origin), so it does not depend on a display size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LabBox {
    pub class: u32,
    pub bbox: [f32; 4],
}

/// Train / validation assignment of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabSplit {
    /// Decided by the source of the image ([`auto_split`]).
    #[default]
    Auto,
    Train,
    Val,
}

/// An image of a project (`GET /api/v1/lab/projects/{id}/images`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabImage {
    pub id: String,
    /// Media asset (D21) holding the picture.
    pub asset_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Import batch or video the image comes from: the split unit.
    pub source: String,
    #[serde(default)]
    pub split: LabSplit,
    #[serde(default)]
    pub boxes: Vec<LabBox>,
    /// Checked "no object here" (negative example).
    #[serde(default)]
    pub negative: bool,
    /// Optimistic concurrency counter of the annotations.
    pub version: i32,
    pub updated_at: String,
}

impl LabImage {
    pub fn annotated(&self) -> bool {
        self.negative || !self.boxes.is_empty()
    }

    /// Effective split: the forced one, else by source.
    pub fn effective_split(&self) -> LabSplit {
        match self.split {
            LabSplit::Auto => auto_split(&self.source),
            other => other,
        }
    }
}

/// `PUT /api/v1/lab/images/{id}/annotations`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabAnnotationsInput {
    pub boxes: Vec<LabBox>,
    #[serde(default)]
    pub negative: bool,
    #[serde(default)]
    pub split: LabSplit,
    /// Version the editor read (409 when it changed meanwhile).
    pub expected_version: i32,
}

impl LabAnnotationsInput {
    /// Machine token of the first problem, `None` when valid.
    pub fn check(&self, class_count: usize) -> Option<&'static str> {
        if self.boxes.len() > LAB_MAX_BOXES {
            return Some("lab-boxes-too-many");
        }
        if self.negative && !self.boxes.is_empty() {
            return Some("lab-negative-with-boxes");
        }
        let ok = self.boxes.iter().all(|b| {
            let [x, y, w, h] = b.bbox;
            (b.class as usize) < class_count
                && b.bbox.iter().all(|v| v.is_finite())
                && x >= 0.0
                && y >= 0.0
                && w > 0.0
                && h > 0.0
                && x + w <= 1.0 + 1e-4
                && y + h <= 1.0 + 1e-4
        });
        if ok {
            None
        } else {
            Some("lab-box-invalid")
        }
    }
}

/// `POST /api/v1/lab/images/{id}/suggest` — pre-annotation with a model
/// of the registry (§4.6.1 step 4): proposals only, nothing is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabSuggestRequest {
    /// `ml_models.id` (detection model of the org).
    pub model_id: String,
    /// Overrides the model threshold when > 0.
    #[serde(default)]
    pub min_score: f32,
}

/// One proposed box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabSuggestion {
    /// Label of the model.
    pub label: String,
    pub score: f32,
    /// Project class whose name matches the label, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<u32>,
    /// Normalized `[x, y, w, h]`.
    pub bbox: [f32; 4],
}

/// Most proposals returned for one image.
pub const LAB_MAX_SUGGESTIONS: usize = 100;

/// `Red Valve`, `red-valve` and `red_valve` name the same class.
fn class_key(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect()
}

/// Project class matching a model label.
pub fn class_for_label(classes: &[String], label: &str) -> Option<u32> {
    let key = class_key(label);
    classes
        .iter()
        .position(|c| class_key(c) == key)
        .map(|i| i as u32)
}

/// `POST /api/v1/lab/projects/{id}/import-recording` — frames of a camera
/// recording (D79) added as pictures, one import = one source (D202).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabRecordingImport {
    /// `device_registries.id` of the camera.
    pub device: i64,
    /// RFC 3339 bounds of the extract.
    pub from: String,
    pub to: String,
    /// One frame every N seconds.
    pub every_secs: f64,
}

/// Longest extract of one import, s.
pub const LAB_RECORDING_MAX_SECS: i64 = 3600;
/// Most frames of one import.
pub const LAB_RECORDING_MAX_FRAMES: usize = 200;

impl LabRecordingImport {
    /// Field token of the first problem, given the parsed span in ms.
    pub fn check(&self, span_ms: i64) -> Option<(&'static str, String)> {
        if span_ms <= 0 || span_ms > LAB_RECORDING_MAX_SECS * 1000 {
            return Some(("to", format!("range:1..{LAB_RECORDING_MAX_SECS}")));
        }
        if !self.every_secs.is_finite() || !(0.5..=600.0).contains(&self.every_secs) {
            return Some(("every_secs", "range:0.5..600".into()));
        }
        let frames = (span_ms as f64 / (self.every_secs * 1000.0)).ceil() as usize;
        if frames > LAB_RECORDING_MAX_FRAMES {
            return Some((
                "every_secs",
                format!("max_frames:{LAB_RECORDING_MAX_FRAMES}"),
            ));
        }
        None
    }
}

/// Frames to extract: for each instant `from + k·every` inside a segment,
/// the nearest frame of that segment. Segments are `(start_ms, end_ms,
/// frame count)`, frames evenly spread over the segment. Returns
/// `(segment index, frame index, instant)`; instants with no recording are
/// skipped, a frame is never picked twice.
pub fn pick_frames(
    segments: &[(i64, i64, usize)],
    from_ms: i64,
    to_ms: i64,
    every_ms: i64,
) -> Vec<(usize, usize, i64)> {
    let mut out: Vec<(usize, usize, i64)> = Vec::new();
    if every_ms <= 0 {
        return out;
    }
    let mut t = from_ms;
    while t < to_ms {
        let hit = segments
            .iter()
            .enumerate()
            .find(|(_, (s, e, n))| *n > 0 && t >= *s && t < *e);
        if let Some((si, (s, e, n))) = hit {
            let span = (e - s).max(1) as f64;
            let fi = (((t - s) as f64 / span) * *n as f64).floor() as usize;
            let fi = fi.min(n - 1);
            if !out.iter().any(|(a, b, _)| *a == si && *b == fi) {
                out.push((si, fi, t));
            }
        }
        t += every_ms;
    }
    out
}

/// FNV-1a 64: a stable hash (the std one may change between releases).
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Split by **source**, never by frame (§4.6.2): every image of a source
/// lands on the same side, so near-identical frames never leak from train
/// into validation.
pub fn auto_split(source: &str) -> LabSplit {
    if fnv1a(source).is_multiple_of(LAB_VAL_EVERY) {
        LabSplit::Val
    } else {
        LabSplit::Train
    }
}

/// Counts of a dataset and the warnings shown before a training (§4.6.2,
/// indicative floors: never blocking).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LabStats {
    pub images: usize,
    pub annotated: usize,
    pub negatives: usize,
    pub train: usize,
    pub val: usize,
    /// Boxes per class, in class order.
    pub boxes_per_class: Vec<usize>,
    /// Annotated images showing each class, in class order.
    pub images_per_class: Vec<usize>,
    /// Machine codes (`lab-warn-…`) with their class when relevant.
    pub warnings: Vec<LabWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabWarning {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

pub fn stats(classes: &[String], images: &[LabImage]) -> LabStats {
    let mut s = LabStats {
        images: images.len(),
        boxes_per_class: vec![0; classes.len()],
        images_per_class: vec![0; classes.len()],
        ..Default::default()
    };
    for img in images.iter().filter(|i| i.annotated()) {
        s.annotated += 1;
        s.negatives += usize::from(img.negative);
        match img.effective_split() {
            LabSplit::Val => s.val += 1,
            _ => s.train += 1,
        }
        let mut seen = std::collections::BTreeSet::new();
        for b in &img.boxes {
            if let Some(n) = s.boxes_per_class.get_mut(b.class as usize) {
                *n += 1;
            }
            seen.insert(b.class as usize);
        }
        for c in seen {
            if let Some(n) = s.images_per_class.get_mut(c) {
                *n += 1;
            }
        }
    }
    let warn = |code: &str, class: Option<&String>| LabWarning {
        code: code.to_string(),
        class: class.cloned(),
    };
    if classes.is_empty() {
        s.warnings.push(warn("lab-warn-no-class", None));
    }
    for (i, c) in classes.iter().enumerate() {
        if s.images_per_class[i] < LAB_MIN_PER_CLASS {
            s.warnings.push(warn("lab-warn-class-few", Some(c)));
        }
    }
    if s.val < LAB_MIN_VAL_IMAGES {
        s.warnings.push(warn("lab-warn-val-few", None));
    }
    if s.annotated > 0 && s.negatives == 0 {
        s.warnings.push(warn("lab-warn-no-negative", None));
    }
    s
}

/// One image of a frozen dataset version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotImage {
    pub image_id: String,
    pub asset_id: String,
    /// Media version frozen at snapshot time (D21).
    pub asset_version: i64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub split: LabSplit,
    pub boxes: Vec<LabBox>,
}

/// Frozen dataset (`lab_dataset_versions.snapshot`): what a training run or
/// an export reads, never the live annotations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetSnapshot {
    pub classes: Vec<String>,
    pub images: Vec<SnapshotImage>,
}

/// A dataset version (`GET /api/v1/lab/projects/{id}/versions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabDatasetVersion {
    pub id: String,
    pub version: i32,
    #[serde(default)]
    pub note: String,
    pub stats: LabStats,
    pub created_at: String,
    #[serde(default)]
    pub created_by: String,
}

/// COCO detection annotations of one split (`None` = every image), boxes
/// in pixels, categories 1-based in class order; `file_name` is
/// `images/<image_id>.<ext>` as written in the export archive.
pub fn coco(
    snapshot: &DatasetSnapshot,
    split: Option<LabSplit>,
    ext_of: &dyn Fn(&SnapshotImage) -> String,
) -> serde_json::Value {
    let mut images = Vec::new();
    let mut annotations = Vec::new();
    let mut ann_id = 0u64;
    for (i, img) in snapshot
        .images
        .iter()
        .filter(|img| split.is_none_or(|s| img.split == s))
        .enumerate()
    {
        let image_id = i as u64 + 1;
        images.push(serde_json::json!({
            "id": image_id,
            "file_name": format!("images/{}.{}", img.image_id, ext_of(img)),
            "width": img.width,
            "height": img.height,
        }));
        let (w, h) = (img.width as f32, img.height as f32);
        for b in &img.boxes {
            ann_id += 1;
            let px = [b.bbox[0] * w, b.bbox[1] * h, b.bbox[2] * w, b.bbox[3] * h];
            let px = px.map(|v| (v * 100.0).round() / 100.0);
            annotations.push(serde_json::json!({
                "id": ann_id,
                "image_id": image_id,
                "category_id": b.class + 1,
                "bbox": px,
                "area": ((px[2] * px[3]) * 100.0).round() / 100.0,
                "iscrowd": 0,
            }));
        }
    }
    let categories: Vec<serde_json::Value> = snapshot
        .classes
        .iter()
        .enumerate()
        .map(|(i, c)| serde_json::json!({ "id": i + 1, "name": c, "supercategory": "object" }))
        .collect();
    serde_json::json!({
        "info": { "description": "PneX Vision Lab export", "version": "1.0" },
        "images": images,
        "annotations": annotations,
        "categories": categories,
    })
}

/// Counts per split of a snapshot (shown before an export).
pub fn snapshot_split_counts(snapshot: &DatasetSnapshot) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for img in &snapshot.images {
        let k = match img.split {
            LabSplit::Val => "val",
            _ => "train",
        };
        *m.entry(k).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(source: &str, boxes: Vec<LabBox>, negative: bool) -> LabImage {
        LabImage {
            id: format!("i-{source}"),
            asset_id: "a".into(),
            name: "n".into(),
            width: 200,
            height: 100,
            source: source.into(),
            split: LabSplit::Auto,
            boxes,
            negative,
            version: 1,
            updated_at: String::new(),
        }
    }

    #[test]
    fn project_check() {
        let mut p = LabProjectInput {
            name: " ".into(),
            ..Default::default()
        };
        assert!(p.check().unwrap_err().iter().any(|(f, _)| *f == "name"));
        p.name = "valves".into();
        p.classes = vec!["red_valve".into(), "red_valve".into()];
        assert!(p.check().unwrap_err().iter().any(|(f, _)| *f == "classes"));
        p.classes = vec!["red_valve".into(), "helmet".into()];
        assert!(p.check().is_ok());
    }

    #[test]
    fn annotations_check() {
        let b = |class, bbox| LabBox { class, bbox };
        let ok = LabAnnotationsInput {
            boxes: vec![b(1, [0.1, 0.1, 0.5, 0.5])],
            negative: false,
            split: LabSplit::Auto,
            expected_version: 1,
        };
        assert_eq!(ok.check(2), None);
        assert_eq!(ok.check(1), Some("lab-box-invalid"), "unknown class");
        let out = LabAnnotationsInput {
            boxes: vec![b(0, [0.8, 0.1, 0.5, 0.5])],
            ..ok.clone()
        };
        assert_eq!(out.check(2), Some("lab-box-invalid"));
        let neg = LabAnnotationsInput {
            negative: true,
            ..ok.clone()
        };
        assert_eq!(neg.check(2), Some("lab-negative-with-boxes"));
    }

    #[test]
    fn recording_frames() {
        // Two 10 s segments of 10 frames, a gap between them.
        let segs = [(0, 10_000, 10), (20_000, 30_000, 10)];
        let picks = pick_frames(&segs, 0, 30_000, 5_000);
        assert_eq!(
            picks,
            vec![(0, 0, 0), (0, 5, 5_000), (1, 0, 20_000), (1, 5, 25_000)]
        );
        // Faster than the frames: no duplicate.
        assert_eq!(pick_frames(&[(0, 1_000, 1)], 0, 1_000, 200).len(), 1);
        let imp = LabRecordingImport {
            device: 1,
            from: String::new(),
            to: String::new(),
            every_secs: 1.0,
        };
        assert!(imp.check(60_000).is_none());
        assert_eq!(imp.check(0).unwrap().0, "to");
        assert_eq!(imp.check(3_600_000).unwrap().1, "max_frames:200");
    }

    #[test]
    fn labels_map_to_classes() {
        let classes = vec!["Red valve".to_string(), "person".to_string()];
        assert_eq!(class_for_label(&classes, "red_valve"), Some(0));
        assert_eq!(class_for_label(&classes, "Person"), Some(1));
        assert_eq!(class_for_label(&classes, "dog"), None);
    }

    #[test]
    fn split_follows_the_source() {
        let a = auto_split("video-42");
        assert_eq!(a, auto_split("video-42"), "stable");
        let val = (0..1000)
            .filter(|i| auto_split(&format!("import-{i}")) == LabSplit::Val)
            .count();
        assert!((150..250).contains(&val), "≈ 20 % in validation: {val}");
    }

    #[test]
    fn stats_and_warnings() {
        let classes = vec!["valve".to_string(), "helmet".to_string()];
        let b = |class| LabBox {
            class,
            bbox: [0.1, 0.1, 0.2, 0.2],
        };
        let imgs = vec![
            image("s1", vec![b(0), b(0)], false),
            image("s2", vec![], true),
            image("s3", vec![], false),
        ];
        let s = stats(&classes, &imgs);
        assert_eq!((s.images, s.annotated, s.negatives), (3, 2, 1));
        assert_eq!(s.boxes_per_class, vec![2, 0]);
        assert_eq!(s.images_per_class, vec![1, 0]);
        assert!(s
            .warnings
            .iter()
            .any(|w| w.code == "lab-warn-class-few" && w.class.as_deref() == Some("helmet")));
        assert!(!s.warnings.iter().any(|w| w.code == "lab-warn-no-negative"));
    }

    #[test]
    fn coco_in_pixels() {
        let snap = DatasetSnapshot {
            classes: vec!["valve".into()],
            images: vec![SnapshotImage {
                image_id: "x1".into(),
                asset_id: "a".into(),
                asset_version: 3,
                name: "n".into(),
                width: 200,
                height: 100,
                split: LabSplit::Train,
                boxes: vec![LabBox {
                    class: 0,
                    bbox: [0.1, 0.2, 0.5, 0.5],
                }],
            }],
        };
        let c = coco(&snap, None, &|_| "jpg".into());
        assert_eq!(c["images"][0]["file_name"], "images/x1.jpg");
        assert_eq!(
            c["annotations"][0]["bbox"],
            serde_json::json!([20.0, 20.0, 100.0, 50.0])
        );
        assert_eq!(c["annotations"][0]["category_id"], 1);
        assert_eq!(c["categories"][0]["name"], "valve");
        assert_eq!(
            coco(&snap, Some(LabSplit::Val), &|_| "jpg".into())["images"],
            serde_json::json!([])
        );
    }
}
