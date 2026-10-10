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
    /// Silero voice activity detector (task `vad`).
    #[serde(rename = "silero")]
    SileroVad,
    /// pyannote segmentation (task `diarization_segmentation`).
    PyannoteSegmentation,
    /// Speaker embedding extractor: 3D-Speaker, WeSpeaker, NeMo exports
    /// (task `speaker_embedding`).
    SpeakerEmbedding,
}

/// `ml_models.task` values of the audio registry (D167).
pub const TASK_ASR: &str = "asr";
pub const TASK_VAD: &str = "vad";
pub const TASK_SEGMENTATION: &str = "diarization_segmentation";
pub const TASK_EMBEDDING: &str = "speaker_embedding";
pub const AUDIO_TASKS: [&str; 4] = [TASK_ASR, TASK_VAD, TASK_SEGMENTATION, TASK_EMBEDDING];

impl Family {
    pub const ALL: [Family; 7] = [
        Self::ParakeetTdt,
        Self::Canary,
        Self::Whisper,
        Self::WhisperGgml,
        Self::SileroVad,
        Self::PyannoteSegmentation,
        Self::SpeakerEmbedding,
    ];

    pub fn wire(self) -> &'static str {
        match self {
            Self::ParakeetTdt => "parakeet_tdt",
            Self::Canary => "canary",
            Self::Whisper => "whisper",
            Self::WhisperGgml => "whisper_ggml",
            Self::SileroVad => "silero",
            Self::PyannoteSegmentation => "pyannote_segmentation",
            Self::SpeakerEmbedding => "speaker_embedding",
        }
    }

    /// Registry task of the family (`ml_models.task`).
    pub fn task(self) -> &'static str {
        match self {
            Self::SileroVad => TASK_VAD,
            Self::PyannoteSegmentation => TASK_SEGMENTATION,
            Self::SpeakerEmbedding => TASK_EMBEDDING,
            _ => TASK_ASR,
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

/// Bytes of an ONNX file searched for metadata: `metadata_props` is the
/// last field of `ModelProto`, after the weights.
const ONNX_META_TAIL: usize = 64 * 1024;

/// Value of an ONNX `metadata_props` entry, read from the raw protobuf
/// (`StringStringEntryProto`: `0x0a len key 0x12 len value`). sherpa-onnx
/// exports state what they are there (`model_type`, `framework`).
pub fn onnx_meta(bytes: &[u8], key: &str) -> Option<String> {
    let tail = &bytes[bytes.len().saturating_sub(ONNX_META_TAIL)..];
    let mut needle = vec![0x0a, u8::try_from(key.len()).ok()?];
    needle.extend_from_slice(key.as_bytes());
    needle.push(0x12);
    let at = tail.windows(needle.len()).rposition(|w| w == needle)? + needle.len();
    // Values are short: a one-byte length (< 128) is all that is accepted.
    let len = usize::from(*tail.get(at)?);
    if len >= 0x80 {
        return None;
    }
    let value = tail.get(at + 1..at + 1 + len)?;
    std::str::from_utf8(value).ok().map(str::to_string)
}

/// Family of a non-ASR ONNX file, from its metadata.
fn onnx_family(path: &Path) -> Option<Family> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.first() != Some(&0x08) {
        return None;
    }
    match onnx_meta(&bytes, "model_type").as_deref() {
        Some("silero-vad") => return Some(Family::SileroVad),
        Some(t) if t.starts_with("pyannote-segmentation") => {
            return Some(Family::PyannoteSegmentation)
        }
        _ => {}
    }
    onnx_meta(&bytes, "framework").map(|_| Family::SpeakerEmbedding)
}

