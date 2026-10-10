//! Capture chain of a media stream (media-ingest.md D160), shared by the
//! server carriers (`server`, `worker`) and the capture boxes (edge agent,
//! `capture_on = device:<id>`, lot 6b):
//!
//! filtered fetcher (HTTP family, HLS, ICY) or RTSP client → confined
//! ffmpeg (`pipe:0` → `pipe:1`, no network, no file) → PCM segmenter /
//! JPEG splitter → a [`Host`] that stores or uploads what was cut.
//!
//! No database, no storage, no bus: the host owns all of that.

pub mod decoder;
pub mod fetch;
pub mod hls;
pub mod icy;
pub mod rtsp;
pub mod sandbox;
pub mod segmenter;
pub mod video;

use std::future::Future;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use pnex_core::media_ingest::{ClockSource, MediaStreamKind, FRAME_MAX_WIDTH};
use reqwest::Url;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use self::fetch::{Auth, Fetcher, MetadataEvent};
use self::sandbox::SandboxMode;
use self::segmenter::Segmenter;

/// Restart backoff bounds of a capture supervisor.
pub const BACKOFF_MIN: Duration = Duration::from_secs(5);
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// A run longer than this resets the backoff.
pub const HEALTHY_RUN: Duration = Duration::from_secs(300);
/// No decoded output for this long = stalled stream.
pub const SILENT_INPUT: Duration = Duration::from_secs(60);

/// Why a capture run ended. The wire codes land in
/// `media_streams.capture_error` (short machine codes, no detail: D160).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureError {
    Unreachable,
    FormatUnsupported,
    Encrypted,
    TooLarge,
    Stalled,
    DecoderFailed,
    DecoderMissing,
    SecretUnreadable,
    StoreFailed,
}

impl CaptureError {
    pub const ALL: [CaptureError; 9] = [
        Self::Unreachable,
        Self::FormatUnsupported,
        Self::Encrypted,
        Self::TooLarge,
        Self::Stalled,
        Self::DecoderFailed,
        Self::DecoderMissing,
        Self::SecretUnreadable,
        Self::StoreFailed,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::FormatUnsupported => "format-unsupported",
            Self::Encrypted => "encrypted",
            Self::TooLarge => "too-large",
            Self::Stalled => "stalled",
            Self::DecoderFailed => "decoder-failed",
            Self::DecoderMissing => "decoder-missing",
            Self::SecretUnreadable => "secret-unreadable",
            Self::StoreFailed => "store-failed",
        }
    }

    /// Known code → error; anything else (an untrusted box) is `None`.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.code() == code)
    }
}

impl From<hls::HlsError> for CaptureError {
    fn from(e: hls::HlsError) -> Self {
        match e {
            hls::HlsError::Encrypted => Self::Encrypted,
            hls::HlsError::Empty => Self::Stalled,
            hls::HlsError::NotAPlaylist => Self::FormatUnsupported,
        }
    }
}

/// Process-level capture settings (`PNEX_MEDIA_*`).
#[derive(Debug, Clone)]
pub struct CaptureSettings {
    pub enabled: bool,
    pub ffmpeg: String,
    pub sandbox: SandboxMode,
}

impl CaptureSettings {
    pub fn from_env() -> Self {
        let enabled = !matches!(
            std::env::var("PNEX_MEDIA_CAPTURE")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "0" | "false" | "off" | "no"
        );
        let ffmpeg = std::env::var("PNEX_FFMPEG")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "ffmpeg".into());
        let sandbox = match std::env::var("PNEX_MEDIA_SANDBOX")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "none" => SandboxMode::None,
            "bwrap" => SandboxMode::Bwrap {
                program: std::env::var("PNEX_MEDIA_BWRAP").unwrap_or_else(|_| "bwrap".into()),
                ro_binds: std::env::var("PNEX_MEDIA_SANDBOX_RO_BINDS")
                    .unwrap_or_default()
                    .split(':')
                    .filter(|d| !d.is_empty())
                    .map(str::to_string)
                    .collect(),
            },
            _ => SandboxMode::Kernel,
        };
        Self {
            enabled,
            ffmpeg,
            sandbox,
        }
    }
}

/// A cut audio segment, numbered by the run.
pub struct Segment {
    pub seq: i64,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub clock_source: ClockSource,
    pub wav: Vec<u8>,
}

