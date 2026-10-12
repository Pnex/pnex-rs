//! Voice identification by consented enrolment (`docs/architecture/voice-id.md`,
//! cases.md K19): DTOs shared by the server and the UI, and the pure maths
//! of a voice profile — windowing, energy speech gate, centroid, calibrated
//! threshold and the verdict of a test. No native runtime: the embeddings
//! come from the `pnex-asr` process, this module only compares them.

use serde::{Deserialize, Serialize};

/// Object type a voice profile attaches to (pack `investigation`).
pub const PERSON_TYPE: &str = "person";

/// Sample rate of the decoded audio (same as the transcription chain).
pub const SAMPLE_RATE: usize = 16_000;
/// Length of one embedding window: a clip is cut into windows and each one
/// gives an embedding, so a single long clip already calibrates a profile.
pub const WINDOW_MS: u32 = 8_000;
/// Speech a window needs to be embedded (shorter = too noisy a vector).
pub const MIN_WINDOW_SPEECH_MS: u32 = 3_000;
/// Usable windows a profile needs before it can be generated (~24 s of
/// speech in total, over one or several clips).
pub const MIN_WINDOWS: usize = 3;
/// Bound of the windows used per training and per test (time and memory).
pub const MAX_WINDOWS: usize = 60;
/// Longest part of an uploaded clip that is decoded, seconds.
pub const CLIP_MAX_SECS: u32 = 180;
/// Calibrated threshold bounds: the calibration never goes below the floor
/// (a scattered enrolment would accept anyone) nor above the ceiling.
pub const THRESHOLD_FLOOR: f32 = 0.45;
pub const THRESHOLD_CEIL: f32 = 0.80;
/// A 1:N verdict needs the best profile ahead of the second by this much.
pub const MIN_MARGIN: f32 = 0.05;
/// A window farther than this from the leave-one-out centroid is an
/// outlier (another voice, music, noise) and is left out of the profile.
pub const OUTLIER_SCORE: f32 = 0.25;
/// Consent text bounds.
pub const CONSENT_SCOPE_MAX: usize = 255;
pub const CONSENT_PROOF_MAX: usize = 1000;

/// Profile lifecycle: `draft` (samples being added), `ready` (centroid and
/// threshold computed), `failed` (last generation failed, see `last_error`).
pub const STATUS_DRAFT: &str = "draft";
pub const STATUS_READY: &str = "ready";
pub const STATUS_FAILED: &str = "failed";

/// Sample state after the last generation.
pub const SAMPLE_PENDING: &str = "pending";
pub const SAMPLE_USED: &str = "used";
pub const SAMPLE_REJECTED: &str = "rejected";

/// Creation of a profile: the consent is mandatory (K19 variant A).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceProfileInput {
    pub object_id: String,
    /// Speaker embedding model of the org (task `speaker_embedding`).
    pub model_id: String,
    /// Date the person consented (`YYYY-MM-DD` or RFC 3339).
    pub consent_at: String,
    /// What the person consented to (purpose, duration).
    pub consent_scope: String,
    /// Where the proof is kept (signed form reference, recording id…).
    pub consent_proof: String,
    /// The admin confirms the consent is explicit and recorded.
    #[serde(default)]
    pub consent_confirmed: bool,
}

/// Change of embedding model: the profile goes back to `draft`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceProfilePatch {
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceSampleView {
    pub id: String,
    pub media_asset_id: String,
    pub name: String,
    #[serde(default)]
    pub duration_ms: Option<u32>,
    #[serde(default)]
    pub speech_ms: Option<u32>,
    /// Windows of this clip used by the last generation.
    #[serde(default)]
    pub windows: Option<u32>,
    /// Mean similarity of its windows to the profile (last generation).
    #[serde(default)]
    pub score: Option<f32>,
    pub status: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub created_at: String,
}

