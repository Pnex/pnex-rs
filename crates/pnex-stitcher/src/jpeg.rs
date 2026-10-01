//! Codec JPEG — décodage RGB8 et encodage qualité fixée, via la crate
//! `image` (feature jpeg seule : pas de décodeurs superflus embarqués).

use image::codecs::jpeg::JpegEncoder;
use thiserror::Error;

/// Erreurs du codec et du stitching (cf. [`crate::stitch`]).
#[derive(Debug, Error)]
pub enum JpegError {
    /// Données illisibles ou dimensions incohérentes.
    #[error("JPEG illisible : {0}")]
    Decode(#[from] image::ImageError),
    /// Buffer RGB incompatible avec les dimensions annoncées.
    #[error("buffer RGB incompatible : {got} octets, attendu {expected}")]
    Buffer {
        /// Taille reçue.
        got: usize,
        /// Taille attendue (w·h·3).
        expected: usize,
    },
    /// Échec d'encodage.
    #[error("encodage JPEG : {0}")]
    Encode(String),
}

/// Décode un JPEG en RGB8.
///
/// # Erreurs
/// [`JpegError::Decode`] si les octets ne sont pas un image décodable.
pub fn decode_jpeg_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), JpegError> {
    let img = image::load_from_memory(bytes)?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    Ok((rgb.into_raw(), w, h))
}

/// Décode un JPEG en RGB8, en plafonnant la largeur à `max_width`
/// (downscale bilinéaire au-delà — la FOV est inchangée, seule la densité
/// de pixels baisse). Pour l'aperçu device multi-anneaux : 45 frames
/// 1080×1920 décodées pleine résolution ≈ 280 Mo ×2 par le clone du
/// stitcher → swap-thrash (même piège que le constat 2048 du 2026-09-09).
/// À 540 de large, 45 frames ≈ 70 Mo ×2 : borné.
#[must_use]
pub fn decode_jpeg_rgb_capped(bytes: &[u8], max_width: u32) -> (Vec<u8>, u32, u32) {
    let img = match image::load_from_memory(bytes) {
        Ok(img) => img,
        Err(_) => return (Vec::new(), 0, 0),
    };
    let rgb_img = img.to_rgb8();
    let (w, h) = rgb_img.dimensions();
    if w <= max_width {
        return (rgb_img.into_raw(), w, h);
    }
    let nh = ((h as u64 * max_width as u64) / w as u64) as u32;
    let resized = image::imageops::resize(
        &rgb_img,
        max_width,
        nh.max(1),
        image::imageops::FilterType::Triangle,
    );
    let (w, h) = resized.dimensions();
    (resized.into_raw(), w, h)
}

/// Encode un buffer RGB8 en JPEG à la qualité demandée (0-100).
///
/// # Erreurs
/// [`JpegError::Buffer`] si `rgb.len() != w·h·3`, [`JpegError::Encode`]
/// en cas d'échec d'encodage.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, JpegError> {
    let expected = width as usize * height as usize * 3;
    if rgb.len() != expected {
        return Err(JpegError::Buffer {
            got: rgb.len(),
            expected,
        });
    }
    let img = image::RgbImage::from_raw(width, height, rgb.to_vec())
        .ok_or_else(|| JpegError::Encode("dimensions incohérentes".into()))?;
    let mut out = Vec::new();
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))
        .map_err(|e| JpegError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cycle encode → decode aux dimensions exactes.
    #[test]
    fn aller_retour() {
        let (w, h) = (8u32, 4u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let jpg = encode_jpeg(&rgb, w, h, 90).unwrap();
        let (rgb2, w2, h2) = decode_jpeg_rgb(&jpg).unwrap();
        assert_eq!((w2, h2), (w, h));
        assert_eq!(rgb2.len(), rgb.len());
    }

    /// Buffer trop court → erreur claire (pas de panic).
    #[test]
    fn buffer_incoherent() {
        assert!(matches!(
            encode_jpeg(&[0u8; 10], 4, 4, 80),
            Err(JpegError::Buffer {
                got: 10,
                expected: 48
            })
        ));
    }
}
