//! `pnex-asr`: the speech-to-text process of PNEX (media-ingest.md
//! D166–D167). The server spawns it and talks JSON lines
//! ([`pnex_asr::protocol`]).
//!
//! ```text
//! pnex-asr inspect --dir <model dir>
//! pnex-asr serve --dir <model dir> [--family F] [--language fr]
//!                [--threads 4] [--provider cpu|cuda] [--gpu] [--gpu-device N]
//!                [--vad-dir <dir>] [--seg-dir <dir> --emb-dir <dir>]
//! ```
//!
//! The family of `--dir` sets what the process serves: a speech-to-text
//! model (optionally preceded by a VAD, `--vad-dir`, and followed by an
//! intra-segment diarization, `--seg-dir` + `--emb-dir`), or one VAD,
//! segmentation (needs `--emb-dir`) or embedding model alone (model check).
//!
//! Which runtimes are compiled in is decided at build time (features
//! `sherpa`, `sherpa-shared`, `whisper`, `whisper-vulkan`); a family whose
//! runtime is missing fails at `serve` with a clear message.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine;
use pnex_asr::protocol::{self, Family, Fatal, Ready, Request, Response, WireVoice, WireWord};
use pnex_asr::{Diarizer, Embedder, Transcriber, VoiceDetector};

struct Args {
    cmd: String,
    dir: Option<PathBuf>,
    family: Option<String>,
    language: String,
    threads: i32,
    provider: String,
    gpu: bool,
    gpu_device: i32,
    vad_dir: Option<PathBuf>,
    seg_dir: Option<PathBuf>,
    emb_dir: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it
        .next()
        .ok_or("usage: pnex-asr inspect|serve --dir <model dir>")?;
    let mut a = Args {
        cmd,
        dir: None,
        family: None,
        language: "fr".into(),
        threads: 4,
        provider: "cpu".into(),
        gpu: false,
        gpu_device: 0,
        vad_dir: None,
        seg_dir: None,
        emb_dir: None,
    };
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--dir" => a.dir = Some(PathBuf::from(value()?)),
            "--family" => a.family = Some(value()?),
            "--language" => a.language = value()?,
            "--threads" => a.threads = value()?.parse().map_err(|_| "bad --threads")?,
            "--provider" => a.provider = value()?,
            "--gpu" => a.gpu = true,
            "--gpu-device" => a.gpu_device = value()?.parse().map_err(|_| "bad --gpu-device")?,
            "--vad-dir" => a.vad_dir = Some(PathBuf::from(value()?)),
            "--seg-dir" => a.seg_dir = Some(PathBuf::from(value()?)),
            "--emb-dir" => a.emb_dir = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(a)
}

fn print_line<T: serde::Serialize>(value: &T) {
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, value);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn fatal(msg: impl Into<String>) -> ! {
    print_line(&Fatal { fatal: msg.into() });
    std::process::exit(2);
}

#[allow(unused_variables)] // a build without runtimes uses none of them
fn load(
    family: Family,
    a: &Args,
    dir: &std::path::Path,
    files: &[String],
) -> Result<Box<dyn Transcriber>, String> {
    match family {
        #[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
        Family::ParakeetTdt | Family::Canary | Family::Whisper => {
            use pnex_asr::sherpa::{SherpaFamily, SherpaTranscriber};
            let f = match family {
                Family::ParakeetTdt => SherpaFamily::ParakeetTdt,
                Family::Canary => SherpaFamily::Canary,
                _ => SherpaFamily::Whisper,
            };
            SherpaTranscriber::load(f, dir, &sherpa_opts(a))
                .map(|t| Box::new(t) as Box<dyn Transcriber>)
                .map_err(|e| e.to_string())
        }
        #[cfg(feature = "whisper")]
        Family::WhisperGgml => {
            use pnex_asr::whisper::{WhisperOptions, WhisperTranscriber};
            let opts = WhisperOptions {
                use_gpu: a.gpu,
                gpu_device: a.gpu_device,
                num_threads: a.threads,
                language: a.language.clone(),
                beam_size: 1,
            };
            let file = files.first().ok_or("no model file")?;
            WhisperTranscriber::load(&dir.join(file), &opts)
                .map(|t| Box::new(t) as Box<dyn Transcriber>)
                .map_err(|e| e.to_string())
        }
        #[allow(unreachable_patterns)]
        other => Err(format!(
            "runtime {} not compiled in this pnex-asr build",
            other.runtime()
        )),
    }
}

