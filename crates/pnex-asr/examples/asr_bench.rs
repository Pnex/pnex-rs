//! Lot 0 benchmark (media-ingest.md §12): WER, proper-noun recall and real
//! time factor of every model given on the command line.
//!
//! ```text
//! asr_bench --fleurs <dir with test.tsv + test/*.wav> [--limit N]
//!           [--wav <long 16 kHz file> --chunk 30]
//!           [--threads 4] [--provider cpu|cuda] [--gpu] [--gpu-device 1]
//!           [--out <dir for hypotheses>]
//!           --model sherpa:parakeet:<dir> --model sherpa:canary:<dir>
//!           --model sherpa:whisper:<dir> --model whisper:<ggml file> …
//! ```
//!
//! FLEURS `test.tsv` columns: id, file, raw transcription, normalized
//! transcription, … (tab separated, no header).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pnex_asr::wer::{normalize, proper_nouns, WerAccumulator};
use pnex_asr::{read_wav, Transcriber, SAMPLE_RATE};

struct Args {
    fleurs: Option<PathBuf>,
    limit: usize,
    wav: Option<PathBuf>,
    chunk_s: usize,
    threads: i32,
    provider: String,
    gpu: bool,
    gpu_device: i32,
    out: Option<PathBuf>,
    models: Vec<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        fleurs: None,
        limit: usize::MAX,
        wav: None,
        chunk_s: 30,
        threads: 4,
        provider: "cpu".into(),
        gpu: false,
        gpu_device: 0,
        out: None,
        models: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| panic!("{flag} needs a value"));
        match flag.as_str() {
            "--fleurs" => a.fleurs = Some(value().into()),
            "--limit" => a.limit = value().parse().expect("--limit"),
            "--wav" => a.wav = Some(value().into()),
            "--chunk" => a.chunk_s = value().parse().expect("--chunk"),
            "--threads" => a.threads = value().parse().expect("--threads"),
            "--provider" => a.provider = value(),
            "--gpu" => a.gpu = true,
            "--gpu-device" => a.gpu_device = value().parse().expect("--gpu-device"),
            "--out" => a.out = Some(value().into()),
            "--model" => a.models.push(value()),
            other => panic!("unknown flag {other}"),
        }
    }
    a
}

#[allow(unused_variables)] // a build without runtimes uses none of them
fn load(spec: &str, a: &Args) -> Result<Box<dyn Transcriber>, String> {
    let (runtime, rest) = spec.split_once(':').ok_or("model spec: runtime:…")?;
    match runtime {
        #[cfg(any(feature = "sherpa", feature = "sherpa-shared"))]
        "sherpa" => {
            use pnex_asr::sherpa::{SherpaFamily, SherpaOptions, SherpaTranscriber};
            let (family, dir) = rest.split_once(':').ok_or("sherpa:<family>:<dir>")?;
            let family = match family {
                "parakeet" => SherpaFamily::ParakeetTdt,
                "canary" => SherpaFamily::Canary,
                "whisper" => SherpaFamily::Whisper,
                f => return Err(format!("unknown sherpa family {f}")),
            };
            let opts = SherpaOptions {
                provider: a.provider.clone(),
                num_threads: a.threads,
                language: "fr".into(),
            };
            SherpaTranscriber::load(family, Path::new(dir), &opts)
                .map(|t| Box::new(t) as Box<dyn Transcriber>)
                .map_err(|e| e.to_string())
        }
        #[cfg(feature = "whisper")]
        "whisper" => {
            use pnex_asr::whisper::{WhisperOptions, WhisperTranscriber};
            let opts = WhisperOptions {
                use_gpu: a.gpu,
                gpu_device: a.gpu_device,
                num_threads: a.threads,
                ..WhisperOptions::default()
            };
            WhisperTranscriber::load(Path::new(rest), &opts)
                .map(|t| Box::new(t) as Box<dyn Transcriber>)
                .map_err(|e| e.to_string())
        }
        r => Err(format!(
            "runtime {r} not compiled in (features sherpa / whisper)"
        )),
    }
}

