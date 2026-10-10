//! Speaker diarization helpers that need no native runtime (media-ingest.md
//! D166, lot 5): labelling words from speaker turns, linking the local
//! speakers of successive segments into stream-local labels (`S1`, `S2`…),
//! and the diarization error rate (DER) used to measure it.
//!
//! Speakers are never identified (§2 non-goals): a label is local to one
//! stream and one worker process, built from ephemeral embeddings kept in
//! memory only.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::protocol::{WireTurn, WireWord};

/// A labelled speaker turn: one item of the `speakers` field of a
/// transcript document and of the segment event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub label: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

impl Turn {
    pub fn new(label: impl Into<String>, start_ms: u32, end_ms: u32) -> Self {
        Self {
            label: label.into(),
            start_ms,
            end_ms,
        }
    }
}

fn overlap(a: (u32, u32), b: (u32, u32)) -> u32 {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

/// Labels the turns of a segment with the stream labels of their local
/// speakers; turns of a speaker without a label are dropped.
pub fn label_turns(turns: &[WireTurn], labels: &HashMap<u32, String>) -> Vec<Turn> {
    turns
        .iter()
        .filter_map(|t| labels.get(&t.spk).map(|l| Turn::new(l.clone(), t.s, t.e)))
        .collect()
}

/// Sets `spk` on each word: the label of the turn that overlaps it most
/// (a zero-length word uses its start). Words outside any turn keep none.
pub fn label_words(words: &mut [WireWord], turns: &[Turn]) {
    for w in words {
        let span = (w.s, w.e.max(w.s + 1));
        w.spk = turns
            .iter()
            .map(|t| (overlap(span, (t.start_ms, t.end_ms)), t))
            .filter(|(o, _)| *o > 0)
            .max_by_key(|(o, _)| *o)
            .map(|(_, t)| t.label.clone());
    }
}

/// Speaking time per label, ms (overlapping turns of one label counted
/// once), sorted by label.
pub fn speaking_ms(turns: &[Turn]) -> Vec<(String, u64)> {
    let mut by: HashMap<&str, Vec<(u32, u32)>> = HashMap::new();
    for t in turns {
        by.entry(&t.label).or_default().push((t.start_ms, t.end_ms));
    }
    let mut out: Vec<(String, u64)> = by
        .into_iter()
        .map(|(label, mut spans)| {
            spans.sort_unstable();
            let (mut total, mut cur) = (0u64, None::<(u32, u32)>);
            for (s, e) in spans {
                cur = match cur {
                    Some((cs, ce)) if s <= ce => Some((cs, ce.max(e))),
                    Some((cs, ce)) => {
                        total += u64::from(ce - cs);
                        Some((s, e))
                    }
                    None => Some((s, e)),
                };
            }
            if let Some((cs, ce)) = cur {
                total += u64::from(ce.saturating_sub(cs));
            }
            (label.to_string(), total)
        })
        .collect();
    out.sort();
    out
}

fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n.is_finite() && n > 0.0).then(|| v.iter().map(|x| x / n).collect())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

struct Centroid {
    label: u32,
    /// Unit vector.
    v: Vec<f32>,
    n: u32,
    seen: Instant,
}

/// Links the local speakers of successive segments of ONE stream to
/// stream-local labels by cosine similarity of their embeddings.
///
/// At most `cap` centroids, each forgotten `ttl` after it was last heard;
/// a new voice beyond the cap takes the label of the least recently heard
/// one, so labels stay within `S1..=S<cap>` (bounded metric cardinality).
/// Everything lives in memory: a restart, or another worker taking the
/// stream, starts again from `S1` (accepted, D166).
pub struct SpeakerLinker {
    cap: usize,
    ttl: Duration,
    threshold: f32,
    centroids: Vec<Centroid>,
}

/// Weight cap of a centroid's running mean: it keeps following a voice.
const CENTROID_MAX_WEIGHT: u32 = 20;

