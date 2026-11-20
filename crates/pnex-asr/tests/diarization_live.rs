//! Live measures of lot 5 (media.md §3.7), ignored by default: they
//! need the sherpa runtime and models cached in `~/.cache/pnex-asr-test`
//! (`PNEX_ASR_TEST_DIR`):
//! - `models/silero_vad.onnx` (MIT);
//! - `models/sherpa-onnx-pyannote-segmentation-3-0/` (MIT);
//! - `models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx`
//!   (3D-Speaker, Apache-2.0; another one through `PNEX_DIAR_EMB`);
//! - `data/fleurs/` (FLEURS fr test split, CC BY 4.0), `data/radio/`.
//!
//! ```text
//! cargo test -p pnex-asr --release --features sherpa --test diarization_live -- --ignored --nocapture
//! ```
#![cfg(any(feature = "sherpa", feature = "sherpa-shared"))]

use std::path::PathBuf;
use std::time::Instant;

use pnex_asr::diarize::{der, Der, SpeakerLinker, Turn};
use pnex_asr::sherpa::{SherpaDiarizer, SherpaOptions, SherpaVad};
use pnex_asr::{read_wav, Diarizer, VoiceDetector, SAMPLE_RATE};

fn root() -> PathBuf {
    std::env::var("PNEX_ASR_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/pnex-asr-test")
        })
}

fn vad() -> SherpaVad {
    SherpaVad::load(
        &root().join("models/silero_vad.onnx"),
        &SherpaOptions::default(),
    )
    .expect("silero vad")
}

#[test]
#[ignore]
fn vad_rtf_and_silence() {
    let mut v = vad();
    assert_eq!(v.speech_ms(&vec![0.0; 30 * 16_000]).unwrap(), 0, "zeros");
    let radio = read_wav(&root().join("data/radio/franceinter-10min.wav")).unwrap();
    let mut speech = 0u64;
    let started = Instant::now();
    for chunk in radio.chunks(30 * SAMPLE_RATE as usize) {
        speech += u64::from(v.speech_ms(chunk).unwrap());
    }
    let infer = started.elapsed().as_secs_f64();
    let audio = radio.len() as f64 / f64::from(SAMPLE_RATE);
    println!(
        "VAD: {audio:.0} s of radio in 30 s segments, speech {:.0} s, RTF {:.4}",
        speech as f64 / 1000.0,
        infer / audio
    );
    assert!(speech > 0);
}

/// A clip without its leading / trailing quiet.
fn trim(s: &[f32]) -> &[f32] {
    let v = voiced(s);
    match (v.first(), v.last()) {
        (Some(a), Some(b)) => &s[a.0..b.1],
        _ => s,
    }
}

fn rms(c: &[f32]) -> f32 {
    (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()
}

/// Voiced spans of a single-speaker clip (20 ms frames louder than 1/10 of
/// the loudest one, pauses under 300 ms bridged), in samples: the
/// reference holds speech only.
fn voiced(s: &[f32]) -> Vec<(usize, usize)> {
    let w = 320;
    let floor = s.chunks(w).map(rms).fold(0.0f32, f32::max) / 10.0;
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, c) in s.chunks(w).enumerate() {
        if rms(c) <= floor {
            continue;
        }
        let (a, b) = (i * w, (i * w + c.len()).min(s.len()));
        match out.last_mut() {
            Some(last) if a - last.1 < 300 * 16 => last.1 = b,
            _ => out.push((a, b)),
        }
    }
    out
}

/// The longest FLEURS clip of `gender`.
fn longest(gender: &str) -> Vec<f32> {
    let tsv = std::fs::read_to_string(root().join("data/fleurs/test.tsv")).unwrap();
    let file = tsv
        .lines()
        .map(|l| l.split('\t').collect::<Vec<_>>())
        .filter(|c| c.len() >= 7 && c[6] == gender)
        .max_by_key(|c| c[5].parse::<u64>().unwrap_or(0))
        .map(|c| c[1].to_string())
        .unwrap();
    read_wav(&root().join("data/fleurs/test").join(file)).unwrap()
}

