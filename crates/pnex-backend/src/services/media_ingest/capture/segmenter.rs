//! PCM → segments (media-ingest.md D160): the decoder writes 16 kHz mono
//! s16le on stdout; the segmenter cuts it into `segment_secs` pieces that
//! carry `overlap_secs` more audio (the next segment starts at the nominal
//! boundary, the ASR glues overlapping words, D166). Time comes from the
//! sample count, so segments never drift from each other.

use chrono::{DateTime, Duration, Utc};
use pnex_core::media_ingest::SAMPLE_RATE;

/// Bytes per sample (s16le mono).
const BYTES_PER_SAMPLE: usize = 2;

/// One cut segment: WAV bytes and its absolute times.
#[derive(Debug, Clone)]
pub struct Cut {
    pub seq: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub wav: Vec<u8>,
}

pub struct Segmenter {
    t0: DateTime<Utc>,
    segment_samples: usize,
    overlap_samples: usize,
    /// Samples not yet emitted, starting at `next_start` (sample index).
    buf: Vec<i16>,
    next_start: u64,
    next_seq: i64,
    /// Odd byte left over between two reads.
    carry: Option<u8>,
}

impl Segmenter {
    /// `t0` = absolute time of the first sample.
    pub fn new(t0: DateTime<Utc>, segment_secs: u32, overlap_secs: u32) -> Self {
        let segment_samples = (segment_secs * SAMPLE_RATE) as usize;
        Self {
            t0,
            segment_samples,
            overlap_samples: (overlap_secs * SAMPLE_RATE) as usize,
            buf: Vec::with_capacity(segment_samples * 2),
            next_start: 0,
            next_seq: 0,
            carry: None,
        }
    }

    fn time_of(&self, sample: u64) -> DateTime<Utc> {
        self.t0 + Duration::microseconds((sample as i64 * 1_000_000) / SAMPLE_RATE as i64)
    }

    /// Feeds decoder output; returns the segments completed by it.
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<Cut> {
        if let Some(lo) = self.carry.take() {
            if let Some((&hi, rest)) = bytes.split_first() {
                self.buf.push(i16::from_le_bytes([lo, hi]));
                bytes = rest;
            } else {
                self.carry = Some(lo);
            }
        }
        let (pairs, rest) = bytes.as_chunks::<BYTES_PER_SAMPLE>();
        self.buf
            .extend(pairs.iter().map(|c| i16::from_le_bytes(*c)));
        if let [lo] = rest {
            self.carry = Some(*lo);
        }
        let mut out = Vec::new();
        while self.buf.len() >= self.segment_samples + self.overlap_samples {
            out.push(self.cut(self.segment_samples + self.overlap_samples));
        }
        out
    }

    /// Emits what is left (end of a finite file, or of a capture run) when
    /// it is at least one second long.
    pub fn finish(&mut self) -> Option<Cut> {
        (self.buf.len() >= SAMPLE_RATE as usize).then(|| self.cut(self.buf.len()))
    }

    /// Emits `len` samples from the buffer start, then drops the nominal
    /// segment length (the overlap stays for the next segment).
    fn cut(&mut self, len: usize) -> Cut {
        let samples = &self.buf[..len];
        let cut = Cut {
            seq: self.next_seq,
            started_at: self.time_of(self.next_start),
            ended_at: self.time_of(self.next_start + len as u64),
            wav: wav_bytes(samples),
        };
        let advance = self.segment_samples.min(len);
        self.buf.drain(..advance);
        self.next_start += advance as u64;
        self.next_seq += 1;
        if len < self.segment_samples {
            self.buf.clear();
        }
        cut
    }
}

/// 16 kHz mono 16-bit PCM WAV.
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * BYTES_PER_SAMPLE) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * BYTES_PER_SAMPLE as u32).to_le_bytes());
    out.extend_from_slice(&(BYTES_PER_SAMPLE as u16).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(seconds: f64, value: i16) -> Vec<u8> {
        let n = (seconds * SAMPLE_RATE as f64) as usize;
        std::iter::repeat_n(value.to_le_bytes(), n)
            .flatten()
            .collect()
    }

    #[test]
    fn cuts_with_overlap_on_a_fixed_timeline() {
        let t0 = DateTime::parse_from_rfc3339("2026-10-10T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut seg = Segmenter::new(t0, 10, 1);
        // 25 s in odd-sized reads.
        let bytes = pcm(25.0, 7);
        let mut cuts = Vec::new();
        for chunk in bytes.chunks(4097) {
            cuts.extend(seg.push(chunk));
        }
        assert_eq!(cuts.len(), 2);
        assert_eq!(cuts[0].seq, 0);
        assert_eq!(cuts[0].started_at, t0);
        assert_eq!(cuts[0].ended_at, t0 + Duration::seconds(11));
        assert_eq!(cuts[1].started_at, t0 + Duration::seconds(10));
        assert_eq!(cuts[0].wav.len(), 44 + 11 * SAMPLE_RATE as usize * 2);
        // 5 s left: emitted at the end, from the 20 s boundary.
        let last = seg.finish().unwrap();
        assert_eq!(last.seq, 2);
        assert_eq!(last.started_at, t0 + Duration::seconds(20));
        assert_eq!(last.ended_at, t0 + Duration::seconds(25));
        assert!(seg.finish().is_none());
    }

    #[test]
    fn wav_is_readable_by_the_asr_reader() {
        let wav = wav_bytes(&[0, 1000, -1000, i16::MAX]);
        let path = std::env::temp_dir().join(format!("pnex-seg-{}.wav", std::process::id()));
        std::fs::write(&path, &wav).unwrap();
        let samples = pnex_asr_read(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(samples.len(), 4);
    }

    /// Minimal RIFF check without pulling pnex-asr into the backend tests.
    fn pnex_asr_read(path: &std::path::Path) -> Vec<i16> {
        let b = std::fs::read(path).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(&b[8..12], b"WAVE");
        assert_eq!(
            u32::from_le_bytes(b[24..28].try_into().unwrap()),
            SAMPLE_RATE
        );
        b[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect()
    }
}
