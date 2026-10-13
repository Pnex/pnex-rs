//! Head fine-tuning of a YOLOX ONNX in pure Rust (Vision Lab, M2b): the
//! pretrained network stays frozen, only its last 1×1 convolutions (class,
//! objectness, box of each level) are learned again on the project's boxes,
//! then written back into the ONNX with the new class count.
//!
//! Pipeline: find the head in the graph → run tract up to the head
//! features (original + horizontal flip) → assign cells to boxes (SimOTA
//! lite: in box and centre radius, dynamic k from the IoU of the pretrained
//! boxes) → Adam on BCE (objectness, classes) + L1 (box) → rewrite the
//! initializers → mAP@0.5 on the validation pictures with [`Detector`].

use std::collections::HashMap;

use pnex_core::vision::ModelSpec;
use prost::Message;
use tract_onnx::pb::{self, GraphProto, ModelProto, TensorProto};
use tract_onnx::prelude::*;

use crate::{iou, letterbox_bgr, read_model, Detector, VisionError};

/// Negative cells kept per picture at most (the rest is weighted in).
const NEG_PER_IMAGE: usize = 256;
/// Floats of stored features across the dataset (≈ 256 MB): fewer
/// negatives per picture on large datasets or wide models.
const FEATURE_BUDGET: usize = 64 * 1024 * 1024;
/// Most positive cells per box (SimOTA candidate pool).
const TOP_CANDIDATES: usize = 10;
/// Centre radius of a candidate cell, in strides (YOLOX value).
const CENTER_RADIUS: f32 = 2.5;
const BATCH: usize = 256;
const LR_CLS: f32 = 1e-2;
const LR_PRETRAINED: f32 = 1e-3;

/// One picture of the dataset.
pub struct TrainImage {
    /// JPEG or PNG bytes.
    pub bytes: Vec<u8>,
    /// `(class, [x, y, w, h])`, normalized to the picture.
    pub boxes: Vec<(u32, [f32; 4])>,
    pub val: bool,
}