/// What a run needs to know about its stream.
pub struct Spec<'a> {
    /// Stream slug, for the logs only.
    pub slug: &'a str,
    pub kind: MediaStreamKind,
    /// Already checked by the carrier (scheme, credentials, egress host).
    pub url: &'a Url,
    /// Vault secret of the stream (header or `user:password`), in memory.
    pub secret: Option<&'a str>,
    pub want_audio: bool,
    pub want_video: bool,
    /// Video frames per second (clamped by the caller).
    pub fps: u32,
    pub segment_secs: u32,
    pub overlap_secs: u32,
    /// Whether in-band metadata is handed to [`Host::metadata`].
    pub metadata: bool,
}

/// Where a run's output goes: the carrier-specific part of the capture.
pub trait Host: Send {
    /// First decoded output of the run.
    fn running(&mut self) -> impl Future<Output = ()> + Send;
    /// One audio segment; an error ends the run.
    fn segment(&mut self, seg: Segment) -> impl Future<Output = Result<(), CaptureError>> + Send;
    /// One video frame, best effort.
    fn frame(&mut self, frame: video::Jpeg) -> impl Future<Output = ()> + Send;
    /// In-band metadata events of the run; the host spawns their consumer
    /// (the channel closes with the fetcher).
    fn metadata(&mut self, events: mpsc::Receiver<MetadataEvent>);
}

/// How a run ended without error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// The source finished (finite file, HLS `ENDLIST`).
    Finished,
    /// Asked to stop.
    Stopped,
}

/// One input of a decoder: its format and its bytes.
pub struct Track {
    pub format: decoder::InputFormat,
    pub chunks: mpsc::Receiver<Result<Bytes, CaptureError>>,
}

/// Duplicates a muxed input for the audio and the video decoders. A side
/// that stops reading is dropped; the other goes on.
fn tee(t: Track) -> (Track, Track) {
    let (a_tx, a_rx) = mpsc::channel(4);
    let (v_tx, v_rx) = mpsc::channel(4);
    let mut src = t.chunks;
    tokio::spawn(async move {
        while let Some(item) = src.recv().await {
            let a = a_tx.send(item.clone()).await.is_ok();
            let v = v_tx.send(item).await.is_ok();
            if !a && !v {
                return;
            }
        }
    });
    (
        Track {
            format: t.format,
            chunks: a_rx,
        },
        Track {
            format: t.format,
            chunks: v_rx,
        },
    )
}

/// A confined decoder fed by `track`; the feeder returns the fetch error
/// that ended the input, if any (a decoder that stops reading is judged by
/// its exit status, not here).
struct Decoder {
    child: tokio::process::Child,
    feeder: tokio::task::JoinHandle<Result<(), CaptureError>>,
}

fn spawn_decoder(
    settings: &CaptureSettings,
    argv: Vec<String>,
    slug: &str,
    track: Track,
) -> Result<(Decoder, tokio::process::ChildStdout), CaptureError> {
    let argv = sandbox::wrap_argv(&settings.sandbox, argv);
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    sandbox::confine(&mut cmd, &settings.sandbox, sandbox::DECODER_MAX_MEMORY);
    let mut child = cmd.spawn().map_err(|e| {
        tracing::warn!(error = %e, "media decoder spawn failed");
        CaptureError::DecoderMissing
    })?;
    let mut stdin = child.stdin.take().ok_or(CaptureError::DecoderFailed)?;
    let stdout = child.stdout.take().ok_or(CaptureError::DecoderFailed)?;
    if let Some(stderr) = child.stderr.take() {
        let slug = slug.to_string();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(stream = %slug, "ffmpeg: {line}");
            }
        });
    }
    let mut chunks = track.chunks;
    let feeder = tokio::spawn(async move {
        while let Some(chunk) = chunks.recv().await {
            if stdin.write_all(&chunk?).await.is_err() {
                return Ok(());
            }
        }
        Ok::<(), CaptureError>(())
    });
    Ok((Decoder { child, feeder }, stdout))
}

/// What the decoder readers hand to the capture loop.
enum Decoded {
    Pcm(Vec<u8>),
    Frame(video::Jpeg),
}

