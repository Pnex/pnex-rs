//! Speech-to-text for media ingestion (`docs/architecture/media-ingest.md`,
//! D166–D167). One [`Transcriber`] trait, one adapter per runtime:
//!
//! - [`sherpa`] (feature `sherpa`): sherpa-onnx, ONNX models (Parakeet,
//!   Canary, Whisper exports…);
//! - [`whisper`] (feature `whisper`): whisper.cpp through `whisper-rs`,
//!   GGML/GGUF models.
//!
//! Input is always 16 kHz mono `f32` PCM: the capture side (ffmpeg, D160)
//! resamples, this crate never does.

use std::path::Path;

pub mod diarize;
pub mod protocol;
pub mod wer;

#[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
pub mod sherpa;
#[cfg(feature = "whisper")]
pub mod whisper;

/// Sample rate every transcriber expects.
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum AsrError {
    #[error("model load failed: {0}")]
    Load(String),
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("audio: {0}")]
    Audio(String),
}

/// One recognized word with its position in the segment.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub w: String,
    pub start_ms: u32,
    pub end_ms: u32,
    /// Confidence when the runtime reports one.
    pub p: Option<f32>,
}

/// Result of one segment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// Word timings; empty when the model gives none.
    pub words: Vec<Word>,
}

/// A loaded speech-to-text model. `transcribe` takes `&mut self`: one
/// instance serves one job at a time (the worker owns a pool, D166).
pub trait Transcriber: Send {
    /// Short label for logs and benchmarks (`sherpa:parakeet-tdt-0.6b-v3`).
    fn name(&self) -> &str;
    /// Transcribes 16 kHz mono samples.
    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, AsrError>;
}

/// A loaded voice activity detector (task `vad`).
pub trait VoiceDetector: Send {
    /// Speech spans of the samples, `(start, end)` sample indexes in order;
    /// empty = silent.
    fn spans(&mut self, samples: &[f32]) -> Result<Vec<(usize, usize)>, AsrError>;

    /// Speech found in the samples, ms; 0 = silent.
    fn speech_ms(&mut self, samples: &[f32]) -> Result<u32, AsrError> {
        Ok(speech_ms_of(&self.spans(samples)?))
    }
}

/// Total length of speech spans, ms.
pub fn speech_ms_of(spans: &[(usize, usize)]) -> u32 {
    let n: usize = spans.iter().map(|(s, e)| e.saturating_sub(*s)).sum();
    (n as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32
}

/// Silence between two speech spans that makes them separate utterances,
/// samples (2 s). Shorter pauses (breaths, a music bed the VAD misses) stay
/// in one window: cutting there loses words.
const UTTERANCE_GAP: usize = 2 * SAMPLE_RATE as usize;

/// Transcribes the segment in parts cut in the middle of every long
/// silence ([`UTTERANCE_GAP`]); the parts cover the whole segment, so
/// nothing the VAD missed is dropped. Without a long silence it is one
/// plain transcription. A window holding utterances separated by silence
/// otherwise loses one of them: measured on Canary and Parakeet
/// (call-recording.md, R1.1). Word times stay on the segment clock.
pub fn transcribe_spans(
    asr: &mut dyn Transcriber,
    samples: &[f32],
    spans: &[(usize, usize)],
) -> Result<Transcript, AsrError> {
    let mut cuts = vec![0];
    cuts.extend(
        spans
            .windows(2)
            .filter(|w| w[1].0.saturating_sub(w[0].1) >= UTTERANCE_GAP)
            .map(|w| (w[0].1 + w[1].0) / 2),
    );
    if cuts.len() == 1 {
        return asr.transcribe(samples);
    }
    cuts.push(samples.len());
    let mut out = Transcript::default();
    for part in cuts.windows(2) {
        let (s, e) = (part[0], part[1].min(samples.len()));
        if s >= e {
            continue;
        }
        let t = asr.transcribe(&samples[s..e])?;
        let offset = (s as u64 * 1000 / u64::from(SAMPLE_RATE)) as u32;
        let text = t.text.trim();
        if !text.is_empty() {
            if !out.text.is_empty() {
                out.text.push(' ');
            }
            out.text.push_str(text);
        }
        out.words.extend(t.words.into_iter().map(|w| Word {
            start_ms: w.start_ms + offset,
            end_ms: w.end_ms + offset,
            ..w
        }));
    }
    Ok(out)
}

/// Speaker turns of one segment: local speaker indexes (0, 1…) and one
/// ephemeral embedding per speaker that has enough audio for one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diarization {
    pub turns: Vec<protocol::WireTurn>,
    pub voices: Vec<protocol::WireVoice>,
}

