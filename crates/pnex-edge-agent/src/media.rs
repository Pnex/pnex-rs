//! Capture box (media-ingest.md D159, D160, lot 6b): the agent captures the
//! streams the server assigns to it (`capture_on = device:<id>`) with the
//! server's own chain ([`pnex_media_capture`]: filtered fetcher, confined
//! ffmpeg, segmenter) and uploads the segments over `/ws/media` (same TLS
//! client certificate, token and Noise key as `/ws/device`).
//!
//! No local durable file: segments wait in a bounded in-memory queue, the
//! oldest unsent ones are lost when it is full (counted in
//! `media_segments_dropped`). The box holds no storage credential; the
//! secret of a stream arrives with its stream and lives in memory only.
//! Egress: `PNEX_EGRESS` of the box (`lan` by default: Tvheadend and IP
//! cameras of the LAN are reachable, loopback and link-local are not).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use pnex_core::frame::{Initiator, Link as NoiseLink};
use pnex_core::media_ingest::{
    encode_segment, CaptureState, DeviceStream, MediaDownMsg, MediaStreamKind, MediaUpMsg,
    SegmentHeader,
};
use pnex_media_capture::{
    decoder, CaptureError, CaptureSettings, Host, RunEnd, Segment, Spec, BACKOFF_MAX, BACKOFF_MIN,
    HEALTHY_RUN,
};
use tokio::sync::{mpsc, watch, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::config::{Config, Secrets};
use crate::state::Shared;
use crate::uplink::{authorized_request, close_end, rand_u16, tls_config, End};

/// Segments waiting for the link (30 s of 16 kHz PCM ≈ 1 MB each).
const QUEUE_SEGMENTS: usize = 32;
/// Keepalive (the server closes a silent link after 45 s).
const PING_EVERY: Duration = Duration::from_secs(20);
/// An unacknowledged segment older than this restarts the link.
const ACK_TIMEOUT: Duration = Duration::from_secs(60);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const LINK_BACKOFF_MIN: Duration = Duration::from_secs(1);
const LINK_BACKOFF_MAX: Duration = Duration::from_secs(60);
const REVOKED_BACKOFF: Duration = Duration::from_secs(300);

/// The ffmpeg this box captures with, when it may be a capture box:
/// enabled in `config.toml`, Linux (the decoder confinement is Linux
/// only), `PNEX_MEDIA_CAPTURE` not off, ffmpeg found.
pub fn available(cfg: &Config) -> Option<PathBuf> {
    if !cfg.media_capture {
        return None;
    }
    if !cfg!(target_os = "linux") {
        tracing::warn!("media capture needs Linux (decoder confinement): disabled");
        return None;
    }
    let settings = CaptureSettings::from_env();
    if !settings.enabled {
        return None;
    }
    let found = decoder::resolve(&settings.ffmpeg);
    if found.is_none() {
        tracing::warn!(ffmpeg = %settings.ffmpeg, "ffmpeg not found: media capture disabled");
    }
    found
}

/// Last capture state of each stream, re-sent on every (re)connection.
#[derive(Default)]
struct StateBook {
    states: Mutex<HashMap<String, (CaptureState, Option<String>)>>,
    changed: Notify,
}

impl StateBook {
    fn set(&self, id: &str, state: CaptureState, error: Option<CaptureError>) {
        let error = error.map(|e| e.code().to_string());
        self.states
            .lock()
            .expect("states")
            .insert(id.to_string(), (state, error));
        self.changed.notify_one();
    }

    fn messages(&self) -> Vec<MediaUpMsg> {
        self.states
            .lock()
            .expect("states")
            .iter()
            .map(|(id, (state, error))| MediaUpMsg::State {
                stream_id: id.clone(),
                state: *state,
                error: error.clone(),
            })
            .collect()
    }
}

/// What every capture task shares.
struct Ctx {
    shared: Arc<Shared>,
    settings: CaptureSettings,
    ffmpeg: PathBuf,
    queue: mpsc::Sender<Vec<u8>>,
    book: StateBook,
}

/// Carrier side of a box run: segments into the upload queue.
struct BoxHost {
    ctx: Arc<Ctx>,
    stream_id: String,
}

impl Host for BoxHost {
    async fn running(&mut self) {
        self.ctx
            .book
            .set(&self.stream_id, CaptureState::Running, None);
    }

    async fn segment(&mut self, seg: Segment) -> Result<(), CaptureError> {
        let header = SegmentHeader {
            stream_id: self.stream_id.clone(),
            seq: seg.seq,
            started_ms: seg.started_at.timestamp_millis(),
            ended_ms: seg.ended_at.timestamp_millis(),
            clock: seg.clock_source,
        };
        if self
            .ctx
            .queue
            .try_send(encode_segment(&header, &seg.wav))
            .is_err()
        {
            let n = self
                .ctx
                .shared
                .media_dropped
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            if n.is_power_of_two() {
                tracing::warn!(dropped = n, "media upload queue full: segment dropped");
            }
        }
        Ok(())
    }

    // A box captures audio only (the camera bus lives on the server).
    async fn frame(&mut self, _frame: pnex_media_capture::video::Jpeg) {}

    // In-band titles are not forwarded by a box (lot 6b).
    fn metadata(&mut self, _events: mpsc::Receiver<pnex_media_capture::fetch::MetadataEvent>) {}
}

/// Capture task of one stream: runs the chain, restarts it with the
/// server's backoff, reports its state.
async fn supervise(ctx: Arc<Ctx>, stream: DeviceStream, mut stop: watch::Receiver<bool>) {
    let id = stream.id.clone();
    let url = reqwest::Url::parse(&stream.url)
        .ok()
        .filter(|u| stream.kind.scheme_ok(u.scheme()) && u.host_str().is_some());
    let mut backoff = BACKOFF_MIN;
    // Sequence from the clock: a restart never reuses a stored number.
    let mut seq = chrono::Utc::now().timestamp();
    let mut host = BoxHost {
        ctx: ctx.clone(),
        stream_id: id.clone(),
    };
    loop {
        ctx.book.set(&id, CaptureState::Starting, None);
        let started = Instant::now();
        let result = match &url {
            None => Err(CaptureError::Unreachable),
            Some(url) => {
                let spec = Spec {
                    slug: &stream.slug,
                    kind: stream.kind,
                    url,
                    secret: stream.auth_secret.as_deref(),
                    want_audio: true,
                    want_video: false,
                    fps: 1,
                    segment_secs: stream.segment_secs.max(1) as u32,
                    overlap_secs: stream.overlap_secs.max(0) as u32,
                    metadata: false,
                };
                pnex_media_capture::run_once(
                    &mut host,
                    &ctx.settings,
                    &ctx.ffmpeg,
                    &spec,
                    &mut seq,
                    stop.clone(),
                )
                .await
            }
        };
        match result {
            Ok(RunEnd::Stopped) => return,
            // A file is captured once; the task idles until reassigned.
            Ok(RunEnd::Finished) if stream.kind == MediaStreamKind::HttpFile => {
                ctx.book.set(&id, CaptureState::Stopped, None);
                let _ = stop.changed().await;
                return;
            }
            Ok(RunEnd::Finished) => {
                ctx.book
                    .set(&id, CaptureState::Backoff, Some(CaptureError::Stalled));
            }
            Err(e) => {
                tracing::info!(stream = %stream.slug, error = e.code(), "media capture run ended");
                ctx.book.set(&id, CaptureState::Backoff, Some(e));
            }
        }
        if started.elapsed() >= HEALTHY_RUN {
            backoff = BACKOFF_MIN;
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = stop.changed() => return,
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

struct Task {
    spec: DeviceStream,
    stop: watch::Sender<bool>,
}

/// Applies a stream list: changed or removed streams stop, new enabled
/// ones start, unchanged ones keep running.
fn reconcile(ctx: &Arc<Ctx>, tasks: &mut HashMap<String, Task>, streams: Vec<DeviceStream>) {
    let listed: std::collections::HashSet<String> = streams.iter().map(|s| s.id.clone()).collect();
    let wanted: HashMap<String, DeviceStream> = streams
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id.clone(), s))
        .collect();
    tasks.retain(|id, t| {
        let keep = wanted.get(id) == Some(&t.spec);
        if !keep {
            let _ = t.stop.send(true);
            ctx.book.set(id, CaptureState::Stopped, None);
        }
        keep
    });
    for (id, spec) in wanted {
        if tasks.contains_key(&id) {
            continue;
        }
        let (stop, stop_rx) = watch::channel(false);
        tokio::spawn(supervise(ctx.clone(), spec.clone(), stop_rx));
        tasks.insert(id, Task { spec, stop });
    }
    // Streams no longer assigned to this box are forgotten.
    ctx.book
        .states
        .lock()
        .expect("states")
        .retain(|id, _| listed.contains(id));
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Runs the capture box until the process stops: link to `/ws/media`
/// (reconnects forever), capture tasks, upload queue.
pub async fn run(
    shared: Arc<Shared>,
    cfg: Config,
    secrets: Secrets,
    ca_pem: Option<String>,
    ffmpeg: PathBuf,
) {
    let Some(key) = pnex_core::frame::decode_key(&secrets.encryption_key) else {
        tracing::error!("media link: invalid encryption key in secrets.json");
        return;
    };
    let client = (
        secrets.client_cert_pem.as_str(),
        secrets.client_key_pem.as_str(),
    );
    let tls = match (cfg.https_server(), tls_config(ca_pem.as_deref(), client)) {
        (Ok(_), Ok(tls)) => tls,
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("media link: {e}");
            return;
        }
    };
    let url = format!("wss://{}/ws/media", secrets.device_host.trim());
    let (queue, mut segments) = mpsc::channel(QUEUE_SEGMENTS);
    let ctx = Arc::new(Ctx {
        shared: shared.clone(),
        settings: CaptureSettings::from_env(),
        ffmpeg,
        queue,
        book: StateBook::default(),
    });
    let mut tasks: HashMap<String, Task> = HashMap::new();
    // A segment sent but not acknowledged when the link dropped: resent.
    let mut pending: Option<Vec<u8>> = None;
    let mut backoff = LINK_BACKOFF_MIN;
    while !shared.shutting_down.load(Ordering::Relaxed) {
        let request = match authorized_request(&url, &secrets.token) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("media link: {e}");
                return;
            }
        };
        let connector = tokio_tungstenite::Connector::Rustls(tls.clone());
        let connected = tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector)),
        )
        .await;
        let end = match connected {
            Ok(Ok((ws, _))) => {
                backoff = LINK_BACKOFF_MIN;
                let mut link = Session {
                    ctx: &ctx,
                    tasks: &mut tasks,
                    segments: &mut segments,
                    pending: &mut pending,
                };
                link.run(ws, &key, &secrets.device_id).await
            }
            Ok(Err(e)) => End::Retry(format!("connect failed: {e}")),
            Err(_) => End::Retry("connect timeout".into()),
        };
        let wait = match end {
            End::Retry(why) => {
                tracing::warn!("media link down: {why}");
                let wait = backoff + Duration::from_millis(u64::from(rand_u16()) % 1000);
                backoff = (backoff * 2).min(LINK_BACKOFF_MAX);
                wait
            }
            End::Revoked(why) => {
                tracing::error!("media link refused: {why}");
                REVOKED_BACKOFF
            }
        };
        tokio::time::sleep(wait).await;
    }
    for t in tasks.values() {
        let _ = t.stop.send(true);
    }
}

