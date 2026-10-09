//! whisper.cpp adapter through `whisper-rs` (D167 candidate, GGML/GGUF).

use std::path::Path;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::{AsrError, Transcriber, Transcript, Word};

#[derive(Debug, Clone)]
pub struct WhisperOptions {
    pub use_gpu: bool,
    /// Backend device index (an iGPU often comes first).
    pub gpu_device: i32,
    pub num_threads: i32,
    pub language: String,
    /// 1 = greedy, otherwise beam search width.
    pub beam_size: i32,
}

impl Default for WhisperOptions {
    fn default() -> Self {
        Self {
            use_gpu: false,
            gpu_device: 0,
            num_threads: 4,
            language: "fr".into(),
            beam_size: 1,
        }
    }
}

pub struct WhisperTranscriber {
    name: String,
    opts: WhisperOptions,
    // The state borrows nothing from the context in whisper-rs 0.16 (it
    // holds an Arc), so both live side by side.
    _ctx: WhisperContext,
    state: WhisperState,
}

impl WhisperTranscriber {
    pub fn load(model: &Path, opts: &WhisperOptions) -> Result<Self, AsrError> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(opts.use_gpu);
        params.gpu_device(opts.gpu_device);
        let ctx = WhisperContext::new_with_params(model, params)
            .map_err(|e| AsrError::Load(e.to_string()))?;
        let state = ctx
            .create_state()
            .map_err(|e| AsrError::Load(e.to_string()))?;
        let label = model
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("model");
        let device = if opts.use_gpu { "gpu" } else { "cpu" };
        Ok(Self {
            name: format!("whisper.cpp:{label}:{device}"),
            opts: opts.clone(),
            _ctx: ctx,
            state,
        })
    }
}

impl Transcriber for WhisperTranscriber {
    fn name(&self) -> &str {
        &self.name
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<Transcript, AsrError> {
        let strategy = if self.opts.beam_size > 1 {
            SamplingStrategy::BeamSearch {
                beam_size: self.opts.beam_size,
                patience: -1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(Some(&self.opts.language));
        params.set_n_threads(self.opts.num_threads);
        params.set_token_timestamps(true);
        // Segments are independent jobs (D166): no carried-over context.
        params.set_no_context(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        self.state
            .full(params, samples)
            .map_err(|e| AsrError::Inference(e.to_string()))?;

        let mut text = String::new();
        let mut words: Vec<Word> = Vec::new();
        for segment in self.state.as_iter() {
            let seg = segment
                .to_str_lossy()
                .map_err(|e| AsrError::Inference(e.to_string()))?;
            text.push_str(&seg);
            for i in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(i) else {
                    continue;
                };
                let Ok(piece) = token.to_str_lossy() else {
                    continue;
                };
                // Special tokens ([_BEG_], [_TT_123]…) carry no text.
                if piece.starts_with("[_") || piece.starts_with("<|") {
                    continue;
                }
                let data = token.token_data();
                let (t0, t1) = ((data.t0.max(0) * 10) as u32, (data.t1.max(0) * 10) as u32);
                let p = Some(token.token_probability());
                match words.last_mut() {
                    Some(last) if !piece.starts_with(' ') => {
                        last.w.push_str(&piece);
                        last.end_ms = t1.max(last.end_ms);
                    }
                    _ => {
                        let w = piece.trim_start().to_string();
                        if !w.is_empty() {
                            words.push(Word {
                                w,
                                start_ms: t0,
                                end_ms: t1,
                                p,
                            });
                        }
                    }
                }
            }
        }
        Ok(Transcript {
            text: text.trim().to_string(),
            words,
        })
    }
}