#[test]
#[ignore]
fn two_speaker_der() {
    // Two FLEURS speakers (a female and a male clip, so certainly two
    // people), each cut in two halves: A1 B1 A2 B2 with 0.5 s pauses. The
    // reference turns are known exactly from the assembly.
    let (a, b) = (longest("FEMALE"), longest("MALE"));
    let (a, b) = (trim(&a), trim(&b));
    let (ah, bh) = (a.len() / 2, b.len() / 2);
    let pieces = [
        ("A", &a[..ah]),
        ("B", &b[..bh]),
        ("A", &a[ah..]),
        ("B", &b[bh..]),
    ];
    let pause = vec![0.0f32; SAMPLE_RATE as usize / 2];
    let mut audio = Vec::new();
    let mut reference = Vec::new();
    let ms = |n: usize| (n as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
    for (label, p) in pieces {
        let start = audio.len();
        audio.extend_from_slice(p);
        for (s, e) in voiced(p) {
            reference.push(Turn::new(label, ms(start + s), ms(start + e)));
        }
        audio.extend_from_slice(&pause);
    }
    let m = root().join("models");
    let emb = std::env::var("PNEX_DIAR_EMB")
        .unwrap_or_else(|_| "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx".into());
    let mut d = SherpaDiarizer::load(
        &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
        &m.join(&emb),
        &SherpaOptions::default(),
    )
    .expect("diarizer");
    let started = Instant::now();
    let r = d.diarize(&audio).unwrap();
    let infer = started.elapsed().as_secs_f64();
    let hyp: Vec<Turn> = r
        .turns
        .iter()
        .map(|t| Turn::new(format!("S{}", t.spk), t.s, t.e))
        .collect();
    let e = der(&reference, &hyp);
    let audio_s = audio.len() as f64 / f64::from(SAMPLE_RATE);
    println!("reference: {reference:?}");
    println!("hypothesis: {hyp:?}");
    println!(
        "DER {:.1} % (miss {} ms, false alarm {} ms, confusion {} ms over {} ms), {} voices, {audio_s:.1} s audio, RTF {:.3}",
        e.rate() * 100.0,
        e.miss_ms,
        e.false_alarm_ms,
        e.confusion_ms,
        e.total_ms,
        r.voices.len(),
        infer / audio_s
    );
    assert!(e.rate() < 0.35, "DER {}", e.rate());
    assert_eq!(e.confusion_ms, 0, "the two speakers are told apart");
}

/// Distinct readers of the same FLEURS sentence: one sentence id is read by
/// different people, so clips sharing an id are certainly different
/// speakers. Groups of at least `n` readers, in file order.
fn reader_groups(n: usize) -> Vec<Vec<Vec<f32>>> {
    let tsv = std::fs::read_to_string(root().join("data/fleurs/test.tsv")).unwrap();
    let mut by_id: Vec<(String, Vec<String>)> = Vec::new();
    for c in tsv.lines().map(|l| l.split('\t').collect::<Vec<_>>()) {
        if c.len() < 2 {
            continue;
        }
        match by_id.iter_mut().find(|(id, _)| id == c[0]) {
            Some((_, files)) => files.push(c[1].to_string()),
            None => by_id.push((c[0].to_string(), vec![c[1].to_string()])),
        }
    }
    by_id
        .into_iter()
        .filter(|(_, files)| files.len() >= n)
        .map(|(_, files)| {
            files[..n]
                .iter()
                .map(|f| {
                    trim(&read_wav(&root().join("data/fleurs/test").join(f)).unwrap()).to_vec()
                })
                .collect()
        })
        .collect()
}

/// A conversation of `speakers` (each clip cut in two halves, turns
/// A B C … A B C …, 0.3 s pauses) and its exact reference turns.
fn conversation(speakers: &[Vec<f32>]) -> (Vec<f32>, Vec<Turn>) {
    let pause = vec![0.0f32; SAMPLE_RATE as usize * 3 / 10];
    let ms = |n: usize| (n as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
    let (mut audio, mut reference) = (Vec::new(), Vec::new());
    for half in 0..2 {
        for (i, clip) in speakers.iter().enumerate() {
            let mid = clip.len() / 2;
            let piece = if half == 0 {
                &clip[..mid]
            } else {
                &clip[mid..]
            };
            let start = audio.len();
            audio.extend_from_slice(piece);
            for (s, e) in voiced(piece) {
                reference.push(Turn::new(format!("R{i}"), ms(start + s), ms(start + e)));
            }
            audio.extend_from_slice(&pause);
        }
    }
    (audio, reference)
}

/// Production cut: the diarizer sees 30 s windows, each on its own.
/// Returns the summed error and how many windows found fewer voices than
/// they hold.
fn windowed(d: &mut SherpaDiarizer, audio: &[f32], reference: &[Turn]) -> (Der, usize, usize) {
    let win = 30 * SAMPLE_RATE as usize;
    let (mut sum, mut under, mut windows) = (Der::default(), 0, 0);
    for (k, chunk) in audio.chunks(win).enumerate() {
        if chunk.len() < 2 * SAMPLE_RATE as usize {
            continue;
        }
        let (w0, w1) = (
            (k * 30_000) as u32,
            (k * 30_000) as u32 + (chunk.len() as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32,
        );
        let reference: Vec<Turn> = reference
            .iter()
            .filter(|t| t.end_ms > w0 && t.start_ms < w1)
            .map(|t| {
                Turn::new(
                    t.label.clone(),
                    t.start_ms.max(w0) - w0,
                    t.end_ms.min(w1) - w0,
                )
            })
            .collect();
        let r = d.diarize(chunk).unwrap();
        let hyp: Vec<Turn> = r
            .turns
            .iter()
            .map(|t| Turn::new(format!("S{}", t.spk), t.s, t.e))
            .collect();
        let e = der(&reference, &hyp);
        sum.total_ms += e.total_ms;
        sum.miss_ms += e.miss_ms;
        sum.false_alarm_ms += e.false_alarm_ms;
        sum.confusion_ms += e.confusion_ms;
        let count = |turns: &[Turn]| {
            let mut l: Vec<&str> = turns.iter().map(|t| t.label.as_str()).collect();
            l.sort_unstable();
            l.dedup();
            l.len()
        };
        windows += 1;
        if count(&hyp) < count(&reference) {
            under += 1;
        }
    }
    (sum, under, windows)
}

/// Clustering threshold sweep on 2-, 3- and 6-speaker conversations cut
/// in 30 s windows as in production. `PNEX_DIAR_THRESHOLDS` (comma
/// separated), `PNEX_DIAR_EMB`, `PNEX_DIAR_SEG` pick what is measured.
#[test]
#[ignore]
fn threshold_sweep() {
    let groups3 = reader_groups(3);
    let mut sets: Vec<(&str, Vec<(Vec<f32>, Vec<Turn>)>)> = vec![
        (
            "2 voices",
            reader_groups(2)
                .iter()
                .take(10)
                .map(|g| conversation(&g[..2]))
                .collect(),
        ),
        (
            "3 voices",
            groups3.iter().take(10).map(|g| conversation(g)).collect(),
        ),
    ];
    // Two sentence groups side by side: 6 readers, almost surely 6 people.
    let six = groups3
        .chunks(2)
        .filter(|p| p.len() == 2)
        .take(6)
        .map(|p| conversation(&[p[0].clone(), p[1].clone()].concat()))
        .collect();
    sets.push(("6 voices", six));
    let m = root().join("models");
    let emb = std::env::var("PNEX_DIAR_EMB")
        .unwrap_or_else(|_| "wespeaker_en_voxceleb_resnet34.onnx".into());
    let seg = std::env::var("PNEX_DIAR_SEG").unwrap_or_else(|_| "model.onnx".into());
    let thresholds: Vec<f32> = std::env::var("PNEX_DIAR_THRESHOLDS")
        .unwrap_or_else(|_| "0.2,0.3,0.4,0.5,0.6".into())
        .split(',')
        .map(|t| t.trim().parse().unwrap())
        .collect();
    println!("embedding {emb}, segmentation {seg}");
    for t in thresholds {
        let opts = SherpaOptions {
            cluster_threshold: t,
            ..SherpaOptions::default()
        };
        let mut d = SherpaDiarizer::load(
            &m.join("sherpa-onnx-pyannote-segmentation-3-0").join(&seg),
            &m.join(&emb),
            &opts,
        )
        .expect("diarizer");
        for (name, convs) in &sets {
            let (mut sum, mut under, mut windows) = (Der::default(), 0, 0);
            for (audio, reference) in convs {
                let (e, u, w) = windowed(&mut d, audio, reference);
                sum.total_ms += e.total_ms;
                sum.miss_ms += e.miss_ms;
                sum.false_alarm_ms += e.false_alarm_ms;
                sum.confusion_ms += e.confusion_ms;
                under += u;
                windows += w;
            }
            // Same conversations diarized whole, without the 30 s cut.
            let mut whole = Der::default();
            for (audio, reference) in convs {
                let r = d.diarize(audio).unwrap();
                let hyp: Vec<Turn> = r
                    .turns
                    .iter()
                    .map(|t| Turn::new(format!("S{}", t.spk), t.s, t.e))
                    .collect();
                let e = der(reference, &hyp);
                whole.total_ms += e.total_ms;
                whole.miss_ms += e.miss_ms;
                whole.false_alarm_ms += e.false_alarm_ms;
                whole.confusion_ms += e.confusion_ms;
            }
            println!(
                "threshold {t:.2} {name} whole: DER {:5.1} % (confusion {:5.1} %)",
                whole.rate() * 100.0,
                whole.confusion_ms as f64 * 100.0 / whole.total_ms as f64,
            );
            println!(
                "threshold {t:.2} {name}: DER {:5.1} % (confusion {:5.1} %, miss {:4.1} %, fa {:4.1} %), windows with too few voices {under}/{windows}",
                sum.rate() * 100.0,
                sum.confusion_ms as f64 * 100.0 / sum.total_ms as f64,
                sum.miss_ms as f64 * 100.0 / sum.total_ms as f64,
                sum.false_alarm_ms as f64 * 100.0 / sum.total_ms as f64,
            );
        }
    }
}

/// Voices found on a long recording diarized whole (`PNEX_DIAR_WAV`, by
/// default the 10 min radio debate), per threshold.
#[test]
#[ignore]
fn whole_file_voices() {
    let wav = std::env::var("PNEX_DIAR_WAV")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root().join("data/radio/masque-10min.wav"));
    let audio = read_wav(&wav).unwrap();
    let m = root().join("models");
    for t in [0.15f32, 0.25, 0.35, 0.5] {
        let opts = SherpaOptions {
            cluster_threshold: t,
            ..SherpaOptions::default()
        };
        let mut d = SherpaDiarizer::load(
            &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
            &m.join("wespeaker_en_voxceleb_resnet34.onnx"),
            &opts,
        )
        .expect("diarizer");
        let started = Instant::now();
        let r = d.diarize(&audio).unwrap();
        let mut talk: std::collections::BTreeMap<u32, u32> = Default::default();
        for turn in &r.turns {
            *talk.entry(turn.spk).or_default() += turn.e - turn.s;
        }
        let mut secs: Vec<u32> = talk.values().map(|ms| ms / 1000).collect();
        secs.sort_unstable_by(|a, b| b.cmp(a));
        println!(
            "threshold {t:.2}: {} voices, speaking seconds {secs:?}, {:.1} s compute",
            talk.len(),
            started.elapsed().as_secs_f64()
        );
    }
}

/// Local diarization of each 30 s window (production cut): turns on the
/// recording clock with their window, and one voice per local speaker.
fn local_windows(
    d: &mut SherpaDiarizer,
    audio: &[f32],
) -> (Vec<(usize, u32, u32, u32)>, Vec<LocalVoice>) {
    let win = 30 * SAMPLE_RATE as usize;
    let (mut turns, mut voices) = (Vec::new(), Vec::new());
    for (k, chunk) in audio.chunks(win).enumerate() {
        if chunk.len() < 2 * SAMPLE_RATE as usize {
            continue;
        }
        let off = (k * 30_000) as u32;
        let r = d.diarize(chunk).unwrap();
        for t in &r.turns {
            turns.push((k, t.spk, t.s + off, t.e + off));
        }
        for v in r.voices {
            let speech_ms = r
                .turns
                .iter()
                .filter(|t| t.spk == v.spk)
                .map(|t| t.e - t.s)
                .sum();
            voices.push(LocalVoice {
                window: k,
                spk: v.spk,
                v: v.v,
                speech_ms,
            });
        }
    }
    (turns, voices)
}

/// Recording-wide turns from per-voice labels (unlabelled voices dropped).
fn labelled(
    turns: &[(usize, u32, u32, u32)],
    voices: &[LocalVoice],
    labels: &[Option<String>],
) -> Vec<Turn> {
    turns
        .iter()
        .filter_map(|&(w, spk, s, e)| {
            let i = voices.iter().position(|v| v.window == w && v.spk == spk)?;
            labels[i].clone().map(|l| Turn::new(l, s, e))
        })
        .collect()
}

/// Today's linking: greedy, segment after segment.
fn linked_greedily(voices: &[LocalVoice]) -> Vec<Option<String>> {
    let mut linker = SpeakerLinker::new(16, std::time::Duration::from_secs(1800), 0.5);
    let mut out = vec![None; voices.len()];
    let windows: std::collections::BTreeSet<usize> = voices.iter().map(|v| v.window).collect();
    for w in windows {
        let idx: Vec<usize> = (0..voices.len())
            .filter(|&i| voices[i].window == w)
            .collect();
        let pairs: Vec<(u32, &[f32])> = idx
            .iter()
            .map(|&i| (voices[i].spk, voices[i].v.as_slice()))
            .collect();
        let map = linker.link(&pairs, Instant::now());
        for &i in &idx {
            out[i] = map.get(&voices[i].spk).cloned();
        }
    }
    out
}

fn talk(turns: &[Turn]) -> Vec<u32> {
    let mut m: std::collections::BTreeMap<&str, u32> = Default::default();
    for t in turns {
        *m.entry(t.label.as_str()).or_default() += t.end_ms - t.start_ms;
    }
    let mut v: Vec<u32> = m.values().map(|ms| ms / 1000).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

/// Whole-recording DER of today's greedy linking against the global
/// clustering pass, on FLEURS conversations and the radio debate.
#[test]
#[ignore]
fn linking_compare() {
    let m = root().join("models");
    let mut d = SherpaDiarizer::load(
        &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
        &m.join("wespeaker_en_voxceleb_resnet34.onnx"),
        &SherpaOptions::default(),
    )
    .expect("diarizer");
    let groups3 = reader_groups(3);
    let sets: Vec<(&str, Vec<(Vec<f32>, Vec<Turn>)>)> = vec![
        (
            "2 voices",
            reader_groups(2)
                .iter()
                .take(10)
                .map(|g| conversation(&g[..2]))
                .collect(),
        ),
        (
            "3 voices",
            groups3.iter().take(10).map(|g| conversation(g)).collect(),
        ),
        (
            "6 voices",
            groups3
                .chunks(2)
                .filter(|p| p.len() == 2)
                .take(6)
                .map(|p| conversation(&[p[0].clone(), p[1].clone()].concat()))
                .collect(),
        ),
    ];
    let configs: Vec<(f32, u32)> = vec![
        (0.3, 0),
        (0.4, 0),
        (0.5, 0),
        (0.4, 5_000),
        (0.5, 5_000),
        (0.6, 5_000),
    ];
    for (name, convs) in &sets {
        let locals: Vec<_> = convs
            .iter()
            .map(|(a, _)| local_windows(&mut d, a))
            .collect();
        let score = |labels_of: &dyn Fn(&[LocalVoice]) -> Vec<Option<String>>| {
            let mut sum = Der::default();
            for ((_, reference), (turns, voices)) in convs.iter().zip(&locals) {
                let e = der(reference, &labelled(turns, voices, &labels_of(voices)));
                sum.total_ms += e.total_ms;
                sum.miss_ms += e.miss_ms;
                sum.false_alarm_ms += e.false_alarm_ms;
                sum.confusion_ms += e.confusion_ms;
            }
            (
                sum.rate() * 100.0,
                sum.confusion_ms as f64 * 100.0 / sum.total_ms as f64,
            )
        };
        let (r, c) = score(&linked_greedily);
        println!("{name} today (greedy 0.5): DER {r:5.1} % confusion {c:5.1} %");
        for &(t, min) in &configs {
            let (r, c) = score(&|v: &[LocalVoice]| global_labels(v, t, min));
            println!("{name} global t={t:.1} min={min}: DER {r:5.1} % confusion {c:5.1} %");
        }
    }
    let radio = root().join("data/radio/masque-10min.wav");
    if let Ok(audio) = read_wav(&radio) {
        let (turns, voices) = local_windows(&mut d, &audio);
        println!(
            "radio today: voices {:?}",
            talk(&labelled(&turns, &voices, &linked_greedily(&voices)))
        );
        for &(t, min) in &configs {
            let l = global_labels(&voices, t, min);
            println!(
                "radio global t={t:.1} min={min}: voices {:?}",
                talk(&labelled(&turns, &voices, &l))
            );
        }
    }
}

/// Reference turns of an RTTM file within the first `until_ms`.
fn rttm(path: &std::path::Path, until_ms: u32) -> Vec<Turn> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split_whitespace().collect();
            let s = (c.get(3)?.parse::<f64>().ok()? * 1000.0) as u32;
            let e = s + (c.get(4)?.parse::<f64>().ok()? * 1000.0) as u32;
            let who = c.get(7)?.to_string();
            (s < until_ms).then(|| Turn::new(who, s, e.min(until_ms)))
        })
        .collect()
}

