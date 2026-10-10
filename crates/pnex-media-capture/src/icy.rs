//! ICY in-band metadata (media-ingest.md D170): with `Icy-MetaData: 1`,
//! an Icecast/Shoutcast server interleaves a metadata block every
//! `icy-metaint` audio bytes (one length byte × 16, then the block). The
//! demuxer strips the blocks so only audio reaches the decoder, and
//! extracts `StreamTitle='…';`.

/// Accepted `icy-metaint` range; outside it the stream is read as plain
/// audio (no metadata requested back).
pub const METAINT_MIN: usize = 256;
pub const METAINT_MAX: usize = 65_536;
/// Longest title kept, in bytes (cut on a character boundary).
pub const TITLE_MAX_BYTES: usize = 512;

/// `icy-metaint` header value when usable.
pub fn metaint(header: Option<&str>) -> Option<usize> {
    header?
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (METAINT_MIN..=METAINT_MAX).contains(n))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Audio bytes left before the next length byte.
    Audio(usize),
    /// Next byte is the block length (× 16).
    Length,
    /// Metadata bytes left in the current block.
    Meta(usize),
}

/// Stateful splitter: feed it the raw body chunk by chunk.
pub struct Demuxer {
    metaint: usize,
    state: State,
    block: Vec<u8>,
}

impl Demuxer {
    pub fn new(metaint: usize) -> Self {
        Self {
            metaint,
            state: State::Audio(metaint),
            block: Vec::new(),
        }
    }

    /// Appends the audio bytes of `input` to `audio`; returns the titles of
    /// the metadata blocks completed in it (a block split across chunks is
    /// completed by a later call). Empty blocks yield nothing.
    pub fn push(&mut self, mut input: &[u8], audio: &mut Vec<u8>) -> Vec<String> {
        let mut titles = Vec::new();
        while !input.is_empty() {
            match self.state {
                State::Audio(left) => {
                    let n = left.min(input.len());
                    audio.extend_from_slice(&input[..n]);
                    input = &input[n..];
                    self.state = if n == left {
                        State::Length
                    } else {
                        State::Audio(left - n)
                    };
                }
                State::Length => {
                    let len = input[0] as usize * 16;
                    input = &input[1..];
                    self.block.clear();
                    self.state = if len == 0 {
                        State::Audio(self.metaint)
                    } else {
                        State::Meta(len)
                    };
                }
                State::Meta(left) => {
                    let n = left.min(input.len());
                    self.block.extend_from_slice(&input[..n]);
                    input = &input[n..];
                    if n == left {
                        if let Some(t) = stream_title(&self.block) {
                            titles.push(t);
                        }
                        self.block.clear();
                        self.state = State::Audio(self.metaint);
                    } else {
                        self.state = State::Meta(left - n);
                    }
                }
            }
        }
        titles
    }
}

/// `StreamTitle='…';` of a metadata block (NUL padding ignored), lossy
/// UTF-8, trimmed, at most [`TITLE_MAX_BYTES`]. `None` when absent or
/// empty (an empty title carries nothing to record).
pub fn stream_title(block: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"StreamTitle='";
    let start = block.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let rest = &block[start..];
    // The value ends at `';` (a title may itself contain a quote).
    let end = rest
        .windows(2)
        .position(|w| w == b"';")
        .or_else(|| rest.iter().rposition(|&b| b == b'\''))
        .unwrap_or(rest.len());
    let raw = String::from_utf8_lossy(&rest[..end]);
    let mut title = raw
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .to_string();
    if title.len() > TITLE_MAX_BYTES {
        let mut cut = TITLE_MAX_BYTES;
        while !title.is_char_boundary(cut) {
            cut -= 1;
        }
        title.truncate(cut);
    }
    (!title.is_empty()).then_some(title)
}

/// Emission gate of one stream: a title goes out only when it changed, at
/// most once per `min_gap`; a change arriving sooner waits for the next
/// chunk after the gap (the latest one wins).
pub struct TitleGate {
    min_gap: std::time::Duration,
    last_sent: Option<(String, std::time::Instant)>,
    pending: Option<String>,
}

impl TitleGate {
    pub fn new(min_gap: std::time::Duration) -> Self {
        Self {
            min_gap,
            last_sent: None,
            pending: None,
        }
    }