/// Detects the family of a model directory from its file names (the GGML
/// magic, the ONNX metadata of single-file models). Nothing is loaded.
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
        // Single-file models (VAD, segmentation, embedding), int8 first.
        let mut onnx: Vec<&String> = files.iter().filter(|f| f.ends_with(".onnx")).collect();
        onnx.sort_by_key(|f| !f.contains("int8"));
        for f in onnx {
            if let Some(family) = onnx_family(&dir.join(f)) {
                return Ok(Inspection {
                    family,
                    files: vec![f.clone()],
                });
            }
        }
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
    /// Stream-local speaker label (`S1`…), set by the server after the
    /// cross-segment linking; never an identity (§2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spk: Option<String>,
}

/// A speaker turn of the segment: `spk` is the index of a speaker LOCAL to
/// this segment (0, 1…), `s`/`e` in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTurn {
    pub spk: u32,
    pub s: u32,
    pub e: u32,
}

/// Ephemeral voice embedding of a local speaker, used by the server to
/// link speakers from segment to segment (D166). It lives in memory only:
/// never stored, never logged, never attached to an identity (§2).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct WireVoice {
    pub spk: u32,
    pub v: Vec<f32>,
}

impl std::fmt::Debug for WireVoice {
    // The vector stays out of any log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WireVoice {{ spk: {}, dim: {} }}",
            self.spk,
            self.v.len()
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WireWord>,
    /// Speech detected by the VAD, ms; present when the process runs one.
    /// `Some(0)` = silent segment: no transcription was attempted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech_ms: Option<u32>,
    /// Speaker turns, when the process runs a diarizer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<WireTurn>,
    /// One embedding per local speaker of `turns` (or of the whole clip
    /// for an embedding model alone).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub voices: Vec<WireVoice>,
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

    /// A minimal ONNX-looking file: `ir_version`, then one metadata entry.
    pub(crate) fn onnx_with_meta(key: &str, value: &str) -> Vec<u8> {
        let mut entry = vec![0x0a, key.len() as u8];
        entry.extend_from_slice(key.as_bytes());
        entry.extend_from_slice(&[0x12, value.len() as u8]);
        entry.extend_from_slice(value.as_bytes());
        let mut out = vec![0x08, 0x07, 0x3a, 0x02, 0xff, 0xff, 0x72, entry.len() as u8];
        out.extend(entry);
        out
    }

    #[test]
    fn single_onnx_models_are_told_apart_by_metadata() {
        let cases = [
            ("model_type", "silero-vad", Family::SileroVad, "vad"),
            (
                "model_type",
                "pyannote-segmentation-3.0",
                Family::PyannoteSegmentation,
                "diarization_segmentation",
            ),
            (
                "framework",
                "3d-speaker",
                Family::SpeakerEmbedding,
                "speaker_embedding",
            ),
        ];
        for (k, v, family, task) in cases {
            let d = dir_with(&[]);
            std::fs::write(d.join("model.onnx"), onnx_with_meta(k, v)).unwrap();
            std::fs::write(d.join("model.int8.onnx"), onnx_with_meta(k, v)).unwrap();
            let i = inspect(&d).unwrap();
            assert_eq!(i.family, family);
            assert_eq!(i.family.task(), task);
            assert_eq!(i.files, ["model.int8.onnx"], "int8 first");
            assert_eq!(Family::from_wire(family.wire()), Some(family));
            let _ = std::fs::remove_dir_all(d);
        }
        let d = dir_with(&[]);
        std::fs::write(d.join("x.onnx"), onnx_with_meta("model_type", "other")).unwrap();
        assert!(inspect(&d).is_err(), "unknown metadata refused");
        let _ = std::fs::remove_dir_all(d);
        assert_eq!(
            onnx_meta(&onnx_with_meta("framework", "wespeaker"), "framework").as_deref(),
            Some("wespeaker")
        );
        assert_eq!(onnx_meta(b"\x08\x07", "framework"), None);
    }

    #[test]
    fn voices_never_print_their_vector() {
        let v = WireVoice {
            spk: 1,
            v: vec![0.123_456, 0.5],
        };
        assert_eq!(format!("{v:?}"), "WireVoice { spk: 1, dim: 2 }");
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