impl SpeakerLinker {
    pub fn new(cap: usize, ttl: Duration, threshold: f32) -> Self {
        Self {
            cap: cap.max(1),
            ttl,
            threshold,
            centroids: Vec::new(),
        }
    }

    /// No centroid left (all expired): the stream entry can be dropped.
    pub fn is_empty(&self) -> bool {
        self.centroids.is_empty()
    }

    pub fn expire(&mut self, now: Instant) {
        let ttl = self.ttl;
        self.centroids
            .retain(|c| now.saturating_duration_since(c.seen) <= ttl);
    }

    /// Stream labels of the local speakers `voices` (`(local index,
    /// embedding)`). Two voices of one segment never share a label.
    pub fn link(&mut self, voices: &[(u32, &[f32])], now: Instant) -> HashMap<u32, String> {
        self.expire(now);
        let voices: Vec<(u32, Vec<f32>)> = voices
            .iter()
            .filter_map(|(spk, v)| normalized(v).map(|v| (*spk, v)))
            .collect();
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (vi, (_, v)) in voices.iter().enumerate() {
            for (ci, c) in self.centroids.iter().enumerate() {
                if c.v.len() == v.len() {
                    let sim = dot(v, &c.v);
                    if sim >= self.threshold {
                        pairs.push((sim, vi, ci));
                    }
                }
            }
        }
        // shortcut: greedy best-first matching, not Hungarian; a segment
        // holds a handful of speakers.
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut voice_to: Vec<Option<usize>> = vec![None; voices.len()];
        let mut taken = vec![false; self.centroids.len()];
        for (_, vi, ci) in pairs {
            if voice_to[vi].is_none() && !taken[ci] {
                voice_to[vi] = Some(ci);
                taken[ci] = true;
            }
        }
        let mut out = HashMap::new();
        for (vi, (spk, v)) in voices.into_iter().enumerate() {
            let ci = match voice_to[vi] {
                Some(ci) => {
                    let c = &mut self.centroids[ci];
                    let w = c.n as f32;
                    let mean: Vec<f32> = c.v.iter().zip(&v).map(|(a, b)| a * w + b).collect();
                    if let Some(m) = normalized(&mean) {
                        c.v = m;
                    }
                    c.n = (c.n + 1).min(CENTROID_MAX_WEIGHT);
                    ci
                }
                None if self.centroids.len() < self.cap => {
                    let label = (1..=self.cap as u32)
                        .find(|l| self.centroids.iter().all(|c| c.label != *l))
                        .unwrap_or(1);
                    self.centroids.push(Centroid {
                        label,
                        v,
                        n: 1,
                        seen: now,
                    });
                    taken.push(true);
                    self.centroids.len() - 1
                }
                None => {
                    // Cap reached: the least recently heard voice gives
                    // its label away.
                    let Some(ci) = (0..self.centroids.len())
                        .filter(|ci| !taken[*ci])
                        .min_by_key(|ci| self.centroids[*ci].seen)
                    else {
                        continue;
                    };
                    let c = &mut self.centroids[ci];
                    c.v = v;
                    c.n = 1;
                    taken[ci] = true;
                    ci
                }
            };
            self.centroids[ci].seen = now;
            out.insert(spk, format!("S{}", self.centroids[ci].label));
        }
        out
    }
}

/// Diarization error of a hypothesis against a reference, in ms.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Der {
    /// Reference speech (overlapping speakers counted each).
    pub total_ms: u64,
    pub miss_ms: u64,
    pub false_alarm_ms: u64,
    pub confusion_ms: u64,
}

impl Der {
    /// (miss + false alarm + confusion) / reference speech.
    pub fn rate(&self) -> f64 {
        if self.total_ms == 0 {
            return 0.0;
        }
        (self.miss_ms + self.false_alarm_ms + self.confusion_ms) as f64 / self.total_ms as f64
    }
}