struct Utterance {
    id: String,
    wav: PathBuf,
    raw: String,
    reference: String,
}

fn fleurs(dir: &Path, limit: usize) -> Vec<Utterance> {
    let tsv = std::fs::read_to_string(dir.join("test.tsv")).expect("test.tsv");
    let mut seen = std::collections::HashSet::new();
    tsv.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 4 {
                return None;
            }
            let wav = dir.join("test").join(cols[1]);
            // FLEURS repeats a sentence per speaker; keep one reading each.
            (wav.is_file() && seen.insert(cols[0].to_string())).then(|| Utterance {
                id: cols[0].to_string(),
                wav,
                raw: cols[2].to_string(),
                reference: cols[3].to_string(),
            })
        })
        .take(limit)
        .collect()
}

fn main() {
    let a = parse_args();
    let utterances = a
        .fleurs
        .as_deref()
        .map(|d| fleurs(d, a.limit))
        .unwrap_or_default();
    let long = a.wav.as_deref().map(|p| read_wav(p).expect("--wav"));
    if let Some(out) = &a.out {
        std::fs::create_dir_all(out).expect("--out");
    }
    println!(
        "| model | load ms | audio s | compute s | RTF | streams ≈ 1/RTF | WER | proper nouns |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for spec in &a.models {
        let started = Instant::now();
        let mut model = match load(spec, &a) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{spec}: {e}");
                continue;
            }
        };
        let load_ms = started.elapsed().as_millis();
        let mut out = a.out.as_ref().map(|dir| {
            let file = model.name().replace([':', '/'], "_") + ".tsv";
            std::fs::File::create(dir.join(file)).expect("hypothesis file")
        });
        let (mut audio_s, mut compute_s) = (0.0f64, 0.0f64);
        let mut wer = WerAccumulator::default();
        let (mut pn_total, mut pn_found) = (0usize, 0usize);
        for u in &utterances {
            let samples = read_wav(&u.wav).expect("utterance wav");
            let t = Instant::now();
            let hyp = model.transcribe(&samples).expect("transcribe");
            compute_s += t.elapsed().as_secs_f64();
            audio_s += samples.len() as f64 / f64::from(SAMPLE_RATE);
            wer.add(&u.reference, &hyp.text);
            let hyp_words = normalize(&hyp.text);
            for noun in proper_nouns(&u.raw) {
                pn_total += 1;
                pn_found += usize::from(hyp_words.contains(&noun));
            }
            if let Some(f) = out.as_mut() {
                writeln!(f, "{}\t{}\t{}", u.id, u.reference, hyp.text).ok();
            }
        }
        if let Some(samples) = &long {
            let chunk = a.chunk_s * SAMPLE_RATE as usize;
            for (i, part) in samples.chunks(chunk).enumerate() {
                let t = Instant::now();
                let hyp = model.transcribe(part).expect("transcribe chunk");
                compute_s += t.elapsed().as_secs_f64();
                audio_s += part.len() as f64 / f64::from(SAMPLE_RATE);
                if let Some(f) = out.as_mut() {
                    writeln!(f, "chunk-{i}\t\t{}", hyp.text).ok();
                }
            }
        }
        let rtf = if audio_s > 0.0 {
            compute_s / audio_s
        } else {
            0.0
        };
        let wer_cell = if wer.ref_words > 0 {
            format!("{:.1} %", wer.wer() * 100.0)
        } else {
            "—".into()
        };
        let pn_cell = if pn_total > 0 {
            format!(
                "{:.1} % ({pn_found}/{pn_total})",
                pn_found as f64 * 100.0 / pn_total as f64
            )
        } else {
            "—".into()
        };
        println!(
            "| {} | {load_ms} | {audio_s:.0} | {compute_s:.1} | {rtf:.3} | {:.0} | {wer_cell} | {pn_cell} |",
            model.name(),
            if rtf > 0.0 { 1.0 / rtf } else { 0.0 },
        );
    }
}
