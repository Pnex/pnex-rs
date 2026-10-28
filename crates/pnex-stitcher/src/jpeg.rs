//! JPEG codec — RGB8 decoding and fixed-quality encoding, via the
//! `image` crate (jpeg feature only: no superfluous decoders embedded).

use image::codecs::jpeg::JpegEncoder;
use thiserror::Error;

/// Codec and stitching errors (see [`crate::stitch`]).
#[derive(Debug, Error)]
pub enum JpegError {
    /// Unreadable data or inconsistent dimensions.
    #[error("unreadable JPEG: {0}")]
    Decode(#[from] image::ImageError),
    /// RGB buffer incompatible with the announced dimensions.
    #[error("incompatible RGB buffer: {got} bytes, expected {expected}")]
    Buffer {
        /// Received size.
        got: usize,
        /// Expected size (w·h·3).
        expected: usize,
    },
    /// Encoding failure.
    #[error("JPEG encoding: {0}")]
    Encode(String),
}

/// Decodes a JPEG into RGB8.
///
/// # Errors
/// [`JpegError::Decode`] if the bytes are not a decodable image.
pub fn decode_jpeg_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), JpegError> {
    let img = image::load_from_memory(bytes)?;
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    Ok((rgb.into_raw(), w, h))
}

/// Decodes a JPEG into RGB8, capping the width at `max_width`
/// (bilinear downscale beyond — the FOV is unchanged, only the pixel
/// density drops). For the multi-ring device preview: 45 frames
/// 1080×1920 decoded at full resolution ≈ 280 MB ×2 through the stitcher's
/// clone → swap-thrash (same trap as the 2048 finding of 2026-09-09).
/// At 540 wide, 45 frames ≈ 70 MB ×2: bounded.
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

/// Encodes an RGB8 buffer to JPEG at the requested quality (0-100).
///
/// # Errors
/// [`JpegError::Buffer`] if `rgb.len() != w·h·3`, [`JpegError::Encode`]
/// on encoding failure.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, JpegError> {
    let expected = width as usize * height as usize * 3;
    if rgb.len() != expected {
        return Err(JpegError::Buffer {
            got: rgb.len(),
            expected,
        });
    }
    let img = image::RgbImage::from_raw(width, height, rgb.to_vec())
        .ok_or_else(|| JpegError::Encode("inconsistent dimensions".into()))?;
    let mut out = Vec::new();
    img.write_with_encoder(JpegEncoder::new_with_quality(&mut out, quality))
        .map_err(|e| JpegError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode → decode cycle at exact dimensions.
    #[test]
    fn round_trip() {
        let (w, h) = (8u32, 4u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let jpg = encode_jpeg(&rgb, w, h, 90).unwrap();
        let (rgb2, w2, h2) = decode_jpeg_rgb(&jpg).unwrap();
        assert_eq!((w2, h2), (w, h));
        assert_eq!(rgb2.len(), rgb.len());
    }

    /// Buffer too short → clear error (no panic).
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
