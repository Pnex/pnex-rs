//! RTSP capture of IP cameras (media-ingest.md D160, D175).
//!
//! The host is resolved here under the egress policy and the session is
//! opened on that address (the URL handed to the RTSP client carries the
//! IP literal: no second resolution, no DNS rebinding). The client
//! (`retina`) opens one TCP connection and never follows a redirect (any
//! non-2xx answer fails the session); every track is set up in TCP
//! interleaved mode (no UDP, whose destination the SDP would pick). The
//! credential comes from the stream's vault secret (`user:password`), is
//! only sent on that connection (the URL's origin) and never appears in an
//! argv or a URL. H.264/H.265 frames leave as Annex B (parameter sets on
//! every key frame), AAC as ADTS: both are piped to the confined decoder.

use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use pnex_core::egress::{self, EgressPolicy};
use reqwest::Url;
use retina::client::{
    Credentials, PlayOptions, Session, SessionOptions, SetupOptions, TcpTransportOptions, Transport,
};
use retina::codec::{CodecItem, FrameFormat};
use tokio::sync::mpsc;

use crate::decoder::{self, InputFormat};
use crate::{CaptureError, Track};

const DEFAULT_PORT: u16 = 554;
/// DNS, DESCRIBE, SETUP and PLAY each get this long.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);
/// No frame for this long = the camera is gone.
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);
const CHANNEL_DEPTH: usize = 32;

/// Tracks of an opened RTSP session (absent when the camera has none or
/// it was not asked for).
pub struct Opened {
    pub audio: Option<Track>,
    pub video: Option<Track>,
}

/// RTSP credential of a vault secret: `user:password`.
pub fn credentials(secret: &str) -> Option<Credentials> {
    let (username, password) = secret.trim().split_once(':')?;
    (!username.is_empty()).then(|| Credentials {
        username: username.to_string(),
        password: password.to_string(),
    })
}

/// `url` rewritten on the first address of its host allowed by `policy`
/// (R8). Every refusal is `Unreachable` (no scan oracle, D160).
pub async fn pin_address(url: &Url, policy: EgressPolicy) -> Result<Url, CaptureError> {
    if url.scheme() != "rtsp" {
        return Err(CaptureError::Unreachable);
    }
    let host = url.host_str().ok_or(CaptureError::Unreachable)?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let ip = match bare.parse::<std::net::IpAddr>() {
        Ok(ip) => ip,
        Err(_) => {
            if !egress::host_allowed(bare, policy) {
                return Err(CaptureError::Unreachable);
            }
            let port = url.port().unwrap_or(DEFAULT_PORT);
            let found = tokio::time::timeout(STEP_TIMEOUT, tokio::net::lookup_host((bare, port)))
                .await
                .map_err(|_| CaptureError::Unreachable)?
                .map_err(|_| CaptureError::Unreachable)?;
            egress::filter_addrs(found, policy)
                .first()
                .ok_or(CaptureError::Unreachable)?
                .ip()
        }
    };
    if !egress::ip_allowed(ip, policy) {
        return Err(CaptureError::Unreachable);
    }
    let mut pinned = url.clone();
    pinned
        .set_ip_host(ip)
        .map_err(|_| CaptureError::Unreachable)?;
    Ok(pinned)
}

/// Opens `url` under the process egress policy.
pub async fn open(
    url: &Url,
    creds: Option<Credentials>,
    want_audio: bool,
    want_video: bool,
) -> Result<Opened, CaptureError> {
    let pinned = pin_address(url, egress::policy()).await?;
    session_at(pinned, creds, want_audio, want_video).await
}

