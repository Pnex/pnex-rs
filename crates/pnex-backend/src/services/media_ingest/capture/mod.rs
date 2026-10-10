//! Capture of `capture_on = server | worker` streams (media-ingest.md D160).
//!
//! One supervisor per process scans the enabled streams of its carrier
//! every [`SCAN_EVERY`]; each stream gets one capture task, guarded by the
//! lease `task:media-capture:<id>` so a single process captures it. A task
//! runs fetcher → confined ffmpeg → segmenter → [`Sink`], and restarts
//! with a bounded exponential backoff; one stream failing never touches
//! the others. Config changes (`updated_at`) restart the task.
//!
//! The `server` carrier writes segments itself; a mesh `worker` posts them
//! to the control plane (`POST /internal/media/segment`), so it holds no
//! storage write credential. `device:<id>` streams are never scanned here:
//! their capture box runs the same chain ([`pnex_media_capture`]) and
//! uploads over `/ws/media` (lot 6b).

pub mod video;

pub use pnex_media_capture::{
    decode_clip, decoder, fetch, hls, icy, rtsp, sandbox, segmenter, CaptureError, CaptureSettings,
    RunEnd, Track,
};

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::media_ingest::{
    CaptureOn, CaptureState, MediaStreamKind, MediaTracks, FPS_MAX, FPS_MIN,
};
use pnex_media_capture::{Host, Spec, BACKOFF_MAX, BACKOFF_MIN, HEALTHY_RUN};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tokio::sync::watch;
use uuid::Uuid;

use super::segments::{self, NewSegment};
use crate::models::_entities::{media_segments, media_streams};
use crate::services::media::{MediaSettings, MediaStore};
use crate::services::secrets::{store as secret_store, Keyring};

/// Period of the supervisor scan (and lease renewal).
const SCAN_EVERY: Duration = Duration::from_secs(10);

/// Where the segments of a capture go.
#[derive(Clone)]
pub enum Sink {
    /// `server` carrier: blob and row written by this process.
    Direct(Arc<dyn MediaStore>),
    /// `worker` carrier: segments posted to the control plane.
    Remote(Arc<RemoteSink>),
    /// URL test: segments handed to the caller, nothing stored.
    Probe(tokio::sync::mpsc::Sender<NewSegment>),
}

/// Upload target of a mesh worker (`PNEX_MEDIA_SEGMENT_URL` + the service
/// token of the internal endpoints).
pub struct RemoteSink {
    client: reqwest::Client,
    url: String,
    token: String,
}

/// Header of the internal service token (shared with `/internal/flow/*`).
pub const SERVICE_TOKEN_HEADER: &str = "x-pnex-flow-token";

impl RemoteSink {
    /// `None` when the URL or the token is not configured.
    pub fn from_config(config: &loco_rs::config::Config) -> Option<Self> {
        let url = std::env::var("PNEX_MEDIA_SEGMENT_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())?;
        let (_, token) = crate::services::flow::FlowSettings::from_config(config).device_write?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .ok()?;
        Some(Self { client, url, token })
    }
}

/// Starts the capture supervisor of the server carrier (never in the
/// `ForegroundBlocking` test mode).
pub fn spawn_supervisor(ctx: &AppContext) {
    let store = match MediaSettings::from_config(&ctx.config).store() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "media capture: media store unavailable");
            return;
        }
    };
    spawn(ctx, CaptureOn::Server, Sink::Direct(store));
}

/// Starts the capture supervisor of a mesh worker (`--worker` process with
/// `PNEX_MEDIA_CAPTURE_WORKER=1`): captures the `capture_on = worker`
/// streams and posts their segments to `PNEX_MEDIA_SEGMENT_URL`.
pub fn spawn_worker_supervisor(ctx: &AppContext) {
    if !env_flag("PNEX_MEDIA_CAPTURE_WORKER") {
        return;
    }
    let Some(remote) = RemoteSink::from_config(&ctx.config) else {
        tracing::error!(
            "worker media capture needs PNEX_MEDIA_SEGMENT_URL and PNEX_FLOW_RUNTIME_TOKEN"
        );
        return;
    };
    spawn(ctx, CaptureOn::Worker, Sink::Remote(Arc::new(remote)));
}

fn env_flag(key: &str) -> bool {
    matches!(
        std::env::var(key)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "on" | "yes"
    )
}

