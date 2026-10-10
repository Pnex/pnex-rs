//! Process protocol between the server and `pnex-asr serve` (media-ingest.md
//! D166–D167). The runtime lives in its own process: a native crash never
//! takes the server down, the model stays loaded between segments, and the
//! server build links no native library.
//!
//! JSON lines on stdin/stdout. At start the process prints one [`Ready`]
//! line (or a [`Fatal`] line, then exits); then one [`Response`] per
//! [`Request`], in order.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Model family = one adapter (D167). Wire names match `ml_models.family`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// NeMo transducer through sherpa-onnx (Parakeet TDT).
    ParakeetTdt,
    /// NeMo Canary through sherpa-onnx.
    Canary,
    /// Whisper ONNX export through sherpa-onnx.
    Whisper,
    /// Whisper GGML/GGUF through whisper.cpp.
    WhisperGgml,
}

impl Family {
    pub const ALL: [Family; 4] = [
        Self::ParakeetTdt,
        Self::Canary,
        Self::Whisper,
        Self::WhisperGgml,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::ParakeetTdt => "parakeet_tdt",
            Self::Canary => "canary",
            Self::Whisper => "whisper",
            Self::WhisperGgml => "whisper_ggml",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.wire() == s)
    }

    /// Runtime that serves the family.
    pub fn runtime(self) -> &'static str {
        match self {
            Self::WhisperGgml => "whisper.cpp",
            _ => "sherpa-onnx",
        }
    }
}

/// What a model directory holds, read from the files only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inspection {
    pub family: Family,
    /// Files the adapter will load, relative to the directory.
    pub files: Vec<String>,
}

fn names(dir: &Path) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    out.sort();
    Ok(out)
}

fn is_ggml(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && (&magic == b"lmgg" || &magic == b"GGUF")
}

/// Detects the family of a model directory from its file names (and the
/// GGML magic). Nothing is loaded.
pub fn inspect(dir: &Path) -> Result<Inspection, String> {
    let files = names(dir)?;
    let has = |pred: &dyn Fn(&str) -> bool| files.iter().any(|f| pred(f));
    let pick = |stem: &str, ext: &str| -> Option<String> {
        let mut c: Vec<&String> = files
            .iter()
            .filter(|f| f.contains(stem) && f.ends_with(ext))
            .collect();
        c.sort_by_key(|f| !f.contains("int8"));
        c.first().map(|s| s.to_string())
    };
    if let Some(bin) = files
        .iter()
        .find(|f| (f.ends_with(".bin") || f.ends_with(".gguf")) && is_ggml(&dir.join(f)))
    {
        return Ok(Inspection {
            family: Family::WhisperGgml,
            files: vec![bin.clone()],
        });
    }
    let (Some(enc), Some(dec), Some(tok)) = (
        pick("encoder", ".onnx"),
        pick("decoder", ".onnx"),
        pick("tokens", ".txt"),
    ) else {
        return Err("no encoder/decoder/tokens: not a supported model".into());
    };
    let family = if has(&|f| f.contains("joiner") && f.ends_with(".onnx")) {
        Family::ParakeetTdt
    } else if tok != "tokens.txt" && tok.ends_with("-tokens.txt") {
        // Whisper exports prefix every file with the size (`small-`).
        Family::Whisper
    } else {
        Family::Canary
    };
    let mut used = vec![enc, dec, tok];
    if family == Family::ParakeetTdt {
        used.extend(pick("joiner", ".onnx"));
    }
    Ok(Inspection {
        family,
        files: used,
    })
}

/// One segment to transcribe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    /// 16 kHz mono WAV, base64.
    pub wav_b64: String,
}

/// A recognized word: `s`/`e` in ms from the segment start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireWord {
    pub w: String,
    pub s: u32,
    pub e: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WireWord>,
    /// Inference time.
    #[serde(default)]
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// First line once the model is loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ready {
    pub ready: String,
    pub load_ms: u64,
}

/// First line when the model cannot be served (the process then exits).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fatal {
    pub fatal: String,
}

/// Reference sample of the model check (FLEURS fr, CC BY 4.0, see
/// `assets/README.md`) and its transcript.
pub const CHECK_WAV_FR: &[u8] = include_bytes!("../assets/check-fr.wav");
pub const CHECK_TEXT_FR: &str = include_str!("../assets/check-fr.txt");

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[&str]) -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pnex-asr-inspect-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in files {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        d
    }

    #[test]
    fn families_are_told_apart_by_file_names() {
        let p = dir_with(&[
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ]);
        let i = inspect(&p).unwrap();
        assert_eq!(i.family, Family::ParakeetTdt);
        assert_eq!(i.files.len(), 4);
        let c = dir_with(&["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"]);
        assert_eq!(inspect(&c).unwrap().family, Family::Canary);
        let w = dir_with(&[
            "small-encoder.onnx",
            "small-encoder.int8.onnx",
            "small-decoder.int8.onnx",
            "small-tokens.txt",
        ]);
        let i = inspect(&w).unwrap();
        assert_eq!(i.family, Family::Whisper);
        assert!(i.files.contains(&"small-encoder.int8.onnx".to_string()));
        let bad = dir_with(&["model.onnx", "readme.md"]);
        assert!(inspect(&bad).is_err());
        for d in [p, c, w, bad] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn ggml_is_detected_by_magic() {
        let d = dir_with(&[]);
        std::fs::write(d.join("ggml-small.bin"), b"lmgg\x00\x01").unwrap();
        assert_eq!(inspect(&d).unwrap().family, Family::WhisperGgml);
        std::fs::write(d.join("ggml-small.bin"), b"notggml").unwrap();
        assert!(inspect(&d).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn check_sample_is_a_16k_wav() {
        assert_eq!(&CHECK_WAV_FR[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(CHECK_WAV_FR[24..28].try_into().unwrap()),
            16_000
        );
        assert!(CHECK_TEXT_FR.starts_with("La surface de la lune"));
    }
}