/// Session on an already pinned URL (tests use a local mock).
pub(crate) async fn session_at(
    url: Url,
    creds: Option<Credentials>,
    want_audio: bool,
    want_video: bool,
) -> Result<Opened, CaptureError> {
    let options = SessionOptions::default()
        .creds(creds)
        .user_agent(concat!("PNEX-media/", env!("CARGO_PKG_VERSION")).to_string());
    let mut session = step(Session::describe(url, options)).await?;
    let pick = |media: &str, names: &[&str]| {
        session
            .streams()
            .iter()
            .position(|s| s.media() == media && names.contains(&s.encoding_name()))
    };
    let video = want_video
        .then(|| pick("video", &["h264", "h265"]))
        .flatten();
    let audio = want_audio
        .then(|| pick("audio", &["mpeg4-generic"]))
        .flatten();
    let video_format = video.map(|i| match session.streams()[i].encoding_name() {
        "h265" => decoder::HEVC,
        _ => decoder::H264,
    });
    if video.is_none() && audio.is_none() {
        return Err(CaptureError::FormatUnsupported);
    }
    for i in video.iter().chain(audio.iter()) {
        let setup = SetupOptions::default()
            .transport(Transport::Tcp(TcpTransportOptions::default()))
            .frame_format(FrameFormat::SIMPLE);
        step(session.setup(*i, setup)).await?;
    }
    let playing = step(session.play(PlayOptions::default())).await?;
    let mut demuxed = playing
        .demuxed()
        .map_err(|_| CaptureError::FormatUnsupported)?;

    let track = |format: InputFormat| {
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        (tx, Track { format, chunks: rx })
    };
    let (video_tx, video_track) = video_format.map(track).unzip();
    let (audio_tx, audio_track) = audio.map(|_| track(decoder::ADTS)).unzip();
    tokio::spawn(async move {
        loop {
            let item = match tokio::time::timeout(FRAME_TIMEOUT, demuxed.next()).await {
                Ok(Some(Ok(item))) => item,
                Ok(None) => return,
                Ok(Some(Err(_))) | Err(_) => {
                    for tx in video_tx.iter().chain(audio_tx.iter()) {
                        let _ = tx.send(Err(CaptureError::Unreachable)).await;
                    }
                    return;
                }
            };
            let (tx, data) = match item {
                CodecItem::VideoFrame(f) if Some(f.stream_id()) == video => {
                    (video_tx.as_ref(), f.into_data())
                }
                CodecItem::AudioFrame(f) if Some(f.stream_id()) == audio => {
                    (audio_tx.as_ref(), f.data().to_vec())
                }
                _ => continue,
            };
            // A decoder that went away drops its track; the session ends
            // when no track is read any more.
            if let Some(tx) = tx {
                let _ = tx.send(Ok(Bytes::from(data))).await;
            }
            if video_tx
                .iter()
                .chain(audio_tx.iter())
                .all(|t| t.is_closed())
            {
                return;
            }
        }
    });
    Ok(Opened {
        audio: audio_track,
        video: video_track,
    })
}

/// One RTSP exchange, bounded; any failure (refused, redirected,
/// unauthorized, timeout) is `Unreachable`.
async fn step<T>(
    f: impl std::future::Future<Output = Result<T, retina::Error>>,
) -> Result<T, CaptureError> {
    match tokio::time::timeout(STEP_TIMEOUT, f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "rtsp session step failed");
            Err(CaptureError::Unreachable)
        }
        Err(_) => Err(CaptureError::Unreachable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn loopback_and_link_local_are_refused_in_lan() {
        for raw in [
            "rtsp://127.0.0.1/stream",
            "rtsp://[::1]:8554/stream",
            "rtsp://169.254.169.254/latest",
            "rtsp://localhost/stream",
            "rtsp://camera/stream",
        ] {
            let url = Url::parse(raw).unwrap();
            assert!(
                matches!(
                    pin_address(&url, EgressPolicy::Lan).await,
                    Err(CaptureError::Unreachable)
                ),
                "{raw}"
            );
        }
        // The LAN stays reachable in `lan`, not in `public`.
        let lan = Url::parse("rtsp://192.168.1.20:554/h264").unwrap();
        let pinned = pin_address(&lan, EgressPolicy::Lan).await.unwrap();
        assert_eq!(pinned.host_str(), Some("192.168.1.20"));
        assert!(pin_address(&lan, EgressPolicy::Public).await.is_err());
        // Never another scheme.
        let http = Url::parse("http://192.168.1.20/").unwrap();
        assert!(pin_address(&http, EgressPolicy::Lan).await.is_err());
    }

    #[test]
    fn credentials_from_secret() {
        let c = credentials("admin:p:w").unwrap();
        assert_eq!((c.username.as_str(), c.password.as_str()), ("admin", "p:w"));
        assert!(credentials("no-colon").is_none());
        assert!(credentials(":pw").is_none());
    }

    /// A mock camera answering every request with a redirect: the session
    /// fails, and the mock sees no second connection.
    #[tokio::test]
    async fn redirect_is_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = tokio::spawn(async move {
            let mut count = 0;
            while let Ok(Ok((mut sock, _))) =
                tokio::time::timeout(Duration::from_secs(3), listener.accept()).await
            {
                count += 1;
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let cseq = req
                    .lines()
                    .find_map(|l| l.strip_prefix("CSeq: "))
                    .unwrap_or("1")
                    .trim()
                    .to_string();
                let resp = format!(
                    "RTSP/1.0 302 Moved Temporarily\r\nCSeq: {cseq}\r\n\
                     Location: rtsp://10.0.0.1/elsewhere\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
            count
        });
        let url = Url::parse(&format!("rtsp://{addr}/stream")).unwrap();
        let res = session_at(url, None, true, true).await;
        assert!(matches!(res, Err(CaptureError::Unreachable)));
        assert_eq!(accepted.await.unwrap(), 1);
    }
}
