//! Live measures of lot 5 (media-ingest.md §20), ignored by default: they
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

use pnex_asr::diarize::{der, Turn};
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
