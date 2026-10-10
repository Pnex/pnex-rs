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

#[cfg(test)]
mod tests {
    use super::*;

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
}