/// A profile as the UI reads it. Never the vector (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceProfileView {
    pub id: String,
    pub object_id: String,
    pub person: String,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    pub status: String,
    #[serde(default)]
    pub threshold: Option<f32>,
    /// Windows the centroid is built from.
    pub windows_used: u32,
    /// Spread of the enrolment: mean and standard deviation of the
    /// leave-one-out scores (quality indicator).
    #[serde(default)]
    pub intra_mean: Option<f32>,
    #[serde(default)]
    pub intra_std: Option<f32>,
    pub consent_at: String,
    pub consent_scope: String,
    pub consent_proof: String,
    pub consent_recorded_by: String,
    #[serde(default)]
    pub trained_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub samples: Vec<VoiceSampleView>,
    pub created_at: String,
    pub updated_at: String,
}

/// One profile compared with the tested audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCandidate {
    pub profile_id: String,
    pub object_id: String,
    pub person: String,
    pub score: f32,
    pub threshold: f32,
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceVerdict {
    /// One profile accepted, clearly ahead of the others.
    Match,
    /// Several profiles accepted too close to tell apart.
    Ambiguous,
    NoMatch,
}

/// Result of a test (verification 1:1 or identification 1:N). A match is
/// a proposal for a human, never a fact written anywhere (K19).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceTestResult {
    pub model_id: String,
    pub audio_ms: u32,
    pub speech_ms: u32,
    pub windows: u32,
    pub verdict: VoiceVerdict,
    #[serde(default)]
    pub best: Option<VoiceCandidate>,
    /// Best first.
    pub candidates: Vec<VoiceCandidate>,
    pub truncated: bool,
}

/// One window of a clip, in samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: usize,
    pub end: usize,
    pub speech_ms: u32,
}

/// Frame of the energy gate.
const FRAME: usize = SAMPLE_RATE / 50;
/// A frame is speech when its RMS is above this fraction of the clip's
/// reference level and above an absolute floor (digital silence).
const SPEECH_REL: f32 = 0.1;
/// Reference level = this percentile of frame RMS, not the loudest frame:
/// one burst (applause, jingle, plosive) would otherwise gate out the speech.
const REFERENCE_PERCENTILE: f32 = 0.95;
const SPEECH_ABS: f32 = 0.005;

fn rms(frame: &[f32]) -> f32 {
    (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt()
}

/// Speech per window of [`WINDOW_MS`] by an energy gate (20 ms frames).
/// shortcut: energy gate, not a VAD model; noisy recordings overestimate
/// speech — use the org's VAD model if enrolments in noise fail.
pub fn windows(samples: &[f32]) -> Vec<Window> {
    let mut levels: Vec<f32> = samples.chunks(FRAME).map(rms).collect();
    levels.sort_by(f32::total_cmp);
    let reference = levels
        .get(
            ((levels.len() as f32 * REFERENCE_PERCENTILE) as usize)
                .min(levels.len().saturating_sub(1)),
        )
        .copied()
        .unwrap_or(0.0);
    let gate = (reference * SPEECH_REL).max(SPEECH_ABS);
    let len = WINDOW_MS as usize * SAMPLE_RATE / 1000;
    let mut out = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let end = (start + len).min(samples.len());
        let speech_frames = samples[start..end]
            .chunks(FRAME)
            .filter(|f| f.len() == FRAME && rms(f) >= gate)
            .count();
        out.push(Window {
            start,
            end,
            speech_ms: (speech_frames * 20) as u32,
        });
        start = end;
    }
    out
}

/// Windows with enough speech to be embedded.
pub fn usable(ws: &[Window]) -> Vec<Window> {
    ws.iter()
        .copied()
        .filter(|w| w.speech_ms >= MIN_WINDOW_SPEECH_MS)
        .collect()
}

pub fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n.is_finite() && n > 0.0).then(|| v.iter().map(|x| x / n).collect())
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    match (normalized(a), normalized(b)) {
        (Some(a), Some(b)) if a.len() == b.len() => a.iter().zip(&b).map(|(x, y)| x * y).sum(),
        _ => 0.0,
    }
}