/// Whole-recording diarization, then voices under `min_ms` folded into
/// their closest one.
fn whole_folded(d: &mut SherpaDiarizer, audio: &[f32], min_ms: u32) -> Vec<Turn> {
    let r = d.diarize(audio).unwrap();
    let voices: Vec<LocalVoice> = r
        .voices
        .into_iter()
        .map(|v| {
            let speech_ms = r
                .turns
                .iter()
                .filter(|t| t.spk == v.spk)
                .map(|t| t.e - t.s)
                .sum();
            // Each voice its own "window": only the folding can join them.
            LocalVoice {
                window: v.spk as usize,
                spk: v.spk,
                v: v.v,
                speech_ms,
            }
        })
        .collect();
    let labels = global_labels(&voices, 2.0, min_ms);
    let turns: Vec<(usize, u32, u32, u32)> = r
        .turns
        .iter()
        .map(|t| (t.spk as usize, t.spk, t.s, t.e))
        .collect();
    labelled(&turns, &voices, &labels)
}

/// Diarization strategies on real meetings with human references (AMI,
/// CC BY 4.0; `data/ami/<meeting>.wav` = first 10 min, 16 kHz mono, and
/// `<meeting>.rttm` from BUTSpeechFIT/AMI-diarization-setup only_words).
#[test]
#[ignore]
fn ami_compare() {
    let m = root().join("models");
    let load = |t: f32| {
        SherpaDiarizer::load(
            &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
            &m.join("wespeaker_en_voxceleb_resnet34.onnx"),
            &SherpaOptions {
                cluster_threshold: t,
                ..SherpaOptions::default()
            },
        )
        .expect("diarizer")
    };
    let meetings = ["ES2004a", "IS1009a", "TS3003a", "EN2002a"];
    let mut totals: std::collections::BTreeMap<String, Der> = Default::default();
    let mut add = |name: String, meeting: &str, e: Der, voices: usize| {
        println!(
            "{meeting} {name}: DER {:5.1} % (confusion {:5.1} %), {voices} voices",
            e.rate() * 100.0,
            e.confusion_ms as f64 * 100.0 / e.total_ms as f64
        );
        let t = totals.entry(name).or_default();
        t.total_ms += e.total_ms;
        t.miss_ms += e.miss_ms;
        t.false_alarm_ms += e.false_alarm_ms;
        t.confusion_ms += e.confusion_ms;
    };
    let count = |t: &[Turn]| talk(t).iter().filter(|s| **s >= 5).count();
    for meeting in meetings {
        let dir = root().join("data/ami");
        let audio = read_wav(&dir.join(format!("{meeting}.wav"))).unwrap();
        let reference = rttm(&dir.join(format!("{meeting}.rttm")), 600_000);
        for t in [0.25f32, 0.5] {
            let mut d = load(t);
            let (turns, voices) = local_windows(&mut d, &audio);
            let h = labelled(&turns, &voices, &linked_greedily(&voices));
            add(
                format!("windows {t} + greedy link"),
                meeting,
                der(&reference, &h),
                count(&h),
            );
            let h = labelled(&turns, &voices, &global_labels(&voices, 0.5, 5_000));
            add(
                format!("windows {t} + global"),
                meeting,
                der(&reference, &h),
                count(&h),
            );
        }
        for t in [0.25f32, 0.35, 0.5] {
            let h = whole_folded(&mut load(t), &audio, 10_000);
            add(
                format!("whole {t} + fold"),
                meeting,
                der(&reference, &h),
                count(&h),
            );
        }
    }
    for (name, e) in totals {
        println!(
            "TOTAL {name}: DER {:5.1} % (miss {:4.1}, fa {:4.1}, confusion {:5.1})",
            e.rate() * 100.0,
            e.miss_ms as f64 * 100.0 / e.total_ms as f64,
            e.false_alarm_ms as f64 * 100.0 / e.total_ms as f64,
            e.confusion_ms as f64 * 100.0 / e.total_ms as f64
        );
    }
}