/// Captures `spec` once: to its end (finite sources), until `stop`, or
/// until an error. `seq` numbers the segments and keeps counting across
/// runs.
pub async fn run_once<H: Host>(
    host: &mut H,
    settings: &CaptureSettings,
    ffmpeg: &Path,
    spec: &Spec<'_>,
    seq: &mut i64,
    mut stop: watch::Receiver<bool>,
) -> Result<RunEnd, CaptureError> {
    let url = spec.url;
    let mut first_pdt = None;
    let (audio, video) = if spec.kind == MediaStreamKind::Rtsp {
        let creds = match spec.secret {
            Some(s) => Some(rtsp::credentials(s).ok_or(CaptureError::SecretUnreadable)?),
            None => None,
        };
        let opened = tokio::select! {
            r = rtsp::open(url, creds, spec.want_audio, spec.want_video) => r?,
            _ = stop.changed() => return Ok(RunEnd::Stopped),
        };
        (opened.audio, opened.video)
    } else {
        let auth = match spec.secret {
            Some(s) => Some(Auth::parse(s, url).ok_or(CaptureError::SecretUnreadable)?),
            None => None,
        };
        let mut opened = tokio::select! {
            r = Fetcher::new(auth)?.open(spec.kind, url) => r?,
            _ = stop.changed() => return Ok(RunEnd::Stopped),
        };
        if let Some(events) = opened.metadata.take().filter(|_| spec.metadata) {
            host.metadata(events);
        }
        first_pdt = opened.first_pdt;
        let format = opened.format;
        let track = Track {
            format,
            chunks: opened.chunks,
        };
        match (
            spec.want_audio && format.has_audio(),
            spec.want_video && format.has_video(),
        ) {
            (true, true) => {
                let (a, v) = tee(track);
                (Some(a), Some(v))
            }
            (true, false) => (Some(track), None),
            (false, true) => (None, Some(track)),
            (false, false) => (None, None),
        }
    };
    if audio.is_none() && video.is_none() {
        return Err(CaptureError::FormatUnsupported);
    }

    // Decoders → one event channel; it closes when every reader ended.
    let (tx, mut rx) = mpsc::channel::<Decoded>(8);
    let mut decoders: Vec<(Decoder, &'static str)> = Vec::new();
    if let Some(track) = audio {
        let argv = decoder::argv(ffmpeg, track.format);
        let (d, mut out) = spawn_decoder(settings, argv, spec.slug, track)?;
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 || tx.send(Decoded::Pcm(buf[..n].to_vec())).await.is_err() {
                    return;
                }
            }
        });
        decoders.push((d, "audio"));
    }
    if let Some(track) = video {
        let argv = decoder::video_argv(ffmpeg, track.format, spec.fps, FRAME_MAX_WIDTH);
        let (d, mut out) = spawn_decoder(settings, argv, spec.slug, track)?;
        let tx = tx.clone();
        let slug = spec.slug.to_string();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 64 * 1024];
            let mut splitter = video::JpegSplitter::default();
            while let Ok(n) = out.read(&mut buf).await {
                if n == 0 {
                    return;
                }
                let Ok(frames) = splitter.push(&buf[..n]) else {
                    tracing::warn!(stream = %slug, "video decoder output is not a JPEG stream");
                    return;
                };
                for f in frames {
                    if tx.send(Decoded::Frame(f)).await.is_err() {
                        return;
                    }
                }
            }
        });
        decoders.push((d, "video"));
    }
    drop(tx);

    let (t0, clock) = match first_pdt {
        Some(pdt) => (pdt, ClockSource::Pdt),
        None => (Utc::now(), ClockSource::Host),
    };
    let mut cutter = Segmenter::new(t0, spec.segment_secs.max(1), spec.overlap_secs);
    let (mut got_audio, mut got_video) = (false, false);
    let outcome = loop {
        let ev = tokio::select! {
            r = tokio::time::timeout(SILENT_INPUT, rx.recv()) => r,
            _ = stop.changed() => break Ok(RunEnd::Stopped),
        };
        let ev = match ev {
            Err(_) => break Err(CaptureError::Stalled),
            Ok(None) => break Ok(RunEnd::Finished),
            Ok(Some(ev)) => ev,
        };
        if !got_audio && !got_video {
            host.running().await;
        }
        match ev {
            Decoded::Pcm(bytes) => {
                got_audio = true;
                for cut in cutter.push(&bytes) {
                    host.segment(numbered(seq, cut, clock)).await?;
                }
            }
            Decoded::Frame(frame) => {
                got_video = true;
                host.frame(frame).await;
            }
        }
    };
    if matches!(outcome, Ok(RunEnd::Finished)) {
        if let Some(cut) = cutter.finish() {
            host.segment(numbered(seq, cut, clock)).await?;
        }
    }
    let mut fed = Ok(());
    let mut failed = Vec::new();
    for (mut d, track) in decoders {
        let _ = d.child.start_kill();
        let r = d.feeder.await.unwrap_or(Err(CaptureError::DecoderFailed));
        if fed.is_ok() {
            fed = r;
        }
        let ok = d.child.wait().await.is_ok_and(|s| s.success());
        let produced = if track == "audio" {
            got_audio
        } else {
            got_video
        };
        failed.push((produced, ok));
    }
    match outcome {
        Ok(RunEnd::Finished) => {
            // The decoders ended: a fetch error explains it better.
            fed?;
            // A decoder that never produced anything next to one that did
            // is a missing track (a camera without audio), not a failure.
            let any = got_audio || got_video;
            if failed
                .iter()
                .any(|&(produced, ok)| !ok && (produced || !any))
            {
                return Err(CaptureError::DecoderFailed);
            }
            Ok(RunEnd::Finished)
        }
        other => other,
    }
}

