//! Sniff média (D21) — détection du `kind` à l'upload, pur et testable.
//! Priorité résolue dans le contrôleur : kind explicite du client > sniff > `photo`.
//!
//! - Image avec métadonnées **GPano** : `GPano:ProjectionType=equirectangular`
//!   (standard Google photo sphere / Street View) → `panorama` — même
//!   détection que Google Street View, zéro stitching pour la V1 (étude
//!   capture panorama mobile dans `docs/architecture/media.md`).
//! - Extensions `.ply/.splat/.ksplat/.spz` → `splat`.
//! - Tout le reste → `photo` ; cubemaps/tuiles non reconnues restent
//!   stockables (kind forcé par le client).

/// Plafond du scan GPano : le XMP vit dans le segment APP1 en tête de
/// fichier JPEG (ou iTXt en tête de PNG) — 128 Ko suffisent largement.
const SNIFF_WINDOW: usize = 128 * 1024;

/// Résultat du sniff : kind + métadonnées à stocker sur la version.
pub struct Sniff {
    pub kind: &'static str,
    pub metadata: Option<serde_json::Value>,
}

/// Détecte le kind d'un média depuis son contenu et son nom de fichier.
pub fn detect(content_type: &str, filename: &str, bytes: &[u8]) -> Sniff {
    let _ = content_type;
    let window = &bytes[..bytes.len().min(SNIFF_WINDOW)];
    // GPano : présence conjointe des deux marqueurs, insensible à la casse
    // (XMP peut encoder `equirectangular` en attribut XMP normal).
    let has_gpano_ns = contains_ci(window, b"GPano:");
    let has_equirect = contains_ci(window, b"equirectangular");
    if has_gpano_ns && has_equirect {
        return Sniff {
            kind: "panorama",
            metadata: Some(serde_json::json!({
                "projection": "equirectangular",
                "source": "gpano"
            })),
        };
    }
    let ext = filename
        .rsplit('.')
        .next()
        .map_or_else(String::new, str::to_ascii_lowercase);
    match ext.as_str() {
        "ply" | "splat" | "ksplat" | "spz" => Sniff {
            kind: "splat",
            metadata: Some(serde_json::json!({ "format": ext })),
        },
        // ONNX models (vision registry, camera-video.md D81).
        "onnx" => Sniff {
            kind: "model",
            metadata: Some(serde_json::json!({ "format": "onnx" })),
        },
        _ => Sniff {
            kind: "photo",
            metadata: None,
        },
    }
}

/// Recherche insensible à la casse d'une aiguille ASCII dans la fenêtre.
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

    /// JPEG minimal avec un segment APP1 XMP contenant `xmp`.
    /// SOI (FFD8) + APP1 (FFE1 + len + préfixe XMP standard) + EOI.
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
        let sniff = detect("image/jpeg", "sphere.jpg", &bytes);
        assert_eq!(sniff.kind, "panorama");
        let meta = sniff.metadata.unwrap();
        assert_eq!(meta["projection"], "equirectangular");
        assert_eq!(meta["source"], "gpano");
    }

    #[test]
    fn jpeg_sans_gpano_reste_photo() {
        let bytes = jpeg_with_xmp("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>");
        let sniff = detect("image/jpeg", "photo.jpg", &bytes);
        assert_eq!(sniff.kind, "photo");
        assert!(sniff.metadata.is_none());
    }

    #[test]
    fn splats_par_extension() {
        for (ext, format) in [
            ("ply", "ply"),
            ("splat", "splat"),
            ("ksplat", "ksplat"),
            ("spz", "spz"),
        ] {
            let sniff = detect("application/octet-stream", &format!("scene.{ext}"), b"");
            assert_eq!(sniff.kind, "splat");
            assert_eq!(sniff.metadata.unwrap()["format"], format);
        }
    }

    #[test]
    fn photo_et_inconnu_tombent_sur_photo() {
        assert_eq!(detect("image/png", "img.png", b"\x89PNG").kind, "photo");
        assert_eq!(
            detect("application/octet-stream", "archive.zip", b"PK").kind,
            "photo"
        );
    }

    #[test]
    fn gpano_insensible_casse() {
        let xmp = r#"<rdf:Description gpano:projectiontype="EQUIRECTANGULAR"/>"#;
        let bytes = jpeg_with_xmp(xmp);
        assert_eq!(detect("image/jpeg", "p.jpg", &bytes).kind, "panorama");
    }
}
