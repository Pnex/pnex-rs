//! sherpa-onnx adapters (D167): speech-to-text, Silero VAD, speaker
//! embeddings and offline diarization. Model files are found by name in
//! the extracted archive, int8 variants first, so an import only needs
//! the directory and the family.

use std::path::{Path, PathBuf};

use sherpa_onnx::{
    OfflineCanaryModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig, OfflineTransducerModelConfig,
    OfflineWhisperModelConfig, SileroVadModelConfig, SpeakerEmbeddingExtractor,
    SpeakerEmbeddingExtractorConfig, VadModelConfig, VoiceActivityDetector,
};

use crate::protocol::{WireTurn, WireVoice};
use crate::{
    words_from_tokens, AsrError, Diarization, Diarizer, Embedder, Transcriber, Transcript,
    VoiceDetector, SAMPLE_RATE,
};

/// Model families wired in the POC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SherpaFamily {
    /// NeMo transducer (Parakeet TDT).
    ParakeetTdt,
    /// NeMo Canary (encoder-decoder, src/tgt language).
    Canary,
    /// OpenAI Whisper ONNX export.
    Whisper,
}

/// Clustering threshold measured on the two-speaker sample (§20).
pub const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.5;

/// Runtime options.
#[derive(Debug, Clone)]
pub struct SherpaOptions {
    /// `cpu` or `cuda`.
    pub provider: String,
    pub num_threads: i32,
    /// Language code passed to multilingual decoders (`fr`).
    pub language: String,
    /// Distance threshold of the diarization clustering: larger = fewer
    /// speakers.
    pub cluster_threshold: f32,
}

impl Default for SherpaOptions {
    fn default() -> Self {
        Self {
            provider: "cpu".into(),
            num_threads: 4,
            language: "fr".into(),
            cluster_threshold: DEFAULT_CLUSTER_THRESHOLD,
        }
    }
}

pub struct SherpaTranscriber {
    name: String,
    recognizer: OfflineRecognizer,
}

impl SherpaTranscriber {
    pub fn load(family: SherpaFamily, dir: &Path, opts: &SherpaOptions) -> Result<Self, AsrError> {
        let mut config = OfflineRecognizerConfig::default();
        let model = &mut config.model_config;
        model.tokens = Some(path_str(&pick(dir, "tokens", "txt")?));
        model.num_threads = opts.num_threads;
        model.provider = Some(opts.provider.clone());
        match family {
            SherpaFamily::ParakeetTdt => {
                model.transducer = OfflineTransducerModelConfig {
                    encoder: Some(path_str(&pick(dir, "encoder", "onnx")?)),
                    decoder: Some(path_str(&pick(dir, "decoder", "onnx")?)),
                    joiner: Some(path_str(&pick(dir, "joiner", "onnx")?)),
                };
                model.model_type = Some("nemo_transducer".into());
            }
            SherpaFamily::Canary => {
                model.canary = OfflineCanaryModelConfig {
                    encoder: Some(path_str(&pick(dir, "encoder", "onnx")?)),
                    decoder: Some(path_str(&pick(dir, "decoder", "onnx")?)),
                    src_lang: Some(opts.language.clone()),
                    tgt_lang: Some(opts.language.clone()),
                    use_pnc: true,
                };
            }
            SherpaFamily::Whisper => {
                model.whisper = OfflineWhisperModelConfig {
                    encoder: Some(path_str(&pick(dir, "encoder", "onnx")?)),
                    decoder: Some(path_str(&pick(dir, "decoder", "onnx")?)),
                    language: Some(opts.language.clone()),
                    task: Some("transcribe".into()),
                    tail_paddings: -1,
                    enable_token_timestamps: true,
                    enable_segment_timestamps: false,
                };
            }
        }
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| AsrError::Load(format!("sherpa-onnx refused {}", dir.display())))?;
        let label = dir.file_name().and_then(|n| n.to_str()).unwrap_or("model");
        Ok(Self {
            name: format!("sherpa:{label}:{}", opts.provider),
            recognizer,
        })
    }
}

impl Transcriber for SherpaTranscriber {
    fn name(&self) -> &str {
        &self.name
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, AsrError> {
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .ok_or_else(|| AsrError::Inference("no result".into()))?;
        let end_s = samples.len() as f32 / SAMPLE_RATE as f32;
        let words = match &result.timestamps {
            Some(ts) if ts.len() == result.tokens.len() => {
                words_from_tokens(&result.tokens, ts, end_s)
            }
            _ => Vec::new(),
        };
        Ok(Transcript {
            text: result.text.trim().to_string(),
            words,
        })
    }
}

/// Finds `*<stem>*.<ext>` in `dir`, preferring an int8 variant.
fn pick(dir: &Path, stem: &str, ext: &str) -> Result<PathBuf, AsrError> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| AsrError::Load(format!("{}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.contains(stem) && p.extension().and_then(|e| e.to_str()) == Some(ext)
        })
        .collect();
    found.sort_by_key(|p| !p.to_string_lossy().contains("int8"));
    found
        .into_iter()
        .next()
        .ok_or_else(|| AsrError::Load(format!("no *{stem}*.{ext} in {}", dir.display())))
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Silero VAD settings (sherpa-onnx example values).
const VAD_WINDOW: usize = 512;
/// Longest segment the VAD buffer holds, s (segments are ≤ 120 s, D159).
const VAD_BUFFER_SECS: f32 = 130.0;
/// Audio of one speaker given to the embedding extractor, at most.
const VOICE_MAX_SECS: usize = 30;

pub struct SherpaVad {
    vad: VoiceActivityDetector,
}

impl SherpaVad {
    pub fn load(model: &Path, opts: &SherpaOptions) -> Result<Self, AsrError> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path_str(model)),
                threshold: 0.5,
                min_silence_duration: 0.25,
                min_speech_duration: 0.25,
                window_size: VAD_WINDOW as i32,
                max_speech_duration: 20.0,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some(opts.provider.clone()),
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&config, VAD_BUFFER_SECS)
            .ok_or_else(|| AsrError::Load(format!("sherpa-onnx refused {}", model.display())))?;
        Ok(Self { vad })
    }
}