struct Session<'a> {
    ctx: &'a Arc<Ctx>,
    tasks: &'a mut HashMap<String, Task>,
    segments: &'a mut mpsc::Receiver<Vec<u8>>,
    pending: &'a mut Option<Vec<u8>>,
}

impl Session<'_> {
    async fn run(&mut self, ws: Ws, key: &[u8; 32], device_id: &str) -> End {
        let (mut sink, mut stream) = ws.split();
        // Noise handshake in raw bytes (D156, school of `/ws/camera`).
        let (init, msg1) = Initiator::start(key, device_id);
        if let Err(e) = sink.send(Message::Binary(msg1.into())).await {
            return End::Retry(format!("send failed: {e}"));
        }
        let answer = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.next()).await;
        let mut codec = match answer {
            Ok(Some(Ok(Message::Binary(msg2)))) => match init.finish(&msg2) {
                Some(link) => link,
                None => return End::Revoked("Noise handshake failed".into()),
            },
            Ok(Some(Ok(Message::Close(Some(f))))) => return close_end(f.code, &f.reason),
            _ => return End::Retry("no handshake answer from the server".into()),
        };
        tracing::info!("media link up");
        for msg in self.ctx.book.messages() {
            if let Err(e) = send_text(&mut sink, &mut codec, &msg).await {
                return End::Retry(e);
            }
        }
        // Sent, not yet acknowledged (stop-and-wait: segments are seconds
        // apart, one in flight is enough).
        let mut inflight: Option<(Vec<u8>, Instant)> = None;
        if let Some(frame) = self.pending.take() {
            if let Err(e) = send_bin(&mut sink, &mut codec, &frame).await {
                *self.pending = Some(frame);
                return End::Retry(e);
            }
            inflight = Some((frame, Instant::now()));
        }
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        let end = loop {
            tokio::select! {
                incoming = stream.next() => {
                    let msg = match incoming {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => break End::Retry(format!("read failed: {e}")),
                        None => break End::Retry("connection closed".into()),
                    };
                    match msg {
                        Message::Text(text) => {
                            let Some(plain) = codec.open_text(text.as_str()) else {
                                break End::Retry("unauthenticated server frame".into());
                            };
                            match serde_json::from_str::<MediaDownMsg>(&plain) {
                                Ok(MediaDownMsg::Streams { streams }) => {
                                    reconcile(self.ctx, self.tasks, streams);
                                }
                                Ok(MediaDownMsg::Ack { ok, code, .. }) => {
                                    inflight = None;
                                    if ok {
                                        self.ctx.shared.media_sent.fetch_add(1, Ordering::Relaxed);
                                    } else {
                                        self.ctx.shared.media_dropped.fetch_add(1, Ordering::Relaxed);
                                        tracing::warn!(code = code.as_deref().unwrap_or(""), "media segment refused by the server");
                                    }
                                }
                                Err(_) => {} // PONG
                            }
                        }
                        Message::Close(frame) => {
                            break match frame {
                                Some(f) => close_end(f.code, &f.reason),
                                None => End::Retry("closed by server".into()),
                            };
                        }
                        _ => {}
                    }
                }
                next = self.segments.recv(), if inflight.is_none() => {
                    let Some(frame) = next else { break End::Retry("upload queue closed".into()) };
                    if let Err(e) = send_bin(&mut sink, &mut codec, &frame).await {
                        *self.pending = Some(frame);
                        return End::Retry(e);
                    }
                    inflight = Some((frame, Instant::now()));
                }
                _ = self.ctx.book.changed.notified() => {
                    for msg in self.ctx.book.messages() {
                        if let Err(e) = send_text(&mut sink, &mut codec, &msg).await {
                            tracing::debug!("media link: state not sent: {e}");
                            break;
                        }
                    }
                }
                _ = ping.tick() => {
                    if inflight.as_ref().is_some_and(|(_, at)| at.elapsed() > ACK_TIMEOUT) {
                        break End::Retry("acknowledgement timeout".into());
                    }
                    if let Err(e) = sink.send(Message::Text(codec.seal_text("PING").into())).await {
                        break End::Retry(format!("ping failed: {e}"));
                    }
                }
            }
        };
        if let Some((frame, _)) = inflight {
            *self.pending = Some(frame);
        }
        end
    }
}

