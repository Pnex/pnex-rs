//! Media sniff (D21): strict allowlist of upload formats, pure and testable.
//!
//! A file is accepted only when its **content** is a known format (magic
//! bytes, or a structural check for the few formats without one) **and** its
//! extension is one of that format's extensions: a renamed executable, a zip
//! named `.docx` or a JPEG named `.png` are refused. The kind is then derived
//! from the format, or checked against it when the client forces one.
//!
//! - Image with **GPano** metadata (`GPano:ProjectionType=equirectangular`,
//!   Google photo sphere / Street View standard) → `panorama`.
//! - Splats, models (ONNX, GGML/GGUF, sherpa-onnx archives), documents and
//!   tables (doc-search.md) have their own kinds.
//!
//! Residual: `.splat` and `.ksplat` have no signature; they are checked by
//! extension, record size (`.splat`) and the absence of any other known
//! signature. They are served as opaque bytes (SEC-5), never inline.

use pnex_core::doc_extract::{self, DocFormat};

/// GPano scan window: XMP lives in the JPEG APP1 segment (or PNG iTXt) at
/// the head of the file.
const SNIFF_WINDOW: usize = 128 * 1024;
/// Record size of the antimatter15 `.splat` format.
const SPLAT_RECORD: usize = 32;

/// Sniff result: kind + metadata stored on the version.
pub struct Sniff {
    pub kind: &'static str,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    Webp,
    Avif,
    Heic,
    Ply,
    Splat,
    Ksplat,
    Spz,
    Onnx,
    Ggml,
    Tar,
    TarGz,
    TarBz2,
    /// Model archives only (sherpa-onnx publishes some as zip).
    ModelZip,
    Doc(DocFormat),
}

impl Format {
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Png => &["png"],
            Self::Gif => &["gif"],
            Self::Webp => &["webp"],
            Self::Avif => &["avif"],
            Self::Heic => &["heic", "heif"],
            Self::Ply => &["ply"],
            Self::Splat => &["splat"],
            Self::Ksplat => &["ksplat"],
            Self::Spz => &["spz"],
            Self::Onnx => &["onnx"],
            Self::Ggml => &["bin", "gguf", "ggml"],
            Self::Tar => &["tar"],
            Self::TarGz => &["gz", "tgz"],
            Self::TarBz2 => &["bz2", "tbz2"],
            Self::ModelZip => &["zip"],
            // `format_of` already matched the extension.
            Self::Doc(_) => &[],
        }
    }

    /// Kinds a file of this format may be stored as.
    pub fn kinds(self) -> &'static [&'static str] {
        match self {
            Self::Jpeg | Self::Png | Self::Gif | Self::Webp | Self::Avif | Self::Heic => {
                &["photo", "panorama", "floorplan"]
            }
            Self::Ply | Self::Splat | Self::Ksplat | Self::Spz => &["splat"],
            Self::Onnx | Self::Ggml | Self::Tar | Self::TarGz | Self::TarBz2 | Self::ModelZip => {
                &["model"]
            }
            Self::Doc(d) if d.kind() == doc_extract::KIND_TABLE => &["table"],
            Self::Doc(_) => &["document"],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ply => "ply",
            Self::Splat => "splat",
            Self::Ksplat => "ksplat",
            Self::Spz => "spz",
            Self::Onnx => "onnx",
            Self::Ggml => "ggml",
            _ => "",
        }
    }
}