    /// Offers new titles (possibly none) at `now`; returns the one to emit.
    pub fn offer(&mut self, titles: Vec<String>, now: std::time::Instant) -> Option<String> {
        if let Some(t) = titles.into_iter().last() {
            self.pending = Some(t);
        }
        let candidate = self.pending.as_ref()?;
        match &self.last_sent {
            Some((last, _)) if last == candidate => {
                self.pending = None;
                None
            }
            Some((_, at)) if now.duration_since(*at) < self.min_gap => None,
            _ => {
                let t = self.pending.take()?;
                self.last_sent = Some((t.clone(), now));
                Some(t)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Builds an ICY body: `audio` cut every `metaint` bytes, each cut
    /// followed by the next block of `blocks` (or an empty block).
    fn body(audio: &[u8], metaint: usize, blocks: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut blocks = blocks.iter();
        for chunk in audio.chunks(metaint) {
            out.extend_from_slice(chunk);
            if chunk.len() < metaint {
                break;
            }
            match blocks.next().filter(|b| !b.is_empty()) {
                Some(b) => {
                    let len = b.len().div_ceil(16);
                    out.push(len as u8);
                    let mut padded = b.as_bytes().to_vec();
                    padded.resize(len * 16, 0);
                    out.extend_from_slice(&padded);
                }
                None => out.push(0),
            }
        }
        out
    }

    fn audio(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn strips_blocks_and_reads_titles_for_any_chunking() {
        let metaint = 256;
        let pcm = audio(metaint * 4 + 100);
        let raw = body(
            &pcm,
            metaint,
            &[
                "StreamTitle='Artiste - Morceau';StreamUrl='';",
                "",
                "StreamTitle='L''invité: «Économie»';",
                "StreamTitle='';",
            ],
        );
        // Every chunk size, so blocks and titles split across reads.
        for size in [1, 7, 16, 255, 256, 257, 1000, raw.len()] {
            let mut d = Demuxer::new(metaint);
            let mut out = Vec::new();
            let mut titles = Vec::new();
            for chunk in raw.chunks(size) {
                titles.extend(d.push(chunk, &mut out));
            }
            assert_eq!(out, pcm, "audio intact with chunks of {size}");
            assert_eq!(
                titles,
                vec![
                    "Artiste - Morceau".to_string(),
                    "L''invité: «Économie»".to_string(),
                ],
                "chunks of {size}"
            );
        }
    }

    #[test]
    fn titles_are_bounded_and_lossy() {
        let long = format!("StreamTitle='{}';", "é".repeat(400));
        let t = stream_title(long.as_bytes()).unwrap();
        assert!(t.len() <= TITLE_MAX_BYTES && t.chars().all(|c| c == 'é'));
        assert_eq!(
            stream_title(b"StreamTitle='a\xffb';\0\0").as_deref(),
            Some("a\u{fffd}b")
        );
        assert_eq!(stream_title(b"StreamUrl='x';"), None);
        assert_eq!(stream_title(b"StreamTitle='  ';"), None);
        assert_eq!(stream_title(b"StreamTitle='cut"), Some("cut".into()));
    }

    #[test]
    fn metaint_bounds() {
        assert_eq!(metaint(Some("16000")), Some(16000));
        assert_eq!(metaint(Some("8")), None);
        assert_eq!(metaint(Some("100000")), None);
        assert_eq!(metaint(Some("x")), None);
        assert_eq!(metaint(None), None);
    }

    #[test]
    fn gate_emits_changes_at_most_once_per_gap() {
        let t0 = Instant::now();
        let s = |x: &str| vec![x.to_string()];
        let mut g = TitleGate::new(Duration::from_secs(1));
        assert_eq!(g.offer(s("a"), t0).as_deref(), Some("a"));
        // Same title: never again.
        assert_eq!(g.offer(s("a"), t0 + Duration::from_secs(5)), None);
        // A change within the gap waits, the latest one wins.
        let t1 = t0 + Duration::from_secs(10);
        assert_eq!(g.offer(s("b"), t1).as_deref(), Some("b"));
        assert_eq!(g.offer(s("c"), t1 + Duration::from_millis(100)), None);
        assert_eq!(g.offer(s("d"), t1 + Duration::from_millis(200)), None);
        assert_eq!(
            g.offer(Vec::new(), t1 + Duration::from_millis(1100))
                .as_deref(),
            Some("d")
        );
        assert_eq!(g.offer(Vec::new(), t1 + Duration::from_secs(3)), None);
    }
}