/// Normalized mean of unit vectors (`None` when empty or degenerate).
pub fn centroid(vs: &[Vec<f32>]) -> Option<Vec<f32>> {
    let units: Vec<Vec<f32>> = vs.iter().filter_map(|v| normalized(v)).collect();
    let dim = units.first()?.len();
    if units.iter().any(|u| u.len() != dim) {
        return None;
    }
    let mut sum = vec![0.0f32; dim];
    for u in &units {
        for (s, x) in sum.iter_mut().zip(u) {
            *s += x;
        }
    }
    normalized(&sum)
}

/// A generated profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Training {
    pub centroid: Vec<f32>,
    pub threshold: f32,
    /// Leave-one-out score of each input window (outliers included).
    pub scores: Vec<f32>,
    /// Input windows kept in the centroid.
    pub used: Vec<bool>,
    pub intra_mean: f32,
    pub intra_std: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrainError {
    /// Fewer than [`MIN_WINDOWS`] usable windows (after outliers).
    NotEnough { usable: usize },
    /// Vectors of different dimensions or all zero.
    Degenerate,
}

fn mean_std(xs: &[f32]) -> (f32, f32) {
    let n = xs.len().max(1) as f32;
    let mean = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Leave-one-out score of each window: its similarity to the centroid of
/// the others, so a window never vouches for itself.
fn loo_scores(vs: &[Vec<f32>]) -> Result<Vec<f32>, TrainError> {
    (0..vs.len())
        .map(|i| {
            let others: Vec<Vec<f32>> = vs
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, v)| v.clone())
                .collect();
            centroid(&others)
                .map(|c| cosine(&vs[i], &c))
                .ok_or(TrainError::Degenerate)
        })
        .collect()
}

/// Builds a profile from window embeddings: outliers (another voice,
/// music) are dropped, the centroid is the normalized mean of the rest and
/// the threshold is calibrated on the enrolment spread (mean − 2σ of the
/// leave-one-out scores), bounded by [`THRESHOLD_FLOOR`]/[`THRESHOLD_CEIL`].
pub fn train(vs: &[Vec<f32>]) -> Result<Training, TrainError> {
    if vs.len() < MIN_WINDOWS {
        return Err(TrainError::NotEnough { usable: vs.len() });
    }
    let scores = loo_scores(vs)?;
    let used: Vec<bool> = scores.iter().map(|s| *s >= OUTLIER_SCORE).collect();
    let kept: Vec<Vec<f32>> = vs
        .iter()
        .zip(&used)
        .filter(|(_, u)| **u)
        .map(|(v, _)| v.clone())
        .collect();
    if kept.len() < MIN_WINDOWS {
        return Err(TrainError::NotEnough { usable: kept.len() });
    }
    let centroid = centroid(&kept).ok_or(TrainError::Degenerate)?;
    let kept_scores = loo_scores(&kept)?;
    let (intra_mean, intra_std) = mean_std(&kept_scores);
    let threshold = (intra_mean - 2.0 * intra_std).clamp(THRESHOLD_FLOOR, THRESHOLD_CEIL);
    Ok(Training {
        centroid,
        threshold,
        scores,
        used,
        intra_mean,
        intra_std,
    })
}