fn extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Format from magic bytes only (`None` = no known signature).
fn by_magic(b: &[u8]) -> Option<Format> {
    let ftyp_brand = (b.len() >= 12 && &b[4..8] == b"ftyp").then(|| &b[8..12]);
    Some(match b {
        _ if b.starts_with(b"\xFF\xD8\xFF") => Format::Jpeg,
        _ if b.starts_with(b"\x89PNG\r\n\x1a\n") => Format::Png,
        _ if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") => Format::Gif,
        _ if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") => Format::Webp,
        _ if matches!(ftyp_brand, Some(b"avif" | b"avis")) => Format::Avif,
        _ if matches!(ftyp_brand, Some(b"heic" | b"heix" | b"mif1" | b"msf1")) => Format::Heic,
        _ if b.starts_with(b"ply\n") || b.starts_with(b"ply\r\n") => Format::Ply,
        _ if b.starts_with(b"lmgg") || b.starts_with(b"GGUF") => Format::Ggml,
        _ if b.get(257..262) == Some(b"ustar") => Format::Tar,
        _ if b.starts_with(b"\x1F\x8B") => Format::TarGz,
        _ if b.starts_with(b"BZh") => Format::TarBz2,
        _ if b.starts_with(b"PK\x03\x04") => Format::ModelZip,
        _ => return None,
    })
}

/// Identifies an allowed format whose content and extension agree.
pub fn identify(filename: &str, bytes: &[u8]) -> Option<Format> {
    if let Some(doc) = doc_extract::format_of(filename, bytes) {
        return Some(Format::Doc(doc));
    }
    let ext = extension(filename);
    let format = match by_magic(bytes) {
        // gzip is also the container of `.spz` splats.
        Some(Format::TarGz) if ext == "spz" => Format::Spz,
        Some(f) => f,
        // Formats without a signature.
        None => match ext.as_str() {
            // An ONNX ModelProto opens with field 1 (ir_version, varint).
            "onnx" if bytes.first() == Some(&0x08) => Format::Onnx,
            "splat" if !bytes.is_empty() && bytes.len() % SPLAT_RECORD == 0 => Format::Splat,
            "ksplat" if !bytes.is_empty() => Format::Ksplat,
            _ => return None,
        },
    };
    format
        .extensions()
        .contains(&ext.as_str())
        .then_some(format)
}

/// Detects the kind and metadata of an upload; `None` = refused format.
pub fn detect(filename: &str, bytes: &[u8]) -> Option<Sniff> {
    let format = identify(filename, bytes)?;
    let window = &bytes[..bytes.len().min(SNIFF_WINDOW)];
    let image = format.kinds().contains(&"photo");
    if image && contains_ci(window, b"GPano:") && contains_ci(window, b"equirectangular") {
        return Some(Sniff {
            kind: "panorama",
            metadata: Some(serde_json::json!({
                "projection": "equirectangular",
                "source": "gpano"
            })),
        });
    }
    let label = format.label();
    Some(Sniff {
        kind: format.kinds()[0],
        metadata: (!label.is_empty()).then(|| serde_json::json!({ "format": label })),
    })
}

/// Whether `bytes` may be stored under `kind` (forced kind, new version).
pub fn accepts(kind: &str, filename: &str, bytes: &[u8]) -> bool {
    identify(filename, bytes).is_some_and(|f| f.kinds().contains(&kind))
}