fn numbered(seq: &mut i64, cut: segmenter::Cut, clock: ClockSource) -> Segment {
    let seg = Segment {
        seq: *seq,
        started_at: cut.started_at,
        ended_at: cut.ended_at,
        clock_source: clock,
        wav: cut.wav,
    };
    *seq += 1;
    seg
}

/// Decodes an uploaded clip with the confined decoder (D160 regime) into
/// a 16 kHz mono WAV of at most `max_secs`; the flag says it was cut.
pub async fn decode_clip(
    bytes: Vec<u8>,
    format: decoder::InputFormat,
    max_secs: u32,
) -> Result<(Vec<u8>, bool), CaptureError> {
    let settings = CaptureSettings::from_env();
    let ffmpeg = decoder::resolve(&settings.ffmpeg).ok_or(CaptureError::DecoderMissing)?;
    let argv = sandbox::wrap_argv(&settings.sandbox, decoder::argv(&ffmpeg, format));
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    sandbox::confine(&mut cmd, &settings.sandbox, sandbox::DECODER_MAX_MEMORY);
    let mut child = cmd.spawn().map_err(|_| CaptureError::DecoderMissing)?;
    let mut stdin = child.stdin.take().ok_or(CaptureError::DecoderFailed)?;
    let mut stdout = child.stdout.take().ok_or(CaptureError::DecoderFailed)?;
    let feeder = tokio::spawn(async move {
        let _ = stdin.write_all(&bytes).await;
    });
    let max_bytes = max_secs as usize * pnex_core::media_ingest::SAMPLE_RATE as usize * 2;
    let mut pcm = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut truncated = false;
    let read = tokio::time::timeout(SILENT_INPUT, async {
        loop {
            let n = stdout
                .read(&mut buf)
                .await
                .map_err(|_| CaptureError::DecoderFailed)?;
            if n == 0 {
                return Ok::<(), CaptureError>(());
            }
            pcm.extend_from_slice(&buf[..n]);
            if pcm.len() >= max_bytes {
                pcm.truncate(max_bytes);
                truncated = true;
                return Ok(());
            }
        }
    })
    .await;
    feeder.abort();
    let _ = child.start_kill();
    read.map_err(|_| CaptureError::Stalled)??;
    pcm.truncate(pcm.len() & !1);
    if pcm.is_empty() {
        return Err(CaptureError::FormatUnsupported);
    }
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    Ok((segmenter::wav_bytes(&samples), truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tee_feeds_both_sides_and_survives_one_leaving() {
        let (tx, rx) = mpsc::channel(4);
        let (mut a, v) = tee(Track {
            format: decoder::H264,
            chunks: rx,
        });
        tx.send(Ok(Bytes::from_static(b"one"))).await.unwrap();
        let mut v = v.chunks;
        assert_eq!(&a.chunks.recv().await.unwrap().unwrap()[..], b"one");
        assert_eq!(&v.recv().await.unwrap().unwrap()[..], b"one");
        // The video decoder goes away: audio keeps flowing.
        drop(v);
        tx.send(Ok(Bytes::from_static(b"two"))).await.unwrap();
        assert_eq!(&a.chunks.recv().await.unwrap().unwrap()[..], b"two");
        drop(tx);
        assert!(a.chunks.recv().await.is_none());
    }

    #[test]
    fn error_codes_round_trip() {
        for e in CaptureError::ALL {
            assert_eq!(CaptureError::from_code(e.code()), Some(e));
        }
        assert_eq!(CaptureError::from_code("<script>"), None);
    }
}