fn spawn(ctx: &AppContext, carrier: CaptureOn, sink: Sink) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let settings = CaptureSettings::from_env();
    if !settings.enabled {
        tracing::info!("media capture disabled (PNEX_MEDIA_CAPTURE)");
        return;
    }
    let ffmpeg = decoder::resolve(&settings.ffmpeg);
    if ffmpeg.is_none() {
        tracing::warn!(ffmpeg = %settings.ffmpeg, "ffmpeg not found: media streams will not be captured");
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tasks: HashMap<Uuid, Running> = HashMap::new();
        let mut tick = tokio::time::interval(SCAN_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = scan(&ctx, &settings, carrier, &sink, ffmpeg.as_ref(), &mut tasks).await
            {
                tracing::warn!(error = %e, "media capture scan failed");
            }
        }
    });
}

struct Running {
    fingerprint: chrono::DateTime<chrono::FixedOffset>,
    stop: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

fn lease_name(id: Uuid) -> String {
    format!("media-capture:{id}")
}

async fn scan(
    ctx: &AppContext,
    settings: &CaptureSettings,
    carrier: CaptureOn,
    sink: &Sink,
    ffmpeg: Option<&PathBuf>,
    tasks: &mut HashMap<Uuid, Running>,
) -> Result<(), sea_orm::DbErr> {
    let wanted: HashMap<Uuid, media_streams::Model> = media_streams::Entity::find()
        .filter(media_streams::Column::Enabled.eq(true))
        .filter(media_streams::Column::CaptureOn.eq(carrier.wire()))
        .filter(media_streams::Column::DeletedAt.is_null())
        // No profile, no audio capture: the audio would never be
        // transcribed. A stream with video runs without one (D175).
        .filter(
            Condition::any()
                .add(media_streams::Column::AsrProfileId.is_not_null())
                .add(media_streams::Column::Tracks.ne(MediaTracks::Audio.wire())),
        )
        .all(&ctx.db)
        .await?
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
    // Stop what is no longer wanted, changed, finished, or lost its lease.
    let ids: Vec<Uuid> = tasks.keys().copied().collect();
    for id in ids {
        let keep = match wanted.get(&id) {
            Some(s) => {
                let t = &tasks[&id];
                t.fingerprint == s.updated_at
                    && !t.handle.is_finished()
                    && crate::services::singleton::my_turn(&ctx.db, &lease_name(id), SCAN_EVERY)
                        .await
            }
            None => false,
        };
        if !keep {
            if let Some(t) = tasks.remove(&id) {
                let _ = t.stop.send(true);
                let _ = tokio::time::timeout(Duration::from_secs(5), t.handle).await;
            }
            if !wanted.contains_key(&id) {
                set_state(&ctx.db, id, CaptureState::Stopped, None).await;
            }
        }
    }
    let Some(ffmpeg) = ffmpeg else {
        return Ok(());
    };
    for (id, stream) in wanted {
        if tasks.contains_key(&id) {
            continue;
        }
        if !crate::services::singleton::my_turn(&ctx.db, &lease_name(id), SCAN_EVERY).await {
            continue;
        }
        let (stop, stop_rx) = watch::channel(false);
        let fingerprint = stream.updated_at;
        let handle = tokio::spawn(run_stream(
            ctx.clone(),
            settings.clone(),
            ffmpeg.clone(),
            sink.clone(),
            stream,
            stop_rx,
        ));
        tasks.insert(
            id,
            Running {
                fingerprint,
                stop,
                handle,
            },
        );
    }
    Ok(())
}

/// Writes the capture state; never touches `updated_at` (the config
/// fingerprint of the supervisor).
async fn set_state(
    db: &DatabaseConnection,
    id: Uuid,
    state: CaptureState,
    error: Option<CaptureError>,
) {
    let res = media_streams::Entity::update_many()
        .col_expr(
            media_streams::Column::CaptureState,
            Expr::value(state.wire()),
        )
        .col_expr(
            media_streams::Column::CaptureError,
            Expr::value(error.map(|e| e.code().to_string())),
        )
        .col_expr(
            media_streams::Column::CaptureChangedAt,
            Expr::value(Some(sea_orm::prelude::DateTimeWithTimeZone::from(
                Utc::now(),
            ))),
        )
        .filter(media_streams::Column::Id.eq(id))
        .exec(db)
        .await;
    if let Err(e) = res {
        tracing::warn!(stream = %id, error = %e, "media capture state not saved");
    }
}

async fn run_stream(
    ctx: AppContext,
    settings: CaptureSettings,
    ffmpeg: PathBuf,
    sink: Sink,
    stream: media_streams::Model,
    mut stop: watch::Receiver<bool>,
) {
    let mut backoff = BACKOFF_MIN;
    // Sequence numbers continue across restarts of the process.
    let mut seq = media_segments::Entity::find()
        .filter(media_segments::Column::StreamId.eq(stream.id))
        .order_by_desc(media_segments::Column::Seq)
        .one(&ctx.db)
        .await
        .ok()
        .flatten()
        .map_or(0, |s| s.seq + 1);
    loop {
        set_state(&ctx.db, stream.id, CaptureState::Starting, None).await;
        let started = Instant::now();
        let result = run_once(
            &ctx,
            &settings,
            &ffmpeg,
            &sink,
            &stream,
            &mut seq,
            stop.clone(),
        )
        .await;
        match result {
            Ok(RunEnd::Stopped) => return,
            Ok(RunEnd::Finished) => {
                if stream.kind == MediaStreamKind::HttpFile.wire() {
                    // A file is captured once: the stream switches itself off.
                    let _ = media_streams::Entity::update_many()
                        .col_expr(media_streams::Column::Enabled, Expr::value(false))
                        .filter(media_streams::Column::Id.eq(stream.id))
                        .exec(&ctx.db)
                        .await;
                    set_state(&ctx.db, stream.id, CaptureState::Stopped, None).await;
                    return;
                }
                set_state(
                    &ctx.db,
                    stream.id,
                    CaptureState::Backoff,
                    Some(CaptureError::Stalled),
                )
                .await;
            }
            Err(e) => {
                tracing::info!(stream = %stream.slug, org = stream.org_id, error = e.code(), "media capture run ended");
                set_state(&ctx.db, stream.id, CaptureState::Backoff, Some(e)).await;
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

/// Captures `stream` once, to its end (finite sources) or until `limit`
/// elapses; returns the number of segments stored. Used by tests and by
/// the stream URL check, never by the supervisor.
pub async fn capture_once(
    ctx: &AppContext,
    stream: &media_streams::Model,
    limit: Duration,
) -> Result<i64, CaptureError> {
    let settings = CaptureSettings::from_env();
    let ffmpeg = decoder::resolve(&settings.ffmpeg).ok_or(CaptureError::DecoderMissing)?;
    let sink = Sink::Direct(
        MediaSettings::from_config(&ctx.config)
            .store()
            .map_err(|_| CaptureError::StoreFailed)?,
    );
    let (stop_tx, stop) = watch::channel(false);
    let mut seq = 0i64;
    let timer = tokio::spawn(async move {
        tokio::time::sleep(limit).await;
        let _ = stop_tx.send(true);
    });
    let res = run_once(ctx, &settings, &ffmpeg, &sink, stream, &mut seq, stop).await;
    timer.abort();
    res.map(|_| seq)
}

/// Length of the extract captured by a stream test.
pub const PROBE_SEGMENT_SECS: i32 = 10;

/// Captures the first [`PROBE_SEGMENT_SECS`] of `stream` without storing
/// anything (stream test, §7), within `limit`.
pub async fn probe(
    ctx: &AppContext,
    stream: &media_streams::Model,
    limit: Duration,
) -> Result<NewSegment, CaptureError> {
    let settings = CaptureSettings::from_env();
    let ffmpeg = decoder::resolve(&settings.ffmpeg).ok_or(CaptureError::DecoderMissing)?;
    let mut stream = stream.clone();
    // A test checks the audio path only (D175: frames are never tested).
    stream.tracks = MediaTracks::Audio.wire().to_string();
    stream.segment_secs = PROBE_SEGMENT_SECS;
    stream.overlap_secs = 0;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let sink = Sink::Probe(tx);
    let (stop_tx, stop) = watch::channel(false);
    let ctx2 = ctx.clone();
    let run = tokio::spawn(async move {
        let mut seq = 0i64;
        run_once(&ctx2, &settings, &ffmpeg, &sink, &stream, &mut seq, stop).await
    });
    let outcome = tokio::time::timeout(limit, rx.recv()).await;
    let _ = stop_tx.send(true);
    match outcome {
        Ok(Some(seg)) => {
            run.abort();
            Ok(seg)
        }
        // The capture ended before a full extract: its error says why.
        Ok(None) => match run.await {
            Ok(Err(e)) => Err(e),
            _ => Err(CaptureError::Stalled),
        },
        Err(_) => {
            run.abort();
            Err(CaptureError::Stalled)
        }
    }
}

/// Vault secret of the stream, revealed in memory only.
async fn secret_of(
    ctx: &AppContext,
    stream: &media_streams::Model,
) -> Result<Option<String>, CaptureError> {
    let Some(secret) = stream.secret_id else {
        return Ok(None);
    };
    let ring = Keyring::from_config(&ctx.config).map_err(|_| CaptureError::SecretUnreadable)?;
    secret_store::reveal(&ctx.db, &ring, Some(stream.org_id), secret)
        .await
        .map(Some)
        .map_err(|_| CaptureError::SecretUnreadable)
}

/// Carrier side of a server/worker run: segments to the [`Sink`], frames
/// to the stream's camera bus, metadata to O2, state to the row.
struct BackendHost<'a> {
    ctx: &'a AppContext,
    sink: &'a Sink,
    stream: &'a media_streams::Model,
    probe: bool,
    /// Valkey of the frames, connected on the first frame.
    valkey: Option<Option<redis::aio::ConnectionManager>>,
    /// Frame sequence from the clock: a restart never reuses a live key.
    frame_seq: u32,
}

impl Host for BackendHost<'_> {
    async fn running(&mut self) {
        // A test never touches the state of the stream's own capture.
        if !self.probe {
            set_state(&self.ctx.db, self.stream.id, CaptureState::Running, None).await;
        }
    }

    async fn segment(&mut self, seg: NewSegment) -> Result<(), CaptureError> {
        store_segment(self.ctx, self.sink, self.stream, seg).await
    }

    async fn frame(&mut self, frame: video::Jpeg) {
        if self.valkey.is_none() {
            let conn = crate::services::shared_valkey::conn(&self.ctx.config).await;
            if conn.is_none() {
                tracing::warn!(stream = %self.stream.slug, "valkey not configured: video frames are not published");
            }
            self.valkey = Some(conn);
        }
        let (org, slug, seq) = (self.stream.org_id, &self.stream.slug, self.frame_seq);
        if let Some(Some(conn)) = self.valkey.as_mut() {
            if let Err(e) = video::publish(conn, org, slug, seq, &frame).await {
                tracing::debug!(stream = %slug, error = %e, "video frame not published");
            }
        }
        self.frame_seq = self.frame_seq.wrapping_add(1);
    }

    fn metadata(&mut self, mut events: tokio::sync::mpsc::Receiver<fetch::MetadataEvent>) {
        // In-band metadata → O2 `mx_<slug>` (D170). The task ends with the
        // fetcher (its sender is dropped).
        let (ctx, slug, org_id) = (
            self.ctx.clone(),
            self.stream.slug.clone(),
            self.stream.org_id,
        );
        tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                if let Err(e) = super::metadata::write(&ctx, org_id, &slug, &ev).await {
                    tracing::warn!(stream = %slug, error = %e, "media metadata event not written");
                }
            }
        });
    }
}