/// Case-insensitive search of an ASCII needle in the window.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xD9];

    fn kind(filename: &str, bytes: &[u8]) -> Option<&'static str> {
        detect(filename, bytes).map(|s| s.kind)
    }

    #[test]
    fn models_by_magic_or_structure() {
        assert_eq!(kind("ggml-small.bin", b"lmgg\x01\x02"), Some("model"));
        assert_eq!(kind("x.gguf", b"GGUF\x03"), Some("model"));
        assert_eq!(kind("m.onnx", b"\x08\x07rest"), Some("model"));
        assert_eq!(kind("m.onnx", b"not a real onnx"), None);
        let mut tar = vec![0u8; 512];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(kind("model.tar", &tar), Some("model"));
        assert_eq!(kind("model.tar.bz2", b"BZh91AY"), Some("model"));
        assert_eq!(kind("model.zip", b"PK\x03\x04"), Some("model"));
    }

    #[test]
    fn fake_extensions_are_refused() {
        // Unknown bytes, whatever the name.
        assert!(detect("x.bin", b"\x00\x01").is_none());
        assert!(detect("run.exe", b"MZ\x90\x00").is_none());
        assert!(detect("photo.jpg", b"MZ\x90\x00").is_none());
        // Known bytes under another format's extension.
        assert!(detect("plan.png", JPEG).is_none());
        assert!(detect("plan.html", JPEG).is_none());
        assert!(detect("doc.pdf", JPEG).is_none());
        assert!(detect("m.onnx", JPEG).is_none());
        // A plain zip is only a model archive, never a document.
        assert!(detect("a.docx", b"PK\x03\x04photos/a.jpg").is_none());
        // Text names over binary content.
        assert!(detect("notes.txt", b"\x00\x01\x02").is_none());
    }

    #[test]
    fn forced_kind_must_match_the_format() {
        assert!(accepts("floorplan", "p.jpg", JPEG));
        assert!(!accepts("model", "p.jpg", JPEG));
        assert!(!accepts("photo", "notes.md", b"# hi"));
        assert!(accepts("document", "notes.md", b"# hi"));
        assert!(accepts("table", "m.csv", b"a,b\n1,2"));
        assert!(!accepts("document", "m.csv", b"a,b\n1,2"));
    }

    /// Minimal JPEG with an APP1 XMP segment holding `xmp`.
    fn jpeg_with_xmp(xmp: &str) -> Vec<u8> {
        let prefix = b"http://ns.adobe.com/xap/1.0/\x00";
        let content = [prefix.as_slice(), xmp.as_bytes()].concat();
        let len = (content.len() + 2) as u16;
        let mut out = vec![0xFF, 0xD8];
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&content);
        out.extend_from_slice(&[0xFF, 0xD9]);
        out
    }

    #[test]
    fn jpeg_gpano_devient_panorama() {
        let xmp = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description GPano:ProjectionType="equirectangular"/>
            </rdf:RDF></x:xmpmeta>"#;
        let bytes = jpeg_with_xmp(xmp);
        let sniff = detect("sphere.jpg", &bytes).unwrap();
        assert_eq!(sniff.kind, "panorama");
        let meta = sniff.metadata.unwrap();
        assert_eq!(meta["projection"], "equirectangular");
        assert_eq!(meta["source"], "gpano");
    }

    #[test]
    fn jpeg_sans_gpano_reste_photo() {
        let bytes = jpeg_with_xmp("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>");
        let sniff = detect("photo.jpg", &bytes).unwrap();
        assert_eq!(sniff.kind, "photo");
        assert!(sniff.metadata.is_none());
    }

    #[test]
    fn splats_by_signature_or_structure() {
        assert_eq!(kind("s.ply", b"ply\nformat binary"), Some("splat"));
        assert_eq!(kind("s.splat", &[0u8; 64]), Some("splat"));
        assert_eq!(kind("s.splat", &[0u8; 33]), None);
        assert_eq!(kind("s.spz", b"\x1F\x8B\x08"), Some("splat"));
        assert_eq!(kind("s.ksplat", &[1u8; 16]), Some("splat"));
        // A JPEG renamed .ksplat carries another signature.
        assert_eq!(kind("s.ksplat", JPEG), None);
        let meta = detect("s.ply", b"ply\n").unwrap().metadata.unwrap();
        assert_eq!(meta["format"], "ply");
    }

    #[test]
    fn images_by_signature() {
        assert_eq!(kind("img.png", b"\x89PNG\r\n\x1a\n...."), Some("photo"));
        assert_eq!(kind("img.webp", b"RIFF\0\0\0\0WEBPVP8 "), Some("photo"));
        assert_eq!(kind("img.heic", b"\0\0\0\x18ftypheic"), Some("photo"));
        assert_eq!(kind("img.avif", b"\0\0\0\x18ftypavif"), Some("photo"));
        assert_eq!(kind("img.gif", b"GIF89a"), Some("photo"));
    }

    #[test]
    fn gpano_insensible_casse() {
        let xmp = r#"<rdf:Description gpano:projectiontype="EQUIRECTANGULAR"/>"#;
        let bytes = jpeg_with_xmp(xmp);
        assert_eq!(kind("p.jpg", &bytes), Some("panorama"));
    }
}