/// Frame of the DER grid.
const DER_FRAME_MS: u32 = 10;

fn active(turns: &[Turn], t: u32) -> Vec<&str> {
    let mut out: Vec<&str> = turns
        .iter()
        .filter(|x| x.start_ms <= t && t < x.end_ms)
        .map(|x| x.label.as_str())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Diarization error rate (NIST style, no forgiveness collar) on a 10 ms
/// grid. Hypothesis labels are mapped one-to-one onto reference labels by
/// greedy best overlap; labels are arbitrary on both sides.
pub fn der(reference: &[Turn], hypothesis: &[Turn]) -> Der {
    let end = reference
        .iter()
        .chain(hypothesis)
        .map(|t| t.end_ms)
        .max()
        .unwrap_or(0);
    let frames: Vec<(Vec<&str>, Vec<&str>)> = (0..end)
        .step_by(DER_FRAME_MS as usize)
        .map(|t| (active(reference, t), active(hypothesis, t)))
        .collect();
    let mut co: HashMap<(&str, &str), u64> = HashMap::new();
    for (r, h) in &frames {
        for a in r {
            for b in h {
                *co.entry((*a, *b)).or_default() += 1;
            }
        }
    }
    let mut pairs: Vec<((&str, &str), u64)> = co.into_iter().collect();
    // shortcut: greedy mapping (largest overlap first), not Hungarian; exact
    // for the few speakers of a test clip.
    pairs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut map: HashMap<&str, &str> = HashMap::new();
    for ((r, h), _) in pairs {
        if !map.contains_key(h) && !map.values().any(|v| *v == r) {
            map.insert(h, r);
        }
    }
    let mut d = Der::default();
    let f = u64::from(DER_FRAME_MS);
    for (r, h) in &frames {
        let (nr, nh) = (r.len() as u64, h.len() as u64);
        let correct = h
            .iter()
            .filter(|x| map.get(*x).is_some_and(|m| r.contains(m)))
            .count() as u64;
        d.total_ms += nr * f;
        d.miss_ms += nr.saturating_sub(nh) * f;
        d.false_alarm_ms += nh.saturating_sub(nr) * f;
        d.confusion_ms += (nr.min(nh) - correct) * f;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(w: &str, s: u32, e: u32) -> WireWord {
        WireWord {
            w: w.into(),
            s,
            e,
            p: None,
            spk: None,
        }
    }

    #[test]
    fn words_take_the_turn_they_overlap_most() {
        let turns = [Turn::new("S1", 0, 1000), Turn::new("S2", 900, 2000)];
        let mut words = [
            word("a", 100, 400),
            word("b", 850, 1300),
            word("c", 2500, 2600),
        ];
        label_words(&mut words, &turns);
        assert_eq!(words[0].spk.as_deref(), Some("S1"));
        assert_eq!(words[1].spk.as_deref(), Some("S2"), "400 ms vs 150 ms");
        assert_eq!(words[2].spk, None, "outside any turn");
        let json = serde_json::to_string(&words[0]).unwrap();
        assert!(json.contains("\"spk\":\"S1\""), "{json}");
        assert!(!serde_json::to_string(&words[2]).unwrap().contains("spk"));
    }

    #[test]
    fn speaking_time_merges_overlaps_per_label() {
        let turns = [
            Turn::new("S2", 0, 1000),
            Turn::new("S1", 500, 1500),
            Turn::new("S2", 800, 2000),
            Turn::new("S2", 3000, 3500),
        ];
        assert_eq!(
            speaking_ms(&turns),
            vec![("S1".to_string(), 1000), ("S2".to_string(), 2500)]
        );
    }

    #[test]
    fn linker_keeps_labels_across_segments() {
        let mut l = SpeakerLinker::new(16, Duration::from_secs(600), 0.5);
        let t0 = Instant::now();
        let a = [1.0, 0.1, 0.0];
        let b = [0.0, 1.0, 0.1];
        let first = l.link(&[(0, &a), (1, &b)], t0);
        assert_eq!(first[&0], "S1");
        assert_eq!(first[&1], "S2");
        // Next segment: the runtime's local indexes are swapped, the
        // voices are slightly different.
        let a2 = [0.9, 0.2, 0.05];
        let b2 = [0.05, 0.95, 0.0];
        let next = l.link(&[(0, &b2), (1, &a2)], t0 + Duration::from_secs(30));
        assert_eq!(next[&0], "S2");
        assert_eq!(next[&1], "S1");
        // A new voice gets a new label; two voices never share one.
        let c = [0.0, 0.0, 1.0];
        let both = l.link(&[(0, &a), (1, &a)], t0 + Duration::from_secs(40));
        assert_ne!(both[&0], both[&1]);
        let third = l.link(&[(0, &c)], t0 + Duration::from_secs(50));
        assert!(third[&0].starts_with('S'));
        // Past the TTL, everything is forgotten: labels restart.
        let late = l.link(&[(0, &b)], t0 + Duration::from_secs(3600));
        assert_eq!(late[&0], "S1");
    }

    #[test]
    fn linker_labels_are_bounded_by_the_cap() {
        let mut l = SpeakerLinker::new(2, Duration::from_secs(600), 0.9);
        let t0 = Instant::now();
        let mut seen = std::collections::HashSet::new();
        for i in 0..6u32 {
            let mut v = [0.0f32; 6];
            v[i as usize] = 1.0;
            let m = l.link(&[(0, &v)], t0 + Duration::from_secs(u64::from(i)));
            seen.insert(m[&0].clone());
        }
        assert_eq!(seen.len(), 2, "{seen:?}");
        // The least recently heard one gave its label away.
        let mut v = [0.0f32; 6];
        v[5] = 1.0;
        assert_eq!(
            l.link(&[(0, &v)], t0 + Duration::from_secs(10))[&0],
            l.link(&[(0, &v)], t0 + Duration::from_secs(11))[&0]
        );
        assert!(l.link(&[(0, &[0.0, 0.0])], t0).is_empty(), "zero vector");
    }

    #[test]
    fn der_maps_labels_and_counts_each_error_kind() {
        let reference = [Turn::new("A", 0, 1000), Turn::new("B", 1000, 2000)];
        // Same split, other names: no error.
        let same = [Turn::new("S2", 0, 1000), Turn::new("S1", 1000, 2000)];
        assert_eq!(der(&reference, &same).rate(), 0.0);
        // Everything one speaker: half the speech is confused.
        let one = [Turn::new("S1", 0, 2000)];
        let d = der(&reference, &one);
        assert_eq!((d.total_ms, d.confusion_ms), (2000, 1000));
        assert!((d.rate() - 0.5).abs() < 1e-9);
        // Missed 200 ms, 300 ms of speech invented after the end.
        let partial = [Turn::new("x", 0, 800), Turn::new("y", 1000, 2300)];
        let d = der(&reference, &partial);
        assert_eq!((d.miss_ms, d.false_alarm_ms, d.confusion_ms), (200, 300, 0));
        assert!((d.rate() - 0.25).abs() < 1e-9);
        assert_eq!(der(&[], &[]).rate(), 0.0);
    }

    #[test]
    fn turns_are_labelled_from_the_link() {
        let turns = [
            WireTurn {
                spk: 0,
                s: 0,
                e: 10,
            },
            WireTurn {
                spk: 1,
                s: 10,
                e: 20,
            },
            WireTurn {
                spk: 2,
                s: 20,
                e: 30,
            },
        ];
        let labels = HashMap::from([(0, "S3".to_string()), (1, "S1".to_string())]);
        assert_eq!(
            label_turns(&turns, &labels),
            vec![Turn::new("S3", 0, 10), Turn::new("S1", 10, 20)]
        );
    }
}