/// A loaded intra-segment diarizer (segmentation + embedding + clustering).
pub trait Diarizer: Send {
    fn diarize(&mut self, samples: &[f32]) -> Result<Diarization, AsrError>;
}

/// A loaded speaker embedding extractor (task `speaker_embedding`).
pub trait Embedder: Send {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>, AsrError>;
}

/// Reads a 16 kHz WAV file as mono `f32`; stereo is downmixed, any other
/// rate is refused (resampling belongs to the capture).
pub fn read_wav(path: &Path) -> Result<Vec<f32>, AsrError> {
    let reader = hound::WavReader::open(path).map_err(|e| AsrError::Audio(e.to_string()))?;
    samples_of(reader)
}

/// [`read_wav`] on in-memory bytes.
pub fn read_wav_bytes(bytes: &[u8]) -> Result<Vec<f32>, AsrError> {
    let reader = hound::WavReader::new(std::io::Cursor::new(bytes))
        .map_err(|e| AsrError::Audio(e.to_string()))?;
    samples_of(reader)
}

fn samples_of<R: std::io::Read>(mut reader: hound::WavReader<R>) -> Result<Vec<f32>, AsrError> {
    let spec = reader.spec();
    if spec.sample_rate != SAMPLE_RATE {
        return Err(AsrError::Audio(format!(
            "sample rate {} Hz, expected {SAMPLE_RATE}",
            spec.sample_rate
        )));
    }
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| AsrError::Audio(e.to_string()))?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()
                .map_err(|e| AsrError::Audio(e.to_string()))?
        }
    };
    let channels = usize::from(spec.channels.max(1));
    Ok(interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect())
}

/// Groups sub-word tokens into words: a token that starts with a space or
/// the SentencePiece marker opens a new word. `starts` are in seconds.
pub fn words_from_tokens(tokens: &[String], starts: &[f32], end_s: f32) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        let start = starts.get(i).copied().unwrap_or(end_s);
        let opens = tok.starts_with(' ') || tok.starts_with('\u{2581}');
        let piece = tok.trim_start_matches([' ', '\u{2581}']);
        if piece.is_empty() {
            continue;
        }
        let ms = (start * 1000.0).max(0.0) as u32;
        match words.last_mut() {
            Some(last) if !opens => last.w.push_str(piece),
            _ => {
                if let Some(last) = words.last_mut() {
                    last.end_ms = ms;
                }
                words.push(Word {
                    w: piece.to_string(),
                    start_ms: ms,
                    end_ms: ms,
                    p: None,
                });
            }
        }
    }
    if let Some(last) = words.last_mut() {
        last.end_ms = ((end_s * 1000.0) as u32).max(last.start_ms);
    }
    words
}

/// Most consecutive occurrences of one n-gram kept by [`collapse_repeats`].
pub const MAX_REPEATS: usize = 3;
/// Longest n-gram (in words) looked for by [`collapse_repeats`].
const MAX_NGRAM: usize = 4;

fn repeat_key(w: &str) -> String {
    w.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

/// Which items survive: in a run where the same 1–4 item n-gram repeats
/// more than `max` times in a row, only its first `max` occurrences stay.
fn loop_mask(keys: &[String], max: usize) -> Vec<bool> {
    let n = keys.len();
    let mut keep = vec![true; n];
    let mut i = 0;
    'outer: while i < n {
        for size in 1..=MAX_NGRAM.min(n - i) {
            let gram = &keys[i..i + size];
            let mut k = 1;
            while i + (k + 1) * size <= n && keys[i + k * size..i + (k + 1) * size] == *gram {
                k += 1;
            }
            if k > max {
                keep[i + max * size..i + k * size].fill(false);
                i += k * size;
                continue 'outer;
            }
        }
        i += 1;
    }
    keep
}