/// Window threshold sweep on the AMI meetings (`PNEX_DIAR_THRESHOLDS`),
/// production cut and linking: the default is chosen on real meetings.
#[test]
#[ignore]
fn ami_threshold_sweep() {
    let m = root().join("models");
    let thresholds: Vec<f32> = std::env::var("PNEX_DIAR_THRESHOLDS")
        .unwrap_or_else(|_| "0.4,0.5,0.6,0.7".into())
        .split(',')
        .map(|t| t.trim().parse().unwrap())
        .collect();
    let dir = root().join("data/ami");
    let meetings: Vec<(Vec<f32>, Vec<Turn>)> = ["ES2004a", "IS1009a", "TS3003a", "EN2002a"]
        .iter()
        .map(|mt| {
            (
                read_wav(&dir.join(format!("{mt}.wav"))).unwrap(),
                rttm(&dir.join(format!("{mt}.rttm")), 600_000),
            )
        })
        .collect();
    for t in thresholds {
        let mut d = SherpaDiarizer::load(
            &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
            &m.join("wespeaker_en_voxceleb_resnet34.onnx"),
            &SherpaOptions {
                cluster_threshold: t,
                ..SherpaOptions::default()
            },
        )
        .expect("diarizer");
        let mut sum = Der::default();
        let mut each = Vec::new();
        for (audio, reference) in &meetings {
            let (turns, voices) = local_windows(&mut d, audio);
            let e = der(
                reference,
                &labelled(&turns, &voices, &linked_greedily(&voices)),
            );
            each.push(format!("{:.1}", e.rate() * 100.0));
            sum.total_ms += e.total_ms;
            sum.miss_ms += e.miss_ms;
            sum.false_alarm_ms += e.false_alarm_ms;
            sum.confusion_ms += e.confusion_ms;
        }
        println!(
            "AMI threshold {t:.2}: DER {:5.1} % (confusion {:5.1} %) per meeting {each:?}",
            sum.rate() * 100.0,
            sum.confusion_ms as f64 * 100.0 / sum.total_ms as f64
        );
    }
}