/// What the caller hears while training runs.
#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Features { done: usize, total: usize },
    Epoch { epoch: u32, total: u32, loss: f32 },
    Evaluating,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrainOutcome {
    pub onnx: Vec<u8>,
    /// mAP@0.5 on the validation pictures; `None` without any.
    pub map50: Option<f32>,
    /// AP@0.5 per class (`None` = no validation box of that class).
    pub per_class: Vec<Option<f32>>,
    pub positives: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum TrainError {
    #[error("not a YOLOX head this trainer understands: {0}")]
    Head(String),
    #[error("the base model must pin its input size")]
    DynamicInput,
    #[error("no annotated training picture")]
    NoData,
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Vision(#[from] VisionError),
}

/// Initializers of one head level and the tensors that feed it.
#[derive(Debug, Clone)]
struct Level {
    cls_feat: String,
    reg_feat: String,
    cls_w: String,
    cls_b: String,
    obj_w: String,
    obj_b: String,
    reg_w: String,
    reg_b: String,
    /// `[1, 5 + classes, n]` target of the level's Reshape.
    shape: String,
    channels: usize,
    stride: usize,
}

/// The three levels of the head, strides 8, 16, 32 (export order).
fn find_head(g: &GraphProto) -> Result<Vec<Level>, TrainError> {
    let err = |m: &str| TrainError::Head(m.to_string());
    let producer: HashMap<&str, &pb::NodeProto> = g
        .node
        .iter()
        .flat_map(|n| n.output.iter().map(move |o| (o.as_str(), n)))
        .collect();
    let inits: HashMap<&str, &TensorProto> =
        g.initializer.iter().map(|t| (t.name.as_str(), t)).collect();
    let op = |name: &str, kind: &str| -> Result<&pb::NodeProto, TrainError> {
        producer
            .get(name)
            .copied()
            .filter(|n| n.op_type == kind)
            .ok_or_else(|| err(&format!("expected {kind} producing {name}")))
    };
    let out = g.output.first().ok_or_else(|| err("no output"))?;
    let concat = op(&op(&out.name, "Transpose")?.input[0], "Concat")?;
    if concat.input.len() != 3 {
        return Err(err("three levels expected"));
    }
    let mut levels = Vec::new();
    for (i, r) in concat.input.iter().enumerate() {
        let reshape = op(r, "Reshape")?;
        let parts = op(&reshape.input[0], "Concat")?;
        if parts.input.len() != 3 {
            return Err(err("reg, obj, cls expected"));
        }
        let reg = op(&parts.input[0], "Conv")?;
        let obj = op(&op(&parts.input[1], "Sigmoid")?.input[0], "Conv")?;
        let cls = op(&op(&parts.input[2], "Sigmoid")?.input[0], "Conv")?;
        let weights = |conv: &pb::NodeProto,
                       rows: Option<i64>|
         -> Result<(String, String, usize), TrainError> {
            let (w, b) = (conv.input.get(1), conv.input.get(2));
            let (Some(w), Some(b)) = (w, b) else {
                return Err(err("conv without bias"));
            };
            let t = inits
                .get(w.as_str())
                .ok_or_else(|| err("conv weight not an initializer"))?;
            if t.dims.len() != 4
                || t.dims[2] != 1
                || t.dims[3] != 1
                || rows.is_some_and(|r| t.dims[0] != r)
            {
                return Err(err("1x1 head convolution expected"));
            }
            Ok((w.clone(), b.clone(), t.dims[1] as usize))
        };
        let (reg_w, reg_b, c) = weights(reg, Some(4))?;
        let (obj_w, obj_b, c2) = weights(obj, Some(1))?;
        let (cls_w, cls_b, c3) = weights(cls, None)?;
        if c != c2 || c != c3 || reg.input[0] != obj.input[0] {
            return Err(err("inconsistent head channels"));
        }
        if !inits.contains_key(reshape.input[1].as_str()) {
            return Err(err("reshape target not an initializer"));
        }
        levels.push(Level {
            cls_feat: cls.input[0].clone(),
            reg_feat: reg.input[0].clone(),
            cls_w,
            cls_b,
            obj_w,
            obj_b,
            reg_w,
            reg_b,
            shape: reshape.input[1].clone(),
            channels: c,
            stride: 8 << i,
        });
    }
    Ok(levels)
}

fn floats(t: &TensorProto) -> Vec<f32> {
    if t.raw_data.is_empty() {
        t.float_data.clone()
    } else {
        t.raw_data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect()
    }
}

fn int64s(t: &TensorProto) -> Vec<i64> {
    if t.raw_data.is_empty() {
        t.int64_data.clone()
    } else {
        t.raw_data
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| i64::from_le_bytes(*b))
            .collect()
    }
}

fn set_floats(t: &mut TensorProto, dims: Vec<i64>, data: &[f32]) {
    t.dims = dims;
    t.float_data.clear();
    t.raw_data = data.iter().flat_map(|v| v.to_le_bytes()).collect();
}

/// Learned weights of one level: rows of `channels` floats.
#[derive(Debug, Clone)]
struct Head {
    cls_w: Vec<f32>,
    cls_b: Vec<f32>,
    obj_w: Vec<f32>,
    obj_b: Vec<f32>,
    reg_w: Vec<f32>,
    reg_b: Vec<f32>,
}

/// One training cell: its features and targets.
struct Sample {
    level: usize,
    cls_feat: Vec<f32>,
    reg_feat: Vec<f32>,
    /// `Some(class, raw box target)` for a positive cell.
    target: Option<(u32, [f32; 4])>,
    /// Weight of a negative (stands for the unsampled ones).
    weight: f32,
}

fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

fn dot(w: &[f32], f: &[f32]) -> f32 {
    w.iter().zip(f).map(|(a, b)| a * b).sum()
}

/// Small deterministic generator (xorshift): reproducible sampling.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Feature maps `[C, H, W]` (flattened) of one level.
struct Maps {
    cls: Vec<f32>,
    reg: Vec<f32>,
    w: usize,
    h: usize,
}

impl Maps {
    fn at(map: &[f32], c: usize, plane: usize, cell: usize) -> Vec<f32> {
        (0..c).map(|k| map[k * plane + cell]).collect()
    }
}

/// Cells of one picture turned into samples (positives + sampled negatives).
fn assign(
    levels: &[Level],
    heads: &[Head],
    maps: &[Maps],
    boxes: &[(u32, [f32; 4])],
    rng: &mut Rng,
    neg_per_image: usize,
    out: &mut Vec<Sample>,
) -> usize {
    // (level, cell) → (box index, iou).
    let mut owner: HashMap<(usize, usize), (usize, f32)> = HashMap::new();
    for (bi, (_, b)) in boxes.iter().enumerate() {
        let (cx, cy) = (b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
        let mut cands: Vec<(f32, usize, usize, f32)> = Vec::new();
        for (li, (lv, m)) in levels.iter().zip(maps).enumerate() {
            let s = lv.stride as f32;
            let plane = m.w * m.h;
            for cell in 0..plane {
                let (gx, gy) = ((cell % m.w) as f32, (cell / m.w) as f32);
                let (px, py) = ((gx + 0.5) * s, (gy + 0.5) * s);
                let in_box = px > b[0] && px < b[0] + b[2] && py > b[1] && py < b[1] + b[3];
                let in_ctr =
                    (px - cx).abs() < CENTER_RADIUS * s && (py - cy).abs() < CENTER_RADIUS * s;
                if !(in_box && in_ctr) {
                    continue;
                }
                let f = Maps::at(&m.reg, lv.channels, plane, cell);
                let r: Vec<f32> = (0..4)
                    .map(|k| {
                        dot(&heads[li].reg_w[k * lv.channels..(k + 1) * lv.channels], &f)
                            + heads[li].reg_b[k]
                    })
                    .collect();
                let (bw, bh) = (r[2].exp() * s, r[3].exp() * s);
                let pred = [
                    (r[0] + gx) * s - bw / 2.0,
                    (r[1] + gy) * s - bh / 2.0,
                    bw,
                    bh,
                ];
                let o = iou(&pred, b);
                let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() / s;
                cands.push((o - 0.01 * dist, li, cell, o));
            }
        }
        if cands.is_empty() {
            // Box smaller than a stride-8 cell: its nearest cell.
            let m = &maps[0];
            let gx = ((cx / 8.0) as usize).min(m.w - 1);
            let gy = ((cy / 8.0) as usize).min(m.h - 1);
            cands.push((0.0, 0, gy * m.w + gx, 0.0));
        }
        cands.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top: f32 = cands.iter().take(TOP_CANDIDATES).map(|c| c.3).sum();
        let k = (top as usize).clamp(1, cands.len());
        for &(_, li, cell, o) in cands.iter().take(k) {
            let e = owner.entry((li, cell)).or_insert((bi, o));
            if o > e.1 {
                *e = (bi, o);
            }
        }
    }
    let mut negatives: Vec<(usize, usize)> = Vec::new();
    for (li, m) in maps.iter().enumerate() {
        for cell in 0..m.w * m.h {
            if !owner.contains_key(&(li, cell)) {
                negatives.push((li, cell));
            }
        }
    }
    let total_neg = negatives.len();
    let keep = neg_per_image.min(total_neg);
    for i in 0..keep {
        let j = i + rng.below(total_neg - i);
        negatives.swap(i, j);
    }
    let weight = if keep == 0 {
        0.0
    } else {
        total_neg as f32 / keep as f32
    };
    let sample = |li: usize, cell: usize, target, weight| {
        let (lv, m) = (&levels[li], &maps[li]);
        let plane = m.w * m.h;
        Sample {
            level: li,
            cls_feat: Maps::at(&m.cls, lv.channels, plane, cell),
            reg_feat: Maps::at(&m.reg, lv.channels, plane, cell),
            target,
            weight,
        }
    };
    let positives = owner.len();
    for (&(li, cell), &(bi, _)) in &owner {
        let (class, b) = boxes[bi];
        let m = &maps[li];
        let s = levels[li].stride as f32;
        let (gx, gy) = ((cell % m.w) as f32, (cell / m.w) as f32);
        let t = [
            (b[0] + b[2] / 2.0) / s - gx,
            (b[1] + b[3] / 2.0) / s - gy,
            (b[2] / s).max(1e-3).ln(),
            (b[3] / s).max(1e-3).ln(),
        ];
        out.push(sample(li, cell, Some((class, t)), 1.0));
    }
    for &(li, cell) in negatives.iter().take(keep) {
        out.push(sample(li, cell, None, weight));
    }
    positives
}

/// Adam state of one parameter vector.
struct Adam {
    m: Vec<f32>,
    v: Vec<f32>,
    lr: f32,
}

impl Adam {
    fn new(n: usize, lr: f32) -> Self {
        Self {
            m: vec![0.0; n],
            v: vec![0.0; n],
            lr,
        }
    }

    fn step(&mut self, p: &mut [f32], g: &[f32], t: i32) {
        let (b1, b2) = (0.9f32, 0.999f32);
        let (c1, c2) = (1.0 - b1.powi(t), 1.0 - b2.powi(t));
        for i in 0..p.len() {
            self.m[i] = b1 * self.m[i] + (1.0 - b1) * g[i];
            self.v[i] = b2 * self.v[i] + (1.0 - b2) * g[i] * g[i];
            p[i] -= self.lr * (self.m[i] / c1) / ((self.v[i] / c2).sqrt() + 1e-8);
        }
    }
}

/// Fine-tunes the head of `base` (a fixed-input YOLOX ONNX) on `images`
/// for `classes` classes. `progress` returns `false` to cancel.
pub fn train(
    base: &[u8],
    classes: usize,
    epochs: u32,
    images: &[TrainImage],
    progress: &mut dyn FnMut(Progress) -> bool,
) -> Result<TrainOutcome, TrainError> {
    let mut proto = ModelProto::decode(base).map_err(|e| VisionError::Load(e.to_string()))?;
    let graph = proto
        .graph
        .as_ref()
        .ok_or_else(|| TrainError::Head("no graph".into()))?;
    let levels = find_head(graph)?;
    let inits: HashMap<&str, &TensorProto> = graph
        .initializer
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();
    let pretrained: Vec<Head> = levels
        .iter()
        .map(|l| {
            let c = l.channels;
            Head {
                // New classes: zero weights, prior bias p = 0.01 (YOLOX init).
                cls_w: vec![0.0; classes * c],
                cls_b: vec![-(99.0f32).ln(); classes],
                obj_w: floats(inits[l.obj_w.as_str()]),
                obj_b: floats(inits[l.obj_b.as_str()]),
                reg_w: floats(inits[l.reg_w.as_str()]),
                reg_b: floats(inits[l.reg_b.as_str()]),
            }
        })
        .collect();

    // Frozen network up to the head features.
    let model = read_model(base)?;
    let (w, h) = crate::declared_input(&model).ok_or(TrainError::DynamicInput)?;
    let mut outlets = Vec::new();
    for l in &levels {
        for name in [&l.cls_feat, &l.reg_feat] {
            let outlet = model
                .find_outlet_label(name)
                .or_else(|| {
                    let n = graph.node.iter().find(|n| n.output.contains(name))?;
                    model
                        .node_by_name(&n.name)
                        .ok()
                        .map(|n| OutletId::new(n.id, 0))
                })
                .ok_or_else(|| TrainError::Head(format!("feature {name} not found")))?;
            outlets.push(outlet);
        }
    }
    let plan = model
        .with_output_outlets(&outlets)
        .and_then(|m| m.with_input_fact(0, f32::fact([1, 3, h as usize, w as usize]).into()))
        .and_then(|m| m.into_optimized())
        .and_then(|m| m.into_runnable())
        .map_err(|e| VisionError::Load(format!("feature extractor: {e:?}")))?;

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut samples: Vec<Sample> = Vec::new();
    let mut positives = 0usize;
    let train_imgs: Vec<&TrainImage> = images
        .iter()
        // Negative pictures (no box) teach the objectness what is not there.
        .filter(|i| !i.val)
        .collect();
    if train_imgs.is_empty() {
        return Err(TrainError::NoData);
    }
    // Two passes (flip) × two feature vectors of `channels` floats per cell.
    let channels = levels.iter().map(|l| l.channels).max().unwrap_or(1);
    let neg_per_image =
        (FEATURE_BUDGET / (train_imgs.len() * 4 * channels)).clamp(16, NEG_PER_IMAGE);
    for (done, img) in train_imgs.iter().enumerate() {
        if !progress(Progress::Features {
            done,
            total: train_imgs.len(),
        }) {
            return Err(TrainError::Cancelled);
        }
        let Ok(decoded) = image::load_from_memory(&img.bytes) else {
            continue;
        };
        let rgb = decoded.to_rgb8();
        for flip in [false, true] {
            let pic = if flip {
                image::imageops::flip_horizontal(&rgb)
            } else {
                rgb.clone()
            };
            let (sw, sh) = (pic.width() as f32, pic.height() as f32);
            let (input, ratio) = letterbox_bgr(&pic, w, h);
            let outputs = plan
                .run(tvec!(input.into()))
                .map_err(|e| VisionError::Run(e.to_string()))?;
            let mut maps = Vec::new();
            for (li, _) in levels.iter().enumerate() {
                let take = |k: usize| -> Result<(Vec<f32>, usize, usize), TrainError> {
                    let v = outputs[k]
                        .to_plain_array_view::<f32>()
                        .map_err(|e| VisionError::Run(e.to_string()))?;
                    let s = v.shape().to_vec();
                    Ok((v.iter().copied().collect(), s[3], s[2]))
                };
                let (cls, mw, mh) = take(2 * li)?;
                let (reg, _, _) = take(2 * li + 1)?;
                maps.push(Maps {
                    cls,
                    reg,
                    w: mw,
                    h: mh,
                });
            }
            let boxes: Vec<(u32, [f32; 4])> = img
                .boxes
                .iter()
                .filter(|(c, _)| (*c as usize) < classes)
                .map(|(c, b)| {
                    let x = if flip { 1.0 - b[0] - b[2] } else { b[0] };
                    (
                        *c,
                        [
                            x * sw * ratio,
                            b[1] * sh * ratio,
                            b[2] * sw * ratio,
                            b[3] * sh * ratio,
                        ],
                    )
                })
                .collect();
            positives += assign(
                &levels,
                &pretrained,
                &maps,
                &boxes,
                &mut rng,
                neg_per_image,
                &mut samples,
            );
        }
    }
    if positives == 0 {
        return Err(TrainError::NoData);
    }

    let mut heads = pretrained.clone();
    let mut opt: Vec<[Adam; 6]> = heads
        .iter()
        .map(|hd| {
            [
                Adam::new(hd.cls_w.len(), LR_CLS),
                Adam::new(hd.cls_b.len(), LR_CLS),
                Adam::new(hd.obj_w.len(), LR_PRETRAINED),
                Adam::new(hd.obj_b.len(), LR_PRETRAINED),
                Adam::new(hd.reg_w.len(), LR_PRETRAINED),
                Adam::new(hd.reg_b.len(), LR_PRETRAINED),
            ]
        })
        .collect();
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let mut t = 0i32;
    for epoch in 1..=epochs {
        for i in (1..order.len()).rev() {
            let j = rng.below(i + 1);
            order.swap(i, j);
        }
        let mut epoch_loss = 0.0f32;
        for batch in order.chunks(BATCH) {
            t += 1;
            let mut grads: Vec<Head> = heads
                .iter()
                .map(|hd| Head {
                    cls_w: vec![0.0; hd.cls_w.len()],
                    cls_b: vec![0.0; hd.cls_b.len()],
                    obj_w: vec![0.0; hd.obj_w.len()],
                    obj_b: vec![0.0; hd.obj_b.len()],
                    reg_w: vec![0.0; hd.reg_w.len()],
                    reg_b: vec![0.0; hd.reg_b.len()],
                })
                .collect();
            let fg = batch
                .iter()
                .filter(|&&i| samples[i].target.is_some())
                .count()
                .max(1) as f32;
            let mut loss = 0.0f32;
            for &i in batch {
                let s = &samples[i];
                let (hd, g, c) = (
                    &heads[s.level],
                    &mut grads[s.level],
                    levels[s.level].channels,
                );
                let y = if s.target.is_some() { 1.0 } else { 0.0 };
                let p = sigmoid(dot(&hd.obj_w, &s.reg_feat) + hd.obj_b[0]).clamp(1e-6, 1.0 - 1e-6);
                loss -= s.weight * (y * p.ln() + (1.0 - y) * (1.0 - p).ln()) / fg;
                let dz = s.weight * (p - y) / fg;
                for (gw, f) in g.obj_w.iter_mut().zip(&s.reg_feat) {
                    *gw += dz * f;
                }
                g.obj_b[0] += dz;
                let Some((class, target)) = s.target else {
                    continue;
                };
                for k in 0..classes {
                    let row = &hd.cls_w[k * c..(k + 1) * c];
                    let p = sigmoid(dot(row, &s.cls_feat) + hd.cls_b[k]).clamp(1e-6, 1.0 - 1e-6);
                    let y = if k == class as usize { 1.0 } else { 0.0 };
                    loss -= (y * p.ln() + (1.0 - y) * (1.0 - p).ln()) / fg;
                    let dz = (p - y) / fg;
                    for (gw, f) in g.cls_w[k * c..(k + 1) * c].iter_mut().zip(&s.cls_feat) {
                        *gw += dz * f;
                    }
                    g.cls_b[k] += dz;
                }
                for (k, t) in target.iter().enumerate() {
                    let o = dot(&hd.reg_w[k * c..(k + 1) * c], &s.reg_feat) + hd.reg_b[k];
                    loss += (o - t).abs() / fg;
                    let dz = (o - t).signum() / fg;
                    for (gw, f) in g.reg_w[k * c..(k + 1) * c].iter_mut().zip(&s.reg_feat) {
                        *gw += dz * f;
                    }
                    g.reg_b[k] += dz;
                }
            }
            for ((hd, g), o) in heads.iter_mut().zip(&grads).zip(&mut opt) {
                o[0].step(&mut hd.cls_w, &g.cls_w, t);
                o[1].step(&mut hd.cls_b, &g.cls_b, t);
                o[2].step(&mut hd.obj_w, &g.obj_w, t);
                o[3].step(&mut hd.obj_b, &g.obj_b, t);
                o[4].step(&mut hd.reg_w, &g.reg_w, t);
                o[5].step(&mut hd.reg_b, &g.reg_b, t);
            }
            epoch_loss += loss;
        }
        let batches = order.len().div_ceil(BATCH).max(1) as f32;
        if !progress(Progress::Epoch {
            epoch,
            total: epochs,
            loss: epoch_loss / batches,
        }) {
            return Err(TrainError::Cancelled);
        }
    }

    // Write the learned head back, with the new class count.
    let graph = proto.graph.as_mut().expect("checked above");
    let width = 5 + classes as i64;
    for (l, hd) in levels.iter().zip(&heads) {
        let c = l.channels as i64;
        for t in graph.initializer.iter_mut() {
            let n = t.name.as_str();
            if n == l.cls_w {
                set_floats(t, vec![classes as i64, c, 1, 1], &hd.cls_w);
            } else if n == l.cls_b {
                set_floats(t, vec![classes as i64], &hd.cls_b);
            } else if n == l.obj_w {
                set_floats(t, vec![1, c, 1, 1], &hd.obj_w);
            } else if n == l.obj_b {
                set_floats(t, vec![1], &hd.obj_b);
            } else if n == l.reg_w {
                set_floats(t, vec![4, c, 1, 1], &hd.reg_w);
            } else if n == l.reg_b {
                set_floats(t, vec![4], &hd.reg_b);
            } else if n == l.shape {
                let mut v = int64s(t);
                if v.len() == 3 {
                    v[1] = width;
                }
                t.int64_data.clear();
                t.raw_data = v.iter().flat_map(|x| x.to_le_bytes()).collect();
            }
        }
    }
    // Shape hints of intermediate tensors (optional in ONNX) still carry
    // the old class count: drop them. IR < 4 exports also declare the
    // initializers as inputs with their shape: keep those in step.
    graph.value_info.clear();
    let dims: HashMap<String, Vec<i64>> = graph
        .initializer
        .iter()
        .map(|t| (t.name.clone(), t.dims.clone()))
        .collect();
    for vi in graph.input.iter_mut() {
        let (Some(d), Some(pb::type_proto::Value::TensorType(tt))) = (
            dims.get(&vi.name),
            vi.r#type.as_mut().and_then(|t| t.value.as_mut()),
        ) else {
            continue;
        };
        if let Some(shape) = tt.shape.as_mut() {
            shape.dim = d
                .iter()
                .map(|v| pb::tensor_shape_proto::Dimension {
                    denotation: String::new(),
                    value: Some(pb::tensor_shape_proto::dimension::Value::DimValue(*v)),
                })
                .collect();
        }
    }
    for vi in graph.output.iter_mut() {
        if let Some(pb::type_proto::Value::TensorType(tt)) =
            vi.r#type.as_mut().and_then(|t| t.value.as_mut())
        {
            if let Some(d) = tt.shape.as_mut().and_then(|s| s.dim.get_mut(2)) {
                d.value = Some(pb::tensor_shape_proto::dimension::Value::DimValue(width));
            }
        }
    }
    let mut onnx = Vec::new();
    proto
        .encode(&mut onnx)
        .map_err(|e| VisionError::Load(e.to_string()))?;

    // Evaluation with the written model, as vision-detect will run it.
    progress(Progress::Evaluating);
    let spec = ModelSpec {
        input_width: w,
        input_height: h,
        labels: (0..classes).map(|i| format!("class_{i}")).collect(),
        score_threshold: 0.05,
        ..Default::default()
    };
    let det = Detector::load(&onnx, spec)?;
    let val: Vec<&TrainImage> = images.iter().filter(|i| i.val).collect();
    let per_class = evaluate(&det, &val, classes);
    let known: Vec<f32> = per_class.iter().flatten().copied().collect();
    let map50 = (!known.is_empty()).then(|| known.iter().sum::<f32>() / known.len() as f32);
    Ok(TrainOutcome {
        onnx,
        map50,
        per_class,
        positives,
    })
}

/// AP@0.5 per class over `val` (all-point interpolation).
fn evaluate(det: &Detector, val: &[&TrainImage], classes: usize) -> Vec<Option<f32>> {
    // Per class: (score, true positive) of every detection, and GT count.
    let mut hits: Vec<Vec<(f32, bool)>> = vec![Vec::new(); classes];
    let mut gts = vec![0usize; classes];
    for img in val {
        let Ok(res) = det.detect_with_floor(&img.bytes, 0.05) else {
            continue;
        };
        let (w, h) = (res.width as f32, res.height as f32);
        let truth: Vec<(u32, [f32; 4])> = img
            .boxes
            .iter()
            .map(|(c, b)| (*c, [b[0] * w, b[1] * h, b[2] * w, b[3] * h]))
            .collect();
        for (c, _) in &truth {
            if let Some(g) = gts.get_mut(*c as usize) {
                *g += 1;
            }
        }
        let mut used = vec![false; truth.len()];
        let mut dets = res.detections;
        dets.sort_by(|a, b| b.score.total_cmp(&a.score));
        for d in dets {
            let Some(list) = hits.get_mut(d.class_id as usize) else {
                continue;
            };
            let best = truth
                .iter()
                .enumerate()
                .filter(|(i, (c, _))| *c == d.class_id && !used[*i])
                .map(|(i, (_, b))| (i, iou(&d.bbox, b)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let tp = match best {
                Some((i, o)) if o >= 0.5 => {
                    used[i] = true;
                    true
                }
                _ => false,
            };
            list.push((d.score, tp));
        }
    }
    hits.into_iter()
        .zip(gts)
        .map(|(mut list, n)| {
            if n == 0 {
                return None;
            }
            list.sort_by(|a, b| b.0.total_cmp(&a.0));
            let (mut tp, mut fp) = (0f32, 0f32);
            let mut curve: Vec<(f32, f32)> = Vec::new();
            for (_, hit) in list {
                if hit {
                    tp += 1.0;
                } else {
                    fp += 1.0;
                }
                curve.push((tp / n as f32, tp / (tp + fp)));
            }
            // Precision envelope, then area under recall.
            for i in (0..curve.len().saturating_sub(1)).rev() {
                curve[i].1 = curve[i].1.max(curve[i + 1].1);
            }
            let mut ap = 0.0;
            let mut prev_r = 0.0;
            for (r, p) in curve {
                ap += (r - prev_r) * p;
                prev_r = r;
            }
            Some(ap)
        })
        .collect()
}

/// Head channels of a base model (compatibility check before a run).
pub fn head_classes(base: &[u8]) -> Result<usize, TrainError> {
    let proto = ModelProto::decode(base).map_err(|e| VisionError::Load(e.to_string()))?;
    let graph = proto
        .graph
        .as_ref()
        .ok_or_else(|| TrainError::Head("no graph".into()))?;
    let levels = find_head(graph)?;
    let inits: HashMap<&str, &TensorProto> = graph
        .initializer
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();
    Ok(inits[levels[0].cls_w.as_str()].dims[0] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_bounded() {
        let mut a = Rng(1);
        let mut b = Rng(1);
        for _ in 0..100 {
            let (x, y) = (a.below(7), b.below(7));
            assert_eq!(x, y);
            assert!(x < 7);
        }
    }

    /// Synthetic dataset: one orange square per picture on a noisy
    /// background, at random places and sizes. The fine-tuned head must
    /// find it on held-out pictures.
    #[test]
    #[ignore = "needs PNEX_VISION_TEST_DIR with yolox_nano.onnx (slow in debug)"]
    fn learns_a_new_class_on_synthetic_pictures() {
        let dir = std::path::PathBuf::from(std::env::var("PNEX_VISION_TEST_DIR").expect("dir"));
        let base = std::fs::read(dir.join("yolox_nano.onnx")).expect("model");
        assert_eq!(head_classes(&base).unwrap(), 80);
        let mut rng = Rng(7);
        let mut images = Vec::new();
        for i in 0..48 {
            let (w, h) = (320u32, 240u32);
            let mut img = image::RgbImage::new(w, h);
            for p in img.pixels_mut() {
                let n = (rng.below(60) + 60) as u8;
                *p = image::Rgb([n, n, n.saturating_add(20)]);
            }
            let side = 30 + rng.below(60) as u32;
            let (x0, y0) = (
                rng.below((w - side) as usize) as u32,
                rng.below((h - side) as usize) as u32,
            );
            for y in y0..y0 + side {
                for x in x0..x0 + side {
                    img.put_pixel(x, y, image::Rgb([240, 120, 20]));
                }
            }
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgb8(img)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .unwrap();
            let b = [
                x0 as f32 / w as f32,
                y0 as f32 / h as f32,
                side as f32 / w as f32,
                side as f32 / h as f32,
            ];
            images.push(TrainImage {
                bytes,
                boxes: vec![(0, b)],
                val: i % 6 == 0,
            });
        }
        let mut seen = Vec::new();
        let out = train(&base, 1, 30, &images, &mut |p| {
            if let Progress::Epoch { epoch, loss, .. } = p {
                seen.push((epoch, loss));
            }
            true
        })
        .expect("trained");
        eprintln!(
            "losses {seen:?}\nmap50 {:?} positives {}",
            out.map50, out.positives
        );
        assert!(seen.last().unwrap().1 < seen[0].1, "loss must go down");
        assert_eq!(crate::inspect(&out.onnx).unwrap().classes, Some(1));
        assert!(out.map50.unwrap() > 0.5, "{:?}", out.map50);
    }

    #[test]
    fn adam_moves_against_the_gradient() {
        let mut p = vec![1.0f32];
        let mut o = Adam::new(1, 0.1);
        for t in 1..=50 {
            let g = vec![2.0 * p[0]];
            o.step(&mut p, &g, t);
        }
        assert!(p[0].abs() < 0.5, "{}", p[0]);
    }
}
