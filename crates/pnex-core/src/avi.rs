//! Minimal MJPEG-in-AVI (RIFF) writer and reader (camera-video.md D78).
//!
//! Recording segments are plain AVI 1.0 files with one `MJPG` video
//! stream and an `idx1` index: playable by VLC/ffmpeg, no transcoding, no
//! native dependency. The writer runs in the flow runtime (`video-record`),
//! the reader in the web UI player (frame offsets from `idx1`). Pure and
//! wasm-safe.

/// Writer accumulating JPEG frames in memory, finalized into AVI bytes.
#[derive(Debug, Default, Clone)]
pub struct AviWriter {
    width: u16,
    height: u16,
    frames: Vec<Vec<u8>>,
    bytes: usize,
}

impl AviWriter {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            frames: Vec::new(),
            bytes: 0,
        }
    }

    pub fn push(&mut self, jpeg: Vec<u8>) {
        self.bytes += jpeg.len();
        self.frames.push(jpeg);
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Accumulated JPEG payload bytes (container overhead excluded).
    pub fn payload_bytes(&self) -> usize {
        self.bytes
    }

    pub fn dims(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Builds the AVI file; `fps` is the playback rate written in the
    /// headers (derive it from the segment duration for real-time
    /// playback). Consumes the writer.
    pub fn finish(self, fps: f64) -> Vec<u8> {
        let frames: Vec<&[u8]> = self.frames.iter().map(Vec::as_slice).collect();
        write_avi(self.width, self.height, fps, &frames)
    }
}

/// Builds an MJPEG AVI from borrowed JPEG frames — used by [`AviWriter`]
/// and to concatenate recorded segments into one file without copying
/// every frame first.
pub fn write_avi(width: u16, height: u16, fps: f64, frames: &[&[u8]]) -> Vec<u8> {
    let payload: usize = frames.iter().map(|f| f.len()).sum();
    let fps = if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        1.0
    };
    // Rate/scale with millisecond precision (e.g. 4.873 fps).
    let scale: u32 = 1000;
    let rate: u32 = ((fps * 1000.0).round() as u32).max(1);
    let usec_per_frame = (1_000_000.0 / fps).round() as u32;
    let n = frames.len() as u32;
    let max_frame = frames.iter().map(|f| f.len()).max().unwrap_or(0) as u32;
    let (w, h) = (u32::from(width), u32::from(height));

    // ── hdrl ──
    let mut avih = Vec::with_capacity(56);
    for v in [
        usec_per_frame,
        (max_frame.saturating_mul(fps.ceil() as u32)).max(1), // max bytes/sec
        0,                                                    // padding granularity
        0x10,                                                 // AVIF_HASINDEX
        n,                                                    // total frames
        0,                                                    // initial frames
        1,                                                    // streams
        max_frame,                                            // suggested buffer
        w,
        h,
        0,
        0,
        0,
        0,
    ] {
        avih.extend_from_slice(&v.to_le_bytes());
    }

    let mut strh = Vec::with_capacity(56);
    strh.extend_from_slice(b"vids");
    strh.extend_from_slice(b"MJPG");
    for v in [0u32, 0, 0, scale, rate, 0, n, max_frame, u32::MAX, 0] {
        // flags, priority+language, initial frames, scale, rate, start,
        // length, suggested buffer, quality (-1), sample size
        strh.extend_from_slice(&v.to_le_bytes());
    }
    // rcFrame: left, top, right, bottom (i16).
    for v in [0u16, 0, width, height] {
        strh.extend_from_slice(&v.to_le_bytes());
    }

    let mut strf = Vec::with_capacity(40);
    strf.extend_from_slice(&40u32.to_le_bytes()); // biSize
    strf.extend_from_slice(&w.to_le_bytes());
    strf.extend_from_slice(&h.to_le_bytes());
    strf.extend_from_slice(&1u16.to_le_bytes()); // planes
    strf.extend_from_slice(&24u16.to_le_bytes()); // bit count
    strf.extend_from_slice(b"MJPG");
    strf.extend_from_slice(&(w * h * 3).to_le_bytes()); // image size
    for _ in 0..4 {
        strf.extend_from_slice(&0u32.to_le_bytes());
    }

    let strl = list(
        b"strl",
        &[chunk(b"strh", &strh), chunk(b"strf", &strf)].concat(),
    );
    let hdrl = list(b"hdrl", &[chunk(b"avih", &avih), strl].concat());

    // ── movi + idx1 ──
    let mut movi_body = Vec::with_capacity(payload + frames.len() * 10);
    let mut idx = Vec::with_capacity(frames.len() * 16);
    for jpeg in frames {
        // Offset relative to the `movi` fourcc (classic AVI 1.0 idx1).
        let offset = 4 + movi_body.len() as u32;
        idx.extend_from_slice(b"00dc");
        idx.extend_from_slice(&0x10u32.to_le_bytes()); // AVIIF_KEYFRAME
        idx.extend_from_slice(&offset.to_le_bytes());
        idx.extend_from_slice(&(jpeg.len() as u32).to_le_bytes());
        movi_body.extend_from_slice(&chunk(b"00dc", jpeg));
    }
    let movi = list(b"movi", &movi_body);
    let idx1 = chunk(b"idx1", &idx);

    let mut body = Vec::with_capacity(4 + hdrl.len() + movi.len() + idx1.len());
    body.extend_from_slice(b"AVI ");
    body.extend_from_slice(&hdrl);
    body.extend_from_slice(&movi);
    body.extend_from_slice(&idx1);
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + data.len() + 1);
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0); // RIFF word alignment
    }
    out
}