fn normalized(v: &[f32]) -> Option<Vec<f32>> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n.is_finite() && n > 0.0).then(|| v.iter().map(|x| x / n).collect())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Global clustering pass measured on 2026-11-16 and not kept: on AMI it
/// gave 29.6 % DER against 28.8 % for the production linking (threshold
/// 0.5); kept here so the comparison can be run again.
/// One local speaker of one window, for [`global_labels`].
#[derive(Debug, Clone)]
struct LocalVoice {
    /// Window index: two voices of one window are never merged.
    window: usize,
    spk: u32,
    /// Speaker embedding (any norm).
    v: Vec<f32>,
    /// Speech of this voice in its window, ms: the weight of its vector.
    speech_ms: u32,
}

/// Global labels (`S1`, `S2`… by first window) of local voices clustered
/// over a whole recording, as pyannote does after its local windows:
/// agglomerative clustering with centroid linkage on cosine similarity,
/// merging while the closest pair is at least `threshold` similar and
/// shares no window; then a cluster with less than `min_ms` of speech is
/// folded into its closest one (an embedding from a few seconds is too
/// noisy to stand as a voice of its own).
/// Returns one label per input voice; a voice without a usable vector gets
/// `None`.
fn global_labels(voices: &[LocalVoice], threshold: f32, min_ms: u32) -> Vec<Option<String>> {
    struct Cluster {
        sum: Vec<f32>,
        windows: Vec<usize>,
        members: Vec<usize>,
        speech_ms: u64,
    }
    let unit = |c: &Cluster| normalized(&c.sum);
    let mut clusters: Vec<Cluster> = voices
        .iter()
        .enumerate()
        .filter_map(|(i, x)| {
            let v = normalized(&x.v)?;
            let w = x.speech_ms.max(1) as f32;
            Some(Cluster {
                sum: v.iter().map(|a| a * w).collect(),
                windows: vec![x.window],
                members: vec![i],
                speech_ms: u64::from(x.speech_ms),
            })
        })
        .collect();
    let disjoint = |a: &Cluster, b: &Cluster| a.windows.iter().all(|w| !b.windows.contains(w));
    let sim = |a: &Cluster, b: &Cluster| match (unit(a), unit(b)) {
        (Some(x), Some(y)) if x.len() == y.len() => dot(&x, &y),
        _ => f32::MIN,
    };
    let merge = |clusters: &mut Vec<Cluster>, i: usize, j: usize| {
        let b = clusters.swap_remove(j.max(i));
        let a = &mut clusters[j.min(i)];
        a.sum.iter_mut().zip(&b.sum).for_each(|(x, y)| *x += y);
        a.windows.extend(b.windows);
        a.members.extend(b.members);
        a.speech_ms += b.speech_ms;
    };
    // shortcut: O(n³) pair search, fine for a few hundred voices (an hour
    // in 30 s windows); use a priority queue beyond.
    loop {
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                if !disjoint(&clusters[i], &clusters[j]) {
                    continue;
                }
                let s = sim(&clusters[i], &clusters[j]);
                if s >= threshold && best.is_none_or(|b| s > b.0) {
                    best = Some((s, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        merge(&mut clusters, i, j);
    }
    // Small clusters into their closest compatible big one.
    while let Some(small) = (0..clusters.len())
        .filter(|&i| clusters[i].speech_ms < u64::from(min_ms))
        .min_by_key(|&i| clusters[i].speech_ms)
    {
        let target = (0..clusters.len())
            .filter(|&j| j != small && clusters[j].speech_ms >= u64::from(min_ms))
            .filter(|&j| disjoint(&clusters[small], &clusters[j]))
            .max_by(|&a, &b| {
                sim(&clusters[small], &clusters[a]).total_cmp(&sim(&clusters[small], &clusters[b]))
            });
        match target {
            Some(j) => merge(&mut clusters, small, j),
            // Nowhere to go: it stays a voice of its own.
            None => clusters[small].speech_ms = u64::from(min_ms),
        }
    }
    // Labels in order of first appearance.
    clusters.sort_by_key(|c| {
        c.members
            .iter()
            .map(|&m| (voices[m].window, voices[m].spk))
            .min()
    });
    let mut out = vec![None; voices.len()];
    for (k, c) in clusters.iter().enumerate() {
        for &m in &c.members {
            out[m] = Some(format!("S{}", k + 1));
        }
    }
    out
}

/// Turns of `speaker` that overlap nobody else (clean enrolment speech).
fn solo_turns(reference: &[Turn], speaker: &str) -> Vec<(u32, u32)> {
    reference
        .iter()
        .filter(|t| t.label == speaker)
        .filter(|t| {
            !reference
                .iter()
                .any(|o| o.label != speaker && o.start_ms < t.end_ms && t.start_ms < o.end_ms)
        })
        .map(|t| (t.start_ms, t.end_ms))
        .collect()
}

fn slice(audio: &[f32], s: u32, e: u32) -> &[f32] {
    let a = (s as usize * SAMPLE_RATE as usize / 1000).min(audio.len());
    let b = (e as usize * SAMPLE_RATE as usize / 1000).min(audio.len());
    &audio[a..b]
}

/// Recognition by voice prints first (media.md D978–D984) on AMI: each
/// participant's print is trained on the other meetings of the series, the
/// test meeting is cut in production windows, its turns in short pieces,
/// each piece matched against every print, the rest grouped anonymously.
#[test]
#[ignore]
fn ami_recognition() {
    use pnex_asr::sherpa::SherpaEmbedder;
    use pnex_asr::Embedder;
    use pnex_core::voice_recognition::{pieces, recognize, Piece, Print, Who};
    let m = root().join("models");
    let emb_path = m.join("wespeaker_en_voxceleb_resnet34.onnx");
    let mut embedder =
        SherpaEmbedder::load(&emb_path, &SherpaOptions::default()).expect("embedder");
    let mut d = SherpaDiarizer::load(
        &m.join("sherpa-onnx-pyannote-segmentation-3-0/model.onnx"),
        &emb_path,
        &SherpaOptions::default(),
    )
    .expect("diarizer");
    let dir = root().join("data/ami");
    let have = |mt: &str| dir.join(format!("{mt}.wav")).exists();
    let (mut base, mut reco, mut anon) = (Der::default(), Der::default(), Der::default());
    let add = |t: &mut Der, e: Der| {
        t.total_ms += e.total_ms;
        t.miss_ms += e.miss_ms;
        t.false_alarm_ms += e.false_alarm_ms;
        t.confusion_ms += e.confusion_ms;
    };
    for series in ["ES2004", "IS1009", "TS3003", "EN2002"] {
        let test = format!("{series}a");
        let others: Vec<String> = ["b", "c", "d"]
            .iter()
            .map(|x| format!("{series}{x}"))
            .filter(|mt| have(mt))
            .collect();
        if !have(&test) || others.is_empty() {
            continue;
        }
        // Prints from the other meetings: up to 60 clean pieces each.
        let mut prints = Vec::new();
        let mut names = Vec::new();
        let mut per_speaker: std::collections::BTreeMap<String, Vec<Vec<f32>>> = Default::default();
        for mt in &others {
            let audio = read_wav(&dir.join(format!("{mt}.wav"))).unwrap();
            let reference = rttm(&dir.join(format!("{mt}.rttm")), (audio.len() / 16) as u32);
            let speakers: std::collections::BTreeSet<String> =
                reference.iter().map(|t| t.label.clone()).collect();
            for sp in speakers {
                let vs = per_speaker.entry(sp.clone()).or_default();
                for (s, e) in pieces(&solo_turns(&reference, &sp)) {
                    if vs.len() >= 60 {
                        break;
                    }
                    if let Ok(v) = embedder.embed(slice(&audio, s, e)) {
                        vs.push(v);
                    }
                }
            }
        }
        for (sp, vs) in &per_speaker {
            if let Ok(t) = pnex_core::voice_id::train(vs) {
                prints.push(Print {
                    centroid: t.centroid,
                    threshold: t.threshold,
                });
                names.push(sp.clone());
            }
        }
        // The test meeting: production segmentation, then pieces.
        let audio = read_wav(&dir.join(format!("{test}.wav"))).unwrap();
        let reference = rttm(&dir.join(format!("{test}.rttm")), 600_000);
        let (turns, voices) = local_windows(&mut d, &audio);
        let today = labelled(&turns, &voices, &linked_greedily(&voices));
        let ps: Vec<Piece> = turns
            .iter()
            .flat_map(|&(w, spk, s, e)| {
                pieces(&[(s, e)])
                    .into_iter()
                    .map(move |(a, b)| (w, spk, a, b))
            })
            .map(|(w, spk, s, e)| Piece {
                start_ms: s,
                end_ms: e,
                v: (e - s >= pnex_core::voice_recognition::PIECE_MIN_MS)
                    .then(|| embedder.embed(slice(&audio, s, e)).ok())
                    .flatten(),
                group: Some(((w as u64) << 32) | u64::from(spk)),
            })
            .collect();
        let to_turns = |who: &[Who]| -> Vec<Turn> {
            ps.iter()
                .zip(who)
                .filter_map(|(p, w)| {
                    let label = match w {
                        Who::Person(i) => names[*i].clone(),
                        Who::Unknown(k) => format!("U{k}"),
                        Who::Nobody => return None,
                    };
                    Some(Turn::new(label, p.start_ms, p.end_ms))
                })
                .collect()
        };
        let with_prints = to_turns(&recognize(&ps, &prints));
        let without = to_turns(&recognize(&ps, &[]));
        let (eb, er, ea) = (
            der(&reference, &today),
            der(&reference, &with_prints),
            der(&reference, &without),
        );
        let recognized = with_prints
            .iter()
            .filter(|t| !t.label.starts_with('U'))
            .map(|t| t.end_ms - t.start_ms)
            .sum::<u32>();
        let total: u32 = with_prints.iter().map(|t| t.end_ms - t.start_ms).sum();
        println!(
            "{test} ({} prints from {others:?}): today {:5.1} % (miss {:.1} fa {:.1} conf {:.1}) | prints first {:5.1} % (miss {:.1} fa {:.1} conf {:.1}, {:.0} % of speech recognized) | anonymous pieces only {:5.1} %",
            prints.len(),
            eb.rate() * 100.0,
            eb.miss_ms as f64 * 100.0 / eb.total_ms as f64,
            eb.false_alarm_ms as f64 * 100.0 / eb.total_ms as f64,
            eb.confusion_ms as f64 * 100.0 / eb.total_ms as f64,
            er.rate() * 100.0,
            er.miss_ms as f64 * 100.0 / er.total_ms as f64,
            er.false_alarm_ms as f64 * 100.0 / er.total_ms as f64,
            er.confusion_ms as f64 * 100.0 / er.total_ms as f64,
            recognized as f64 * 100.0 / total.max(1) as f64,
            ea.rate() * 100.0,
        );
        add(&mut base, eb);
        add(&mut reco, er);
        add(&mut anon, ea);
    }
    println!(
        "TOTAL today {:5.1} % | prints first {:5.1} % | anonymous pieces only {:5.1} %",
        base.rate() * 100.0,
        reco.rate() * 100.0,
        anon.rate() * 100.0
    );
}
