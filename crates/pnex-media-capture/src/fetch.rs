//! Filtered fetcher (media-ingest.md D160): the only component that talks
//! to the stream's host. Redirects are followed by hand so each hop is
//! re-checked (R8) and the credential only ever goes to the stream's exact
//! origin; HLS playlists are read here, never by ffmpeg. Icecast streams
//! are asked for ICY metadata (D170): the interleaved blocks are stripped
//! here and their titles handed to the capture task, never to ffmpeg.

use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use pnex_core::egress;
use pnex_core::media_ingest::MediaStreamKind;
use reqwest::header::{HeaderName, HeaderValue, CONTENT_TYPE, LOCATION};
use reqwest::Url;
use tokio::sync::mpsc;

use crate::decoder::{input_format, InputFormat};
use crate::hls::{self, Playlist};
use crate::icy;
use crate::CaptureError;

/// `(scheme, host, port)` of a URL: the destination a secret is bound to.
pub fn origin(url: &Url) -> (String, String, Option<u16>) {
    (
        url.scheme().to_string(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        url.port_or_known_default(),
    )
}

const MAX_REDIRECTS: usize = 5;
/// Largest HLS segment (or init section) accepted.
const MAX_SEGMENT_BYTES: usize = 16 * 1024 * 1024;
/// Largest finite file (`http_file`).
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Inbound rate cap: well above any audio stream, bounds a hostile one.
const MAX_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;
/// HLS: a playlist that brings nothing new for this many target durations
/// is stalled.
const STALL_TARGETS: u32 = 6;
/// Chunks buffered between the fetcher and the decoder.
const CHANNEL_DEPTH: usize = 32;
/// In-band metadata events buffered for the capture task; beyond, new
/// events are dropped (audio never waits for metadata).
const METADATA_DEPTH: usize = 8;
/// At most one metadata event per second per stream (D170).
const METADATA_MIN_GAP: Duration = Duration::from_secs(1);

/// One in-band metadata event (`StreamTitle` change), stamped at reception.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataEvent {
    pub at: DateTime<Utc>,
    pub title: String,
}

/// Credential of a stream, sent to its origin only.
#[derive(Clone)]
pub struct Auth {
    name: HeaderName,
    value: HeaderValue,
    origin: (String, String, Option<u16>),
}

impl Auth {
    /// Secret value → header: `Header-Name: value`, or else the whole value
    /// as `Authorization` (`Bearer …`, `Basic …`).
    pub fn parse(secret: &str, stream_url: &Url) -> Option<Self> {
        let secret = secret.trim();
        let (name, value) = match secret.split_once(':') {
            Some((n, v))
                if !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && v.starts_with(' ') =>
            {
                (HeaderName::from_bytes(n.as_bytes()).ok()?, v.trim())
            }
            _ => (reqwest::header::AUTHORIZATION, secret),
        };
        let mut value = HeaderValue::from_str(value).ok()?;
        value.set_sensitive(true);
        Some(Self {
            name,
            value,
            origin: origin(stream_url),
        })
    }
}

/// Inbound bytes, plus the absolute time of the first one when the
/// source states it (HLS `EXT-X-PROGRAM-DATE-TIME`).
pub struct Opened {
    pub format: InputFormat,
    pub first_pdt: Option<DateTime<Utc>>,
    pub chunks: mpsc::Receiver<Result<Bytes, CaptureError>>,
    /// ICY title changes, when the server interleaves metadata.
    pub metadata: Option<mpsc::Receiver<MetadataEvent>>,
}

pub struct Fetcher {
    client: reqwest::Client,
    auth: Option<Auth>,
}

impl Fetcher {
    pub fn new(auth: Option<Auth>) -> Result<Self, CaptureError> {
        let client = egress::guarded(reqwest::Client::builder())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("PNEX-media/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| CaptureError::Unreachable)?;
        Ok(Self { client, auth })
    }

    /// GET with hand-followed redirects; every hop re-checked.
    async fn get(
        &self,
        url: &Url,
        timeout: Option<Duration>,
    ) -> Result<reqwest::Response, CaptureError> {
        self.get_with(url, timeout, false).await
    }

    /// [`Self::get`], asking for ICY metadata when `icy` is set.
    async fn get_with(
        &self,
        url: &Url,
        timeout: Option<Duration>,
        icy: bool,
    ) -> Result<reqwest::Response, CaptureError> {
        let mut url = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            if !matches!(url.scheme(), "http" | "https") || egress::check_url(&url).is_some() {
                return Err(CaptureError::Unreachable);
            }
            let mut req = self.client.get(url.clone());
            if icy {
                req = req.header("Icy-MetaData", "1");
            }
            if let Some(t) = timeout {
                req = req.timeout(t);
            }
            if let Some(auth) = &self.auth {
                if origin(&url) == auth.origin {
                    req = req.header(auth.name.clone(), auth.value.clone());
                }
            }
            let res = req.send().await.map_err(|_| CaptureError::Unreachable)?;
            if res.status().is_redirection() {
                let next = res
                    .headers()
                    .get(LOCATION)
                    .and_then(|l| l.to_str().ok())
                    .and_then(|l| url.join(l).ok())
                    .ok_or(CaptureError::Unreachable)?;
                url = next;
                continue;
            }
            if !res.status().is_success() {
                return Err(CaptureError::Unreachable);
            }
            return Ok(res);
        }
        Err(CaptureError::Unreachable)
    }