fn list(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + data.len());
    out.extend_from_slice(b"LIST");
    out.extend_from_slice(&((data.len() + 4) as u32).to_le_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out
}

/// Parsed AVI: playback rate, dimensions and frame byte ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct AviIndex {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    /// `(start, len)` of each JPEG inside the file bytes.
    pub frames: Vec<(usize, usize)>,
}

/// Reads an MJPEG AVI (ours, or any AVI 1.0 with `00dc` chunks). Falls back
/// to a linear `movi` scan when `idx1` is missing.
pub fn read_avi(bytes: &[u8]) -> Result<AviIndex, &'static str> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"AVI " {
        return Err("not an AVI file");
    }
    let mut fps = 0.0;
    let (mut width, mut height) = (0u32, 0u32);
    let mut movi_start: Option<usize> = None; // offset of the `movi` fourcc
    let mut movi_end = 0usize;
    let mut idx1: Option<(usize, usize)> = None;

    let u32_at = |i: usize| -> Option<u32> {
        bytes
            .get(i..i + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };

    // Walk top-level + hdrl/strl lists (bounded, iterative).
    let mut stack = vec![(12usize, bytes.len())];
    while let Some((mut pos, end)) = stack.pop() {
        while pos + 8 <= end {
            let id = &bytes[pos..pos + 4];
            let size = u32_at(pos + 4).ok_or("truncated chunk")? as usize;
            let data = pos + 8;
            let data_end = data.saturating_add(size).min(end);
            match id {
                b"LIST" => {
                    let kind = bytes.get(data..data + 4).ok_or("truncated list")?;
                    if kind == b"movi" {
                        movi_start = Some(data);
                        movi_end = data_end;
                    } else {
                        stack.push((data + 4, data_end));
                    }
                }
                b"avih" => {
                    let us = u32_at(data).unwrap_or(0);
                    if us > 0 && fps == 0.0 {
                        fps = 1_000_000.0 / f64::from(us);
                    }
                    width = u32_at(data + 32).unwrap_or(0);
                    height = u32_at(data + 36).unwrap_or(0);
                }
                b"strh" => {
                    let scale = u32_at(data + 20).unwrap_or(0);
                    let rate = u32_at(data + 24).unwrap_or(0);
                    if scale > 0 && rate > 0 {
                        fps = f64::from(rate) / f64::from(scale);
                    }
                }
                b"idx1" => idx1 = Some((data, data_end)),
                _ => {}
            }
            pos = data + size + (size & 1);
        }
    }

    let movi = movi_start.ok_or("missing movi list")?;
    let mut frames = Vec::new();
    if let Some((mut i, end)) = idx1 {
        while i + 16 <= end {
            let id = &bytes[i..i + 4];
            let off = u32_at(i + 8).unwrap_or(0) as usize;
            let len = u32_at(i + 12).unwrap_or(0) as usize;
            if &id[2..4] == b"dc" || &id[2..4] == b"db" {
                // Offsets are relative to the `movi` fourcc (some writers
                // use absolute offsets — detect by bounds).
                let rel = movi + off + 8;
                let start =
                    if rel + len <= bytes.len() && bytes.get(rel..rel + 2) == Some(&[0xFF, 0xD8]) {
                        rel
                    } else {
                        off + 8
                    };
                if start + len <= bytes.len() {
                    frames.push((start, len));
                }
            }
            i += 16;
        }
    }
    if frames.is_empty() {
        let mut pos = movi + 4;
        while pos + 8 <= movi_end {
            let size = u32_at(pos + 4).ok_or("truncated movi")? as usize;
            if &bytes[pos + 2..pos + 4] == b"dc" && pos + 8 + size <= bytes.len() {
                frames.push((pos + 8, size));
            }
            pos += 8 + size + (size & 1);
        }
    }
    if fps <= 0.0 {
        fps = 1.0;
    }
    Ok(AviIndex {
        fps,
        width,
        height,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jpeg(n: u8, len: usize) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend(std::iter::repeat_n(n, len.saturating_sub(4)));
        v.extend([0xFF, 0xD9]);
        v
    }

    #[test]
    fn roundtrip_frames_and_rate() {
        let mut w = AviWriter::new(640, 480);
        let frames: Vec<Vec<u8>> = (0..5).map(|i| fake_jpeg(i, 100 + usize::from(i))).collect();
        for f in &frames {
            w.push(f.clone());
        }
        assert_eq!(w.frame_count(), 5);
        let bytes = w.finish(4.5);
        let idx = read_avi(&bytes).unwrap();
        assert_eq!((idx.width, idx.height), (640, 480));
        assert!((idx.fps - 4.5).abs() < 1e-6);
        assert_eq!(idx.frames.len(), 5);
        for ((start, len), f) in idx.frames.iter().zip(&frames) {
            assert_eq!(&bytes[*start..*start + *len], f.as_slice());
        }
        // RIFF size is consistent with the file length.
        let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        assert_eq!(riff + 8, bytes.len());
    }

    #[test]
    fn empty_and_garbage() {
        let bytes = AviWriter::new(320, 240).finish(5.0);
        assert!(read_avi(&bytes).unwrap().frames.is_empty());
        assert!(read_avi(b"not an avi at all").is_err());
    }
}