async fn run_once(
    ctx: &AppContext,
    settings: &CaptureSettings,
    ffmpeg: &std::path::Path,
    sink: &Sink,
    stream: &media_streams::Model,
    seq: &mut i64,
    stop: watch::Receiver<bool>,
) -> Result<RunEnd, CaptureError> {
    let url = super::streams::clean_url(&stream.url).map_err(|_| CaptureError::Unreachable)?;
    let kind = MediaStreamKind::from_wire(&stream.kind).ok_or(CaptureError::FormatUnsupported)?;
    let tracks = MediaTracks::from_wire(&stream.tracks).unwrap_or_default();
    let probe = matches!(sink, Sink::Probe(_));
    let secret = secret_of(ctx, stream).await?;
    // A stream with video captures its audio only when it gets transcribed
    // (an audio-only stream without profile is never scanned); a test
    // never publishes frames nor stores metadata (D175).
    let spec = Spec {
        slug: &stream.slug,
        kind,
        url: &url,
        secret: secret.as_deref(),
        want_audio: tracks.has_audio()
            && (tracks == MediaTracks::Audio || probe || stream.asr_profile_id.is_some()),
        want_video: tracks.has_video() && !probe,
        fps: stream.fps.clamp(FPS_MIN, FPS_MAX) as u32,
        segment_secs: stream.segment_secs.max(1) as u32,
        overlap_secs: stream.overlap_secs.max(0) as u32,
        metadata: !probe,
    };
    let mut host = BackendHost {
        ctx,
        sink,
        stream,
        probe,
        valkey: None,
        frame_seq: Utc::now().timestamp_millis() as u32,
    };
    pnex_media_capture::run_once(&mut host, settings, ffmpeg, &spec, seq, stop).await
}