#[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
fn sherpa_opts(a: &Args) -> pnex_asr::sherpa::SherpaOptions {
    pnex_asr::sherpa::SherpaOptions {
        provider: a.provider.clone(),
        num_threads: a.threads,
        language: a.language.clone(),
        ..Default::default()
    }
}

/// The single model file of a VAD / segmentation / embedding directory.
fn single_file(dir: &std::path::Path, want: Family) -> Result<PathBuf, String> {
    let i = protocol::inspect(dir)?;
    if i.family != want {
        return Err(format!("{}: not a {} model", dir.display(), want.wire()));
    }
    i.files
        .first()
        .map(|f| dir.join(f))
        .ok_or_else(|| "no model file".into())
}

#[allow(unused_variables)]
fn load_vad(a: &Args, dir: &std::path::Path) -> Result<Box<dyn VoiceDetector>, String> {
    let file = single_file(dir, Family::SileroVad)?;
    #[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
    return pnex_asr::sherpa::SherpaVad::load(&file, &sherpa_opts(a))
        .map(|v| Box::new(v) as Box<dyn VoiceDetector>)
        .map_err(|e| e.to_string());
    #[allow(unreachable_code)]
    Err("runtime sherpa-onnx not compiled in this pnex-asr build".into())
}

#[allow(unused_variables)]
fn load_embedder(a: &Args, dir: &std::path::Path) -> Result<Box<dyn Embedder>, String> {
    let file = single_file(dir, Family::SpeakerEmbedding)?;
    #[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
    return pnex_asr::sherpa::SherpaEmbedder::load(&file, &sherpa_opts(a))
        .map(|v| Box::new(v) as Box<dyn Embedder>)
        .map_err(|e| e.to_string());
    #[allow(unreachable_code)]
    Err("runtime sherpa-onnx not compiled in this pnex-asr build".into())
}

#[allow(unused_variables)]
fn load_diarizer(
    a: &Args,
    seg: &std::path::Path,
    emb: Option<&std::path::Path>,
) -> Result<Box<dyn Diarizer>, String> {
    let emb = emb.ok_or("a segmentation model needs --emb-dir")?;
    let seg_file = single_file(seg, Family::PyannoteSegmentation)?;
    let emb_file = single_file(emb, Family::SpeakerEmbedding)?;
    #[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
    return pnex_asr::sherpa::SherpaDiarizer::load(&seg_file, &emb_file, &sherpa_opts(a))
        .map(|v| Box::new(v) as Box<dyn Diarizer>)
        .map_err(|e| e.to_string());
    #[allow(unreachable_code)]
    Err("runtime sherpa-onnx not compiled in this pnex-asr build".into())
}

/// What one `serve` process runs on each segment, in order.
#[derive(Default)]
struct Pipeline {
    vad: Option<Box<dyn VoiceDetector>>,
    asr: Option<Box<dyn Transcriber>>,
    diarizer: Option<Box<dyn Diarizer>>,
    embedder: Option<Box<dyn Embedder>>,
    name: String,
}