/// Cuts decoding loops (Whisper on music: "de plus de plus de plus…") from
/// a transcript's text and words: an n-gram of 1–4 words repeated more than
/// [`MAX_REPEATS`] times in a row keeps [`MAX_REPEATS`] occurrences (O56).
/// Case and surrounding punctuation are ignored when comparing words.
pub fn collapse_repeats(mut t: Transcript) -> Transcript {
    let cleaned = {
        let words: Vec<&str> = t.text.split_whitespace().collect();
        let keys: Vec<String> = words.iter().map(|w| repeat_key(w)).collect();
        let keep = loop_mask(&keys, MAX_REPEATS);
        keep.contains(&false).then(|| {
            let kept: Vec<&str> = words
                .iter()
                .zip(&keep)
                .filter_map(|(w, k)| k.then_some(*w))
                .collect();
            kept.join(" ")
        })
    };
    if let Some(text) = cleaned {
        t.text = text;
    }
    let keys: Vec<String> = t.words.iter().map(|w| repeat_key(&w.w)).collect();
    let mut keep = loop_mask(&keys, MAX_REPEATS).into_iter();
    t.words.retain(|_| keep.next().unwrap_or(true));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Says how many samples it got, with one word over its whole input.
    struct Counter(Vec<usize>);

    impl Transcriber for Counter {
        fn name(&self) -> &str {
            "counter"
        }
        fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, AsrError> {
            self.0.push(samples.len());
            let end_ms = (samples.len() as u64 * 1000 / 16_000) as u32;
            Ok(Transcript {
                text: format!("w{}", self.0.len()),
                words: vec![Word {
                    w: format!("w{}", self.0.len()),
                    start_ms: 0,
                    end_ms,
                    p: None,
                }],
            })
        }
    }

    #[test]
    fn utterances_apart_by_a_long_silence_are_transcribed_alone() {
        let samples = vec![0.0f32; 30 * 16_000];
        // Speech 1-3 s and 10-12 s (7 s of silence): cut at 6.5 s, the two
        // parts cover the whole segment, words on the segment clock.
        let mut asr = Counter(Vec::new());
        let t =
            transcribe_spans(&mut asr, &samples, &[(16_000, 48_000), (160_000, 192_000)]).unwrap();
        assert_eq!(t.text, "w1 w2");
        assert_eq!(asr.0, [104_000, 480_000 - 104_000]);
        assert_eq!((t.words[0].start_ms, t.words[0].end_ms), (0, 6500));
        assert_eq!((t.words[1].start_ms, t.words[1].end_ms), (6500, 30_000));
        // Short pauses (1.5 s): one plain transcription of the whole segment.
        let mut asr = Counter(Vec::new());
        let t = transcribe_spans(
            &mut asr,
            &samples,
            &[(16_000, 48_000), (72_000, 100_000), (124_000, 130_000)],
        )
        .unwrap();
        assert_eq!(asr.0, [480_000]);
        assert_eq!(t.text, "w1");
        assert_eq!(speech_ms_of(&[(0, 16_000), (32_000, 40_000)]), 1500);
    }

    #[test]
    fn tokens_are_grouped_into_words() {
        let toks: Vec<String> = ["\u{2581}bon", "jour", "\u{2581}à", " tous"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let words = words_from_tokens(&toks, &[0.0, 0.2, 0.5, 0.7], 1.0);
        let text: Vec<&str> = words.iter().map(|w| w.w.as_str()).collect();
        assert_eq!(text, ["bonjour", "à", "tous"]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 500));
        assert_eq!(words[2].end_ms, 1000);
    }

    fn text_of(text: &str) -> String {
        collapse_repeats(Transcript {
            text: text.into(),
            words: Vec::new(),
        })
        .text
    }

    #[test]
    fn decoding_loops_are_collapsed() {
        assert_eq!(
            text_of("de plus de plus de plus de plus de plus fin"),
            "de plus de plus de plus fin"
        );
        assert_eq!(
            text_of("très très très très très bien"),
            "très très très bien"
        );
        assert_eq!(text_of("Plus, plus, plus. plus plus"), "Plus, plus, plus.");
        assert_eq!(
            text_of("a b c d a b c d a b c d a b c d"),
            "a b c d a b c d a b c d"
        );
        // Legitimate repetitions and spacing stay untouched.
        assert_eq!(text_of("c'est très  très bien"), "c'est très  très bien");
        assert_eq!(text_of("non non non"), "non non non");
        assert_eq!(text_of(""), "");
    }

    #[test]
    fn looped_words_are_dropped_with_the_text() {
        let word = |w: &str, ms: u32| Word {
            w: w.into(),
            start_ms: ms,
            end_ms: ms + 100,
            p: None,
        };
        let t = collapse_repeats(Transcript {
            text: "la la la la la".into(),
            words: (0..5)
                .map(|i| word("la", i * 100))
                .chain([word("fin", 600)])
                .collect(),
        });
        assert_eq!(t.text, "la la la");
        let kept: Vec<u32> = t.words.iter().map(|w| w.start_ms).collect();
        assert_eq!(kept, [0, 100, 200, 600]);
    }
}
