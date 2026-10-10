//! `pnex-asr`: the speech-to-text process of PNEX (media-ingest.md
//! D166–D167). The server spawns it and talks JSON lines
//! ([`pnex_asr::protocol`]).
//!
//! ```text
//! pnex-asr inspect --dir <model dir>
//! pnex-asr serve --dir <model dir> [--family F] [--language fr]
//!                [--threads 4] [--provider cpu|cuda] [--gpu] [--gpu-device N]
//! ```
//!
//! Which runtimes are compiled in is decided at build time (features
//! `sherpa`, `sherpa-shared`, `whisper`, `whisper-vulkan`); a family whose
//! runtime is missing fails at `serve` with a clear message.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine;
use pnex_asr::protocol::{self, Family, Fatal, Ready, Request, Response, WireWord};
use pnex_asr::Transcriber;

struct Args {
    cmd: String,
    dir: Option<PathBuf>,
    family: Option<String>,
    language: String,
    threads: i32,
    provider: String,
    gpu: bool,
    gpu_device: i32,
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
            use pnex_asr::sherpa::{SherpaFamily, SherpaOptions, SherpaTranscriber};
            let f = match family {
                Family::ParakeetTdt => SherpaFamily::ParakeetTdt,
                Family::Canary => SherpaFamily::Canary,
                _ => SherpaFamily::Whisper,
            };
            let opts = SherpaOptions {
                provider: a.provider.clone(),
                num_threads: a.threads,
                language: a.language.clone(),
            };
            SherpaTranscriber::load(f, dir, &opts)
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

fn serve(a: &Args) {
    let Some(dir) = a.dir.as_deref() else {
        fatal("--dir is required");
    };
    let inspection = protocol::inspect(dir).unwrap_or_else(|e| fatal(e));
    let family = match a.family.as_deref() {
        Some(f) => Family::from_wire(f).unwrap_or_else(|| fatal(format!("unknown family {f}"))),
        None => inspection.family,
    };
    let started = Instant::now();
    let mut model = load(family, a, dir, &inspection.files).unwrap_or_else(|e| fatal(e));
    print_line(&Ready {
        ready: model.name().to_string(),
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
        let result = base64::engine::general_purpose::STANDARD
            .decode(req.wav_b64.as_bytes())
            .map_err(|e| e.to_string())
            .and_then(|wav| pnex_asr::read_wav_bytes(&wav).map_err(|e| e.to_string()))
            .and_then(|samples| model.transcribe(&samples).map_err(|e| e.to_string()));
        let ms = started.elapsed().as_millis() as u64;
        let resp = match result {
            Ok(t) => Response {
                id: req.id,
                text: Some(t.text),
                words: t
                    .words
                    .into_iter()
                    .map(|w| WireWord {
                        w: w.w,
                        s: w.start_ms,
                        e: w.end_ms,
                        p: w.p,
                    })
                    .collect(),
                ms,
                error: None,
            },
            Err(e) => Response {
                id: req.id,
                text: None,
                words: Vec::new(),
                ms,
                error: Some(e),
            },
        };
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