async fn store_segment(
    ctx: &AppContext,
    sink: &Sink,
    stream: &media_streams::Model,
    seg: NewSegment,
) -> Result<(), CaptureError> {
    let store = match sink {
        Sink::Direct(store) => store,
        Sink::Remote(remote) => return post_segment(remote, stream, seg).await,
        Sink::Probe(tx) => {
            let _ = tx.try_send(seg);
            return Ok(());
        }
    };
    let row = segments::write(&ctx.db, store, stream, seg)
        .await
        .map_err(|e| {
            tracing::warn!(stream = %stream.slug, error = %e, "media segment not stored");
            CaptureError::StoreFailed
        })?;
    if stream.asr_profile_id.is_some() {
        segments::enqueue(ctx, &row, true).await;
    }
    Ok(())
}
/// Posts a segment to the control plane (`POST /internal/media/segment`).
async fn post_segment(
    remote: &RemoteSink,
    stream: &media_streams::Model,
    seg: NewSegment,
) -> Result<(), CaptureError> {
    let res = remote
        .client
        .post(&remote.url)
        .header(SERVICE_TOKEN_HEADER, &remote.token)
        .query(&[
            ("stream_id", stream.id.to_string()),
            ("org_id", stream.org_id.to_string()),
            ("seq", seg.seq.to_string()),
            ("started_ms", seg.started_at.timestamp_millis().to_string()),
            ("ended_ms", seg.ended_at.timestamp_millis().to_string()),
            ("clock", seg.clock_source.wire().to_string()),
        ])
        .body(seg.wav)
        .send()
        .await;
    match res {
        Ok(r) if r.status().is_success() => Ok(()),
        Ok(r) => {
            tracing::warn!(stream = %stream.slug, status = %r.status(), "media segment upload refused");
            Err(CaptureError::StoreFailed)
        }
        Err(e) => {
            tracing::warn!(stream = %stream.slug, error = %e, "media segment upload failed");
            Err(CaptureError::StoreFailed)
        }
    }
}