impl Pipeline {
    fn run(&mut self, samples: &[f32], resp: &mut Response) -> Result<(), String> {
        if let Some(vad) = self.vad.as_mut() {
            let speech = vad.speech_ms(samples).map_err(|e| e.to_string())?;
            resp.speech_ms = Some(speech);
            if speech == 0 {
                // Silent segment: no transcription, no diarization (D166).
                return Ok(());
            }
        }
        if let Some(asr) = self.asr.as_mut() {
            let t = asr.transcribe(samples).map_err(|e| e.to_string())?;
            resp.text = Some(t.text);
            resp.words = t
                .words
                .into_iter()
                .map(|w| WireWord {
                    w: w.w,
                    s: w.start_ms,
                    e: w.end_ms,
                    p: w.p,
                    spk: None,
                })
                .collect();
        }
        if let Some(d) = self.diarizer.as_mut() {
            let r = d.diarize(samples).map_err(|e| e.to_string())?;
            resp.turns = r.turns;
            resp.voices = r.voices;
        }
        if let Some(e) = self.embedder.as_mut() {
            let v = e.embed(samples).map_err(|e| e.to_string())?;
            resp.voices = vec![WireVoice { spk: 0, v }];
        }
        Ok(())
    }
}

fn pipeline(a: &Args, dir: &std::path::Path) -> Result<Pipeline, String> {
    let inspection = protocol::inspect(dir)?;
    let family = match a.family.as_deref() {
        Some(f) => Family::from_wire(f).ok_or_else(|| format!("unknown family {f}"))?,
        None => inspection.family,
    };
    let mut p = Pipeline {
        name: format!("{}:{}", family.wire(), a.provider),
        ..Default::default()
    };
    match family {
        Family::SileroVad => p.vad = Some(load_vad(a, dir)?),
        Family::PyannoteSegmentation => {
            p.diarizer = Some(load_diarizer(a, dir, a.emb_dir.as_deref())?)
        }
        Family::SpeakerEmbedding => p.embedder = Some(load_embedder(a, dir)?),
        _ => {
            let asr = load(family, a, dir, &inspection.files)?;
            p.name = asr.name().to_string();
            p.asr = Some(asr);
            if let Some(v) = a.vad_dir.as_deref() {
                p.vad = Some(load_vad(a, v)?);
            }
            if let Some(s) = a.seg_dir.as_deref() {
                p.diarizer = Some(load_diarizer(a, s, a.emb_dir.as_deref())?);
            }
        }
    }
    Ok(p)
}

fn serve(a: &Args) {
    let Some(dir) = a.dir.as_deref() else {
        fatal("--dir is required");
    };
    let started = Instant::now();
    let mut pipeline = pipeline(a, dir).unwrap_or_else(|e| fatal(e));
    print_line(&Ready {
        ready: pipeline.name.clone(),
        load_ms: started.elapsed().as_millis() as u64,
    });
    let stdin = std::io::stdin().lock();
    for line in stdin.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => fatal(format!("bad request: {e}")),
        };
        let started = Instant::now();
        let mut resp = Response {
            id: req.id,
            ..Default::default()
        };
        let result = base64::engine::general_purpose::STANDARD
            .decode(req.wav_b64.as_bytes())
            .map_err(|e| e.to_string())
            .and_then(|wav| pnex_asr::read_wav_bytes(&wav).map_err(|e| e.to_string()))
            .and_then(|samples| pipeline.run(&samples, &mut resp));
        resp.ms = started.elapsed().as_millis() as u64;
        if let Err(e) = result {
            resp = Response {
                id: req.id,
                ms: resp.ms,
                error: Some(e),
                ..Default::default()
            };
        }
        print_line(&resp);
    }
}

fn main() {
    let a = parse_args().unwrap_or_else(|e| fatal(e));
    match a.cmd.as_str() {
        "inspect" => {
            let Some(dir) = a.dir.as_deref() else {
                fatal("--dir is required");
            };
            match protocol::inspect(dir) {
                Ok(i) => print_line(&i),
                Err(e) => fatal(e),
            }
        }
        "serve" => serve(&a),
        other => fatal(format!("unknown command {other}")),
    }
}