async fn send_text(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    codec: &mut NoiseLink,
    msg: &MediaUpMsg,
) -> Result<(), String> {
    let plain = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    sink.send(Message::Text(codec.seal_text(&plain).into()))
        .await
        .map_err(|e| format!("send failed: {e}"))
}

async fn send_bin(
    sink: &mut futures_util::stream::SplitSink<Ws, Message>,
    codec: &mut NoiseLink,
    frame: &[u8],
) -> Result<(), String> {
    sink.send(Message::Binary(codec.seal(frame).into()))
        .await
        .map_err(|e| format!("send failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(id: &str, enabled: bool) -> DeviceStream {
        DeviceStream {
            id: id.into(),
            slug: id.into(),
            kind: MediaStreamKind::Icecast,
            url: "http://192.0.2.1:9/none".into(),
            segment_secs: 10,
            overlap_secs: 1,
            tracks: Default::default(),
            fps: 1,
            enabled,
            auth_secret: None,
        }
    }

    #[tokio::test]
    async fn reconcile_starts_changed_and_stops_removed_streams() {
        let dir = tempfile::tempdir().unwrap();
        let queue = crate::queue::Queue::open(&dir.path().join("q.redb")).unwrap();
        let (tx, _rx) = mpsc::channel(1);
        let (seg_tx, _seg_rx) = mpsc::channel(1);
        let ctx = Arc::new(Ctx {
            shared: Shared::new(queue, tx),
            settings: CaptureSettings::from_env(),
            ffmpeg: PathBuf::from("/nonexistent/ffmpeg"),
            queue: seg_tx,
            book: StateBook::default(),
        });
        let mut tasks = HashMap::new();
        reconcile(
            &ctx,
            &mut tasks,
            vec![stream("a", true), stream("b", false)],
        );
        assert_eq!(tasks.len(), 1, "disabled streams do not run");
        let mut a2 = stream("a", true);
        a2.segment_secs = 20;
        reconcile(&ctx, &mut tasks, vec![a2.clone(), stream("c", true)]);
        assert_eq!(tasks["a"].spec, a2, "a changed spec restarts the task");
        assert!(tasks.contains_key("c"));
        reconcile(&ctx, &mut tasks, vec![]);
        assert!(tasks.is_empty());
    }
}
