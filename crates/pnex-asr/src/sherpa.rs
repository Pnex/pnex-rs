//! sherpa-onnx adapter (D167 candidate). Model files are found by name in
//! the extracted archive, int8 variants first, so an import only needs
//! the directory and the family.

use std::path::{Path, PathBuf};

use sherpa_onnx::{
    OfflineCanaryModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineTransducerModelConfig, OfflineWhisperModelConfig,
};

use crate::{words_from_tokens, AsrError, Transcriber, Transcript, SAMPLE_RATE};

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

/// Runtime options.
#[derive(Debug, Clone)]
pub struct SherpaOptions {
    /// `cpu` or `cuda`.
    pub provider: String,
    pub num_threads: i32,
    /// Language code passed to multilingual decoders (`fr`).
    pub language: String,
}

impl Default for SherpaOptions {
    fn default() -> Self {
        Self {
            provider: "cpu".into(),
            num_threads: 4,
            language: "fr".into(),
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