/// Verdict over candidates sorted best first: a match needs the best one
/// accepted and, when a second one is accepted too, a margin of
/// [`MIN_MARGIN`] between them.
pub fn verdict(candidates: &[VoiceCandidate]) -> VoiceVerdict {
    match candidates {
        [best, ..] if !best.accepted => VoiceVerdict::NoMatch,
        [best, second, ..] if second.accepted && best.score - second.score < MIN_MARGIN => {
            VoiceVerdict::Ambiguous
        }
        [_, ..] => VoiceVerdict::Match,
        [] => VoiceVerdict::NoMatch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| amp * (i as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn windows_measure_speech_by_energy() {
        let mut s = tone(8.0, 0.5);
        s.extend(vec![0.0; 8 * SAMPLE_RATE]);
        s.extend(tone(2.0, 0.5));
        let ws = windows(&s);
        assert_eq!(ws.len(), 3);
        assert!(ws[0].speech_ms >= 7_900, "{:?}", ws[0]);
        assert_eq!(ws[1].speech_ms, 0);
        assert!(ws[2].speech_ms <= 2_000);
        assert_eq!(usable(&ws), vec![ws[0]]);
        assert!(windows(&[]).is_empty());
    }

    #[test]
    fn a_loud_burst_does_not_gate_out_speech() {
        // Moderate speech with a short burst 15x louder (applause, jingle).
        let mut s = tone(1.0, 1.0);
        s.extend(tone(23.0, 0.066));
        let ws = windows(&s);
        assert_eq!(usable(&ws).len(), 3, "{ws:?}");
    }

    fn jitter(base: &[f32], k: f32) -> Vec<f32> {
        base.iter()
            .enumerate()
            .map(|(i, x)| x + k * ((i as f32 + k * 10.0).sin()))
            .collect()
    }

    #[test]
    fn training_drops_outliers_and_calibrates() {
        let a = [1.0, 0.2, 0.0, 0.1];
        let mut vs: Vec<Vec<f32>> = (1..=5).map(|k| jitter(&a, 0.05 * k as f32)).collect();
        // Another voice slipped into the enrolment.
        vs.push(vec![0.0, 0.0, 1.0, 0.0]);
        let t = train(&vs).unwrap();
        assert_eq!(t.used, [true, true, true, true, true, false]);
        assert!(cosine(&t.centroid, &a) > 0.95);
        assert!((THRESHOLD_FLOOR..=THRESHOLD_CEIL).contains(&t.threshold));
        assert!(t.intra_mean > 0.9, "{}", t.intra_mean);
        // The voice is accepted, the intruder is not.
        assert!(cosine(&jitter(&a, 0.07), &t.centroid) >= t.threshold);
        assert!(cosine(&[0.0, 0.0, 1.0, 0.0], &t.centroid) < t.threshold);
    }

    #[test]
    fn training_needs_enough_windows() {
        let a = vec![1.0, 0.0];
        assert_eq!(
            train(&[a.clone(), a.clone()]),
            Err(TrainError::NotEnough { usable: 2 })
        );
        // Three windows, one of them another voice: two left, not enough.
        let b = vec![0.0, 1.0];
        assert_eq!(
            train(&[a.clone(), b.clone(), b]),
            Err(TrainError::NotEnough { usable: 2 })
        );
        assert_eq!(
            train(&[vec![0.0, 0.0], vec![0.0, 0.0], vec![0.0, 0.0]]),
            Err(TrainError::Degenerate)
        );
    }

    fn cand(score: f32, accepted: bool) -> VoiceCandidate {
        VoiceCandidate {
            profile_id: String::new(),
            object_id: String::new(),
            person: String::new(),
            score,
            threshold: 0.5,
            accepted,
        }
    }

    #[test]
    fn verdict_needs_a_clear_winner() {
        assert_eq!(verdict(&[]), VoiceVerdict::NoMatch);
        assert_eq!(verdict(&[cand(0.4, false)]), VoiceVerdict::NoMatch);
        assert_eq!(verdict(&[cand(0.7, true)]), VoiceVerdict::Match);
        assert_eq!(
            verdict(&[cand(0.7, true), cand(0.68, true)]),
            VoiceVerdict::Ambiguous
        );
        assert_eq!(
            verdict(&[cand(0.7, true), cand(0.6, true)]),
            VoiceVerdict::Match
        );
        assert_eq!(
            verdict(&[cand(0.7, true), cand(0.69, false)]),
            VoiceVerdict::Match
        );
    }

    #[test]
    fn cosine_handles_bad_vectors() {
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 0.0]), 0.0);
        assert!((cosine(&[2.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }
}