    /// Whole body, refused beyond `max` bytes.
    async fn get_bounded(
        &self,
        url: &Url,
        max: usize,
    ) -> Result<(Option<String>, Bytes), CaptureError> {
        let mut res = self.get(url, Some(Duration::from_secs(30))).await?;
        let ct = content_type(&res);
        if res.content_length().is_some_and(|l| l > max as u64) {
            return Err(CaptureError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = res.chunk().await.map_err(|_| CaptureError::Unreachable)? {
            if body.len() + chunk.len() > max {
                return Err(CaptureError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok((ct, Bytes::from(body)))
    }

    /// Opens the stream: decides the input format, then feeds the bytes in
    /// a bounded channel (backpressure from the decoder).
    pub async fn open(self, kind: MediaStreamKind, url: &Url) -> Result<Opened, CaptureError> {
        match kind {
            MediaStreamKind::Icecast | MediaStreamKind::HttpFile => self.open_http(kind, url).await,
            MediaStreamKind::Hls => self.open_hls(url).await,
            // RTSP has its own client (`capture::rtsp`), never this fetcher.
            MediaStreamKind::Rtsp => Err(CaptureError::FormatUnsupported),
        }
    }

    async fn open_http(self, kind: MediaStreamKind, url: &Url) -> Result<Opened, CaptureError> {
        let icy_wanted = kind == MediaStreamKind::Icecast;
        let mut res = self.get_with(url, None, icy_wanted).await?;
        // Without a usable `icy-metaint` the body is plain audio, as before.
        let metaint = icy_wanted
            .then(|| {
                icy::metaint(
                    res.headers()
                        .get("icy-metaint")
                        .and_then(|v| v.to_str().ok()),
                )
            })
            .flatten();
        let (meta_tx, meta_rx) = match metaint {
            Some(_) => {
                let (tx, rx) = mpsc::channel(METADATA_DEPTH);
                (Some(tx), Some(rx))
            }
            None => (None, None),
        };
        let format = input_format(content_type(&res).as_deref(), res.url().path())
            .ok_or(CaptureError::FormatUnsupported)?;
        let limit = (kind == MediaStreamKind::HttpFile).then_some(MAX_FILE_BYTES);
        if limit.is_some_and(|l| res.content_length().is_some_and(|len| len > l)) {
            return Err(CaptureError::TooLarge);
        }
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        tokio::spawn(async move {
            let mut rate = RateCap::new();
            let mut total = 0u64;
            let mut demux = metaint.map(icy::Demuxer::new);
            let mut gate = icy::TitleGate::new(METADATA_MIN_GAP);
            loop {
                let chunk = match tokio::time::timeout(Duration::from_secs(30), res.chunk()).await {
                    Ok(Ok(Some(c))) => c,
                    Ok(Ok(None)) => break,
                    Ok(Err(_)) | Err(_) => {
                        let _ = tx.send(Err(CaptureError::Unreachable)).await;
                        return;
                    }
                };
                total += chunk.len() as u64;
                if limit.is_some_and(|l| total > l) {
                    let _ = tx.send(Err(CaptureError::TooLarge)).await;
                    return;
                }
                rate.consume(chunk.len()).await;
                let chunk = match demux.as_mut() {
                    None => chunk,
                    Some(d) => {
                        let mut audio = Vec::with_capacity(chunk.len());
                        let titles = d.push(&chunk, &mut audio);
                        if let (Some(title), Some(meta)) =
                            (gate.offer(titles, Instant::now()), meta_tx.as_ref())
                        {
                            // Full or closed: the event is dropped, audio goes on.
                            let _ = meta.try_send(MetadataEvent {
                                at: Utc::now(),
                                title,
                            });
                        }
                        if audio.is_empty() {
                            continue;
                        }
                        Bytes::from(audio)
                    }
                };
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Opened {
            format,
            first_pdt: None,
            chunks: rx,
            metadata: meta_rx,
        })
    }

    /// Media playlist URL and its first parse (master → one variant, two
    /// levels at most).
    async fn media_playlist(&self, url: &Url) -> Result<(Url, hls::MediaPlaylist), CaptureError> {
        let mut url = url.clone();
        for _ in 0..2 {
            let (_, body) = self.get_bounded(&url, hls::MAX_PLAYLIST_BYTES).await?;
            let text = std::str::from_utf8(&body).map_err(|_| CaptureError::FormatUnsupported)?;
            match hls::parse(text).map_err(CaptureError::from)? {
                Playlist::Media(m) => return Ok((url, m)),
                Playlist::Master(variants) => {
                    let v = hls::pick_variant(&variants).ok_or(CaptureError::FormatUnsupported)?;
                    url = url.join(&v.uri).map_err(|_| CaptureError::Unreachable)?;
                }
            }
        }
        Err(CaptureError::FormatUnsupported)
    }

    async fn open_hls(self, url: &Url) -> Result<Opened, CaptureError> {
        let (media_url, first) = self.media_playlist(url).await?;
        // Start near the live edge: the last three segments.
        let start = first.segments.len().saturating_sub(3);
        let Some(seg) = first.segments.get(start).cloned() else {
            return Err(CaptureError::Stalled);
        };
        let seg_url = media_url
            .join(&seg.uri)
            .map_err(|_| CaptureError::Unreachable)?;
        let mut head = Vec::new();
        if let Some(map) = &first.map_uri {
            let map_url = media_url.join(map).map_err(|_| CaptureError::Unreachable)?;
            head.push(self.get_bounded(&map_url, MAX_SEGMENT_BYTES).await?.1);
        }
        let (ct, bytes) = self.get_bounded(&seg_url, MAX_SEGMENT_BYTES).await?;
        let format =
            input_format(ct.as_deref(), seg_url.path()).ok_or(CaptureError::FormatUnsupported)?;
        head.push(bytes);
        let first_pdt = seg.program_date_time;
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        tokio::spawn(async move {
            let mut last_seq = seg.sequence;
            let mut playlist = first;
            for b in head {
                if tx.send(Ok(b)).await.is_err() {
                    return;
                }
            }
            let mut idle = 0u32;
            loop {
                let mut fresh = false;
                let since = last_seq;
                for s in playlist.segments.iter().filter(|s| s.sequence > since) {
                    let Ok(u) = media_url.join(&s.uri) else {
                        let _ = tx.send(Err(CaptureError::Unreachable)).await;
                        return;
                    };
                    match self.get_bounded(&u, MAX_SEGMENT_BYTES).await {
                        Ok((_, b)) => {
                            if tx.send(Ok(b)).await.is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    }
                    last_seq = s.sequence;
                    fresh = true;
                }
                if playlist.ended {
                    return;
                }
                idle = if fresh { 0 } else { idle + 1 };
                let target = playlist.target_duration_secs.clamp(1, 30);
                if idle > STALL_TARGETS * 2 {
                    let _ = tx.send(Err(CaptureError::Stalled)).await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(target * 500)).await;
                if tx.is_closed() {
                    return;
                }
                match self
                    .get_bounded(&media_url, hls::MAX_PLAYLIST_BYTES)
                    .await
                    .and_then(|(_, b)| {
                        let text =
                            std::str::from_utf8(&b).map_err(|_| CaptureError::FormatUnsupported)?;
                        match hls::parse(text).map_err(CaptureError::from)? {
                            Playlist::Media(m) => Ok(m),
                            Playlist::Master(_) => Err(CaptureError::FormatUnsupported),
                        }
                    }) {
                    Ok(m) => playlist = m,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
        });
        Ok(Opened {
            format,
            first_pdt,
            chunks: rx,
            metadata: None,
        })
    }
}

fn content_type(res: &reqwest::Response) -> Option<String> {
    res.headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Token bucket over one-second windows.
struct RateCap {
    window: Instant,
    used: u64,
}

impl RateCap {
    fn new() -> Self {
        Self {
            window: Instant::now(),
            used: 0,
        }
    }

    async fn consume(&mut self, n: usize) {
        self.used += n as u64;
        if self.used > MAX_BYTES_PER_SEC {
            let elapsed = self.window.elapsed();
            if elapsed < Duration::from_secs(1) {
                tokio::time::sleep(Duration::from_secs(1) - elapsed).await;
            }
            self.window = Instant::now();
            self.used = 0;
        } else if self.window.elapsed() >= Duration::from_secs(1) {
            self.window = Instant::now();
            self.used = n as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_forms() {
        let u = Url::parse("https://radio.example/live").unwrap();
        let a = Auth::parse("X-Api-Key: abc", &u).unwrap();
        assert_eq!(a.name.as_str(), "x-api-key");
        assert_eq!(a.value.to_str().unwrap(), "abc");
        assert!(a.value.is_sensitive());
        let b = Auth::parse("Bearer abc:def", &u).unwrap();
        assert_eq!(b.name, reqwest::header::AUTHORIZATION);
        assert_eq!(b.value.to_str().unwrap(), "Bearer abc:def");
        assert!(Auth::parse("bad\nvalue", &u).is_none());
        assert_eq!(
            a.origin,
            ("https".into(), "radio.example".into(), Some(443))
        );
    }
}
