//! Audio model archives → model directory (media-ingest.md D167).
//!
//! An uploaded model is a single GGML/GGUF file, a single ONNX file that
//! states what it is in its metadata (Silero VAD, pyannote segmentation,
//! speaker embedding exports of sherpa-onnx), or an archive (`.tar`,
//! `.tar.bz2`, `.tar.gz`, `.zip`, as published by sherpa-onnx). The archive
//! is untrusted: only regular files with a model extension are written,
//! flattened to their base name (no path from the archive is ever joined,
//! no link followed), within a total size bound.

use std::io::{Cursor, Read};
use std::path::Path;

/// Largest extracted model (Whisper large fp32 ≈ 3 GB).
pub const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Extensions kept from an archive.
const KEPT: [&str; 6] = ["onnx", "txt", "bin", "gguf", "json", "model"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArchiveError {
    #[error("not a model archive")]
    Unsupported,
    #[error("archive too large")]
    TooLarge,
    #[error("archive unreadable: {0}")]
    Corrupt(String),
    #[error("write failed: {0}")]
    Io(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ggml,
    Onnx,
    Tar,
    TarBz2,
    TarGz,
    Zip,
}

fn kind_of(bytes: &[u8]) -> Option<Kind> {
    match bytes {
        [b'l', b'm', b'g', b'g', ..] | [b'G', b'G', b'U', b'F', ..] => Some(Kind::Ggml),
        [b'B', b'Z', b'h', ..] => Some(Kind::TarBz2),
        [0x1f, 0x8b, ..] => Some(Kind::TarGz),
        [b'P', b'K', 3, 4, ..] => Some(Kind::Zip),
        _ if bytes.len() > 262 && &bytes[257..262] == b"ustar" => Some(Kind::Tar),
        // ONNX = protobuf starting with `ir_version`; only one that says
        // what it is is taken.
        [0x08, ..]
            if ["model_type", "framework"]
                .iter()
                .any(|k| pnex_asr::protocol::onnx_meta(bytes, k).is_some()) =>
        {
            Some(Kind::Onnx)
        }
        _ => None,
    }
}

/// Base name of an archive entry, `None` when it is not a kept model file.
fn kept_name(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?.trim();
    if name.is_empty() || name.starts_with('.') || name.contains("..") {
        return None;
    }
    let ext = name.rsplit('.').next()?.to_ascii_lowercase();
    KEPT.contains(&ext.as_str()).then(|| name.to_string())
}

struct Writer<'a> {
    dir: &'a Path,
    total: u64,
}

impl Writer<'_> {
    fn write(&mut self, name: &str, mut src: impl Read) -> Result<(), ArchiveError> {
        let path = self.dir.join(name);
        let mut file = std::fs::File::create(&path).map_err(|e| ArchiveError::Io(e.to_string()))?;
        let budget = MAX_EXTRACTED_BYTES.saturating_sub(self.total);
        let copied = std::io::copy(&mut (&mut src).take(budget + 1), &mut file)
            .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
        if copied > budget {
            return Err(ArchiveError::TooLarge);
        }
        self.total += copied;
        Ok(())
    }
}

fn untar(reader: impl Read, w: &mut Writer<'_>) -> Result<(), ArchiveError> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
    for entry in entries {
        let entry = entry.map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| ArchiveError::Corrupt(e.to_string()))?
            .to_string_lossy()
            .into_owned();
        if let Some(name) = kept_name(&path) {
            w.write(&name, entry)?;
        }
    }
    Ok(())
}

/// Writes the model files of `bytes` into `dir` (existing, empty).
pub fn extract(bytes: &[u8], dir: &Path) -> Result<(), ArchiveError> {
    let mut w = Writer { dir, total: 0 };
    match kind_of(bytes).ok_or(ArchiveError::Unsupported)? {
        Kind::Ggml => w.write("model.bin", bytes),
        Kind::Onnx => w.write("model.onnx", bytes),
        Kind::Tar => untar(bytes, &mut w),
        Kind::TarBz2 => untar(bzip2::read::BzDecoder::new(bytes), &mut w),
        Kind::TarGz => untar(flate2::read::GzDecoder::new(bytes), &mut w),
        Kind::Zip => {
            let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
            for i in 0..zip.len() {
                let file = zip
                    .by_index(i)
                    .map_err(|e| ArchiveError::Corrupt(e.to_string()))?;
                if !file.is_file() {
                    continue;
                }
                if let Some(name) = kept_name(file.name()) {
                    w.write(&name, file)?;
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pnex-archive-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tar_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            b.append_data(&mut h, path, *data).unwrap();
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn sherpa_layout_is_flattened_and_filtered() {
        let raw = tar_of(&[
            ("model-dir/encoder.int8.onnx", b"e"),
            ("model-dir/tokens.txt", b"t"),
            ("model-dir/test_wavs/0.wav", b"w"),
            ("model-dir/README.md", b"r"),
        ]);
        let mut bz = Vec::new();
        std::io::copy(
            &mut bzip2::read::BzEncoder::new(raw.as_slice(), bzip2::Compression::fast()),
            &mut bz,
        )
        .unwrap();
        let d = tmp();
        extract(&bz, &d).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["encoder.int8.onnx", "tokens.txt"]);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn traversal_names_never_leave_the_directory() {
        assert_eq!(
            kept_name("../../etc/cron.d/x.txt").as_deref(),
            Some("x.txt")
        );
        assert_eq!(kept_name("a/..onnx"), None);
        assert_eq!(kept_name("a/.hidden.onnx"), None);
        assert_eq!(kept_name("dir/"), None);
        assert_eq!(kept_name("x.sh"), None);
    }

    #[test]
    fn ggml_is_stored_as_is_and_garbage_refused() {
        let d = tmp();
        extract(b"lmgg\x00\x01\x02", &d).unwrap();
        assert!(d.join("model.bin").exists());
        assert_eq!(extract(b"<html>", &d), Err(ArchiveError::Unsupported));
        assert_eq!(
            extract(b"\x08\x07 no metadata", &d),
            Err(ArchiveError::Unsupported)
        );
        let mut onnx = vec![0x08, 0x07, 0x72, 0x16, 0x0a, 0x0a];
        onnx.extend_from_slice(b"model_type\x12\x0asilero-vad");
        extract(&onnx, &d).unwrap();
        assert!(d.join("model.onnx").exists());
        let _ = std::fs::remove_dir_all(d);
    }
}
