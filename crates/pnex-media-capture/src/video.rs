//! Video track of a stream (media-ingest.md D175): the confined video
//! decoder writes concatenated JPEGs on its stdout; [`JpegSplitter`] cuts
//! them. Publishing them is the carrier's job (server: camera bus).

/// Largest JPEG accepted from the decoder (a 1280 px wide frame at q 5 is
/// far below); beyond, the decoder output is treated as broken.
pub const MAX_JPEG_BYTES: usize = 4 * 1024 * 1024;

/// Cuts a byte stream of concatenated baseline JPEGs into frames.
#[derive(Default)]
pub struct JpegSplitter {
    buf: Vec<u8>,
}

/// One frame and its dimensions (from the SOF segment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jpeg {
    pub bytes: Vec<u8>,
    pub width: u16,
    pub height: u16,
}

impl JpegSplitter {
    /// Appends `chunk`, returns the complete frames. `Err` = not a JPEG
    /// stream, or a frame over [`MAX_JPEG_BYTES`].
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Jpeg>, ()> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        loop {
            match scan(&self.buf)? {
                Some((len, width, height)) => {
                    let rest = self.buf.split_off(len);
                    let bytes = std::mem::replace(&mut self.buf, rest);
                    out.push(Jpeg {
                        bytes,
                        width,
                        height,
                    });
                }
                None if self.buf.len() > MAX_JPEG_BYTES => return Err(()),
                None => return Ok(out),
            }
        }
    }
}

/// Length and size of the first complete JPEG of `b`; `Ok(None)` = more
/// bytes needed. Walks the marker segments by their lengths, then the
/// entropy-coded data up to EOI (a stuffed `FF 00` or a restart marker is
/// data), so a `FF D9` inside a table is never taken for the end.
fn scan(b: &[u8]) -> Result<Option<(usize, u16, u16)>, ()> {
    if b.len() < 2 {
        return Ok(None);
    }
    if b[0] != 0xFF || b[1] != 0xD8 {
        return Err(());
    }
    let (mut width, mut height) = (0u16, 0u16);
    let mut i = 2;
    loop {
        if i + 1 >= b.len() {
            return Ok(None);
        }
        if b[i] != 0xFF {
            return Err(());
        }
        let marker = b[i + 1];
        match marker {
            0xFF => {
                i += 1; // fill byte
                continue;
            }
            0xD9 => return Ok(Some((i + 2, width, height))),
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            _ => {}
        }
        if i + 3 >= b.len() {
            return Ok(None);
        }
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        if len < 2 {
            return Err(());
        }
        // SOF0..SOF15 except DHT (C4), JPG (C8), DAC (CC): frame size.
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            if i + 8 >= b.len() {
                return Ok(None);
            }
            height = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            width = u16::from_be_bytes([b[i + 7], b[i + 8]]);
        }
        i += 2 + len;
        if marker == 0xDA {
            // Entropy-coded data: up to the next real marker.
            loop {
                if i + 1 >= b.len() {
                    return Ok(None);
                }
                if b[i] == 0xFF && b[i + 1] != 0x00 && !(0xD0..=0xD7).contains(&b[i + 1]) {
                    break;
                }
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal JPEG: SOI, a DQT whose table holds `FF D9`, SOF0 640x480,
    /// SOS, entropy data with a stuffed `FF 00` and a restart marker, EOI.
    fn jpeg() -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        j.extend([0xFF, 0xDB, 0x00, 0x05, 0x00, 0xFF, 0xD9]);
        j.extend([
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x01, 0xE0, 0x02, 0x80, 0x01, 0x01, 0x11, 0x00,
        ]);
        j.extend([0xFF, 0xDA, 0x00, 0x02]);
        j.extend([0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD3, 0x56]);
        j.extend([0xFF, 0xD9]);
        j
    }

    #[test]
    fn splits_frames_across_chunks() {
        let one = jpeg();
        let mut stream = one.clone();
        stream.extend(&one);
        let mut s = JpegSplitter::default();
        let mut frames = Vec::new();
        for chunk in stream.chunks(3) {
            frames.extend(s.push(chunk).unwrap());
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].bytes, one);
        assert_eq!((frames[0].width, frames[0].height), (640, 480));
        assert!(s.buf.is_empty());
    }

    #[test]
    fn refuses_non_jpeg_and_oversized() {
        assert!(JpegSplitter::default().push(b"RIFF....").is_err());
        let mut s = JpegSplitter::default();
        let mut big = vec![0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02];
        big.resize(MAX_JPEG_BYTES + 10, 0x11);
        assert!(s.push(&big).is_err());
    }
}