#[cfg(test)]
mod live_tests {
    //! Real streams, real ffmpeg: `cargo test -- --ignored live_` with
    //! network access and `PNEX_FFMPEG` (or ffmpeg in `PATH`).
    use super::*;
    use fetch::Fetcher;
    use sandbox::SandboxMode;
    use segmenter::Segmenter;
    use std::process::Stdio;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn capture_one_segment(kind: MediaStreamKind, url: &str) -> segmenter::Cut {
        let ffmpeg = decoder::resolve(&CaptureSettings::from_env().ffmpeg).expect("ffmpeg");
        let url = reqwest::Url::parse(url).unwrap();
        let opened = Fetcher::new(None)
            .unwrap()
            .open(kind, &url)
            .await
            .expect("open");
        let argv = decoder::argv(&ffmpeg, opened.format);
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        sandbox::confine(&mut cmd, &SandboxMode::Kernel, sandbox::DECODER_MAX_MEMORY);
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut chunks = opened.chunks;
        tokio::spawn(async move {
            while let Some(Ok(c)) = chunks.recv().await {
                if stdin.write_all(&c).await.is_err() {
                    break;
                }
            }
        });
        let mut cutter = Segmenter::new(Utc::now(), 10, 1);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(60), stdout.read(&mut buf))
                .await
                .expect("audio within 60 s")
                .unwrap();
            assert!(n > 0, "decoder ended early");
            if let Some(cut) = cutter.push(&buf[..n]).into_iter().next() {
                if let Ok(path) = std::env::var("PNEX_TEST_DUMP_WAV") {
                    std::fs::write(format!("{path}-{}.wav", kind.wire()), &cut.wav).unwrap();
                }
                return cut;
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn live_icecast_capture() {
        let cut = capture_one_segment(
            MediaStreamKind::Icecast,
            "https://icecast.radiofrance.fr/franceinter-midfi.mp3",
        )
        .await;
        assert_eq!(cut.wav.len(), 44 + 11 * 16_000 * 2);
    }

    #[tokio::test]
    #[ignore]
    async fn live_hls_capture() {
        let url = std::env::var("PNEX_TEST_HLS_URL").unwrap_or_else(|_| {
            "https://stream.radiofrance.fr/franceinter/franceinter.m3u8?id=radiofrance".into()
        });
        let cut = capture_one_segment(MediaStreamKind::Hls, &url).await;
        assert_eq!(cut.wav.len(), 44 + 11 * 16_000 * 2);
    }
}