impl VoiceDetector for SherpaVad {
    fn speech_ms(&mut self, samples: &[f32]) -> Result<u32, AsrError> {
        self.vad.reset();
        for chunk in samples.chunks(VAD_WINDOW) {
            self.vad.accept_waveform(chunk);
        }
        self.vad.flush();
        let mut n: u64 = 0;
        while let Some(seg) = self.vad.front() {
            n += u64::try_from(seg.n()).unwrap_or(0);
            drop(seg);
            self.vad.pop();
        }
        Ok((n * 1000 / u64::from(SAMPLE_RATE)) as u32)
    }
}

pub struct SherpaEmbedder {
    extractor: SpeakerEmbeddingExtractor,
}

fn embedding_config(model: &Path, opts: &SherpaOptions) -> SpeakerEmbeddingExtractorConfig {
    SpeakerEmbeddingExtractorConfig {
        model: Some(path_str(model)),
        num_threads: opts.num_threads,
        debug: false,
        provider: Some(opts.provider.clone()),
    }
}

impl SherpaEmbedder {
    pub fn load(model: &Path, opts: &SherpaOptions) -> Result<Self, AsrError> {
        let extractor = SpeakerEmbeddingExtractor::create(&embedding_config(model, opts))
            .ok_or_else(|| AsrError::Load(format!("sherpa-onnx refused {}", model.display())))?;
        Ok(Self { extractor })
    }
}

impl Embedder for SherpaEmbedder {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>, AsrError> {
        let stream = self
            .extractor
            .create_stream()
            .ok_or_else(|| AsrError::Inference("no embedding stream".into()))?;
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        stream.input_finished();
        if !self.extractor.is_ready(&stream) {
            return Err(AsrError::Inference(
                "too little audio for an embedding".into(),
            ));
        }
        self.extractor
            .compute(&stream)
            .ok_or_else(|| AsrError::Inference("no embedding".into()))
    }
}

/// pyannote segmentation + embedding extractor + clustering (sherpa-onnx
/// offline speaker diarization), then one embedding per local speaker for
/// the cross-segment linking done by the server.
pub struct SherpaDiarizer {
    diarizer: OfflineSpeakerDiarization,
    embedder: SherpaEmbedder,
}

impl SherpaDiarizer {
    pub fn load(
        segmentation: &Path,
        embedding: &Path,
        opts: &SherpaOptions,
    ) -> Result<Self, AsrError> {
        let mut config = OfflineSpeakerDiarizationConfig::default();
        config.segmentation.pyannote.model = Some(path_str(segmentation));
        config.segmentation.num_threads = opts.num_threads;
        config.segmentation.provider = Some(opts.provider.clone());
        config.embedding = embedding_config(embedding, opts);
        config.clustering.threshold = opts.cluster_threshold;
        let diarizer = OfflineSpeakerDiarization::create(&config).ok_or_else(|| {
            AsrError::Load(format!(
                "sherpa-onnx refused {} + {}",
                segmentation.display(),
                embedding.display()
            ))
        })?;
        Ok(Self {
            diarizer,
            embedder: SherpaEmbedder::load(embedding, opts)?,
        })
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&mut self, samples: &[f32]) -> Result<Diarization, AsrError> {
        let result = self
            .diarizer
            .process(samples)
            .ok_or_else(|| AsrError::Inference("diarization failed".into()))?;
        let ms = |s: f32| (s.max(0.0) * 1000.0) as u32;
        let turns: Vec<WireTurn> = result
            .sort_by_start_time()
            .into_iter()
            .filter(|s| s.speaker >= 0 && s.end > s.start)
            .map(|s| WireTurn {
                spk: s.speaker as u32,
                s: ms(s.start),
                e: ms(s.end),
            })
            .collect();
        let mut speakers: Vec<u32> = turns.iter().map(|t| t.spk).collect();
        speakers.sort_unstable();
        speakers.dedup();
        let rate = SAMPLE_RATE as usize;
        let mut voices = Vec::new();
        for spk in speakers {
            let mut buf: Vec<f32> = Vec::new();
            for t in turns.iter().filter(|t| t.spk == spk) {
                let a = (t.s as usize * rate / 1000).min(samples.len());
                let b = (t.e as usize * rate / 1000).min(samples.len());
                buf.extend_from_slice(&samples[a..b]);
                if buf.len() >= VOICE_MAX_SECS * rate {
                    break;
                }
            }
            buf.truncate(VOICE_MAX_SECS * rate);
            // A speaker with too little audio gets no voice: it stays
            // unlabelled rather than being linked by chance.
            if let Ok(v) = self.embedder.embed(&buf) {
                voices.push(WireVoice { spk, v });
            }
        }
        Ok(Diarization { turns, voices })
    }
}
