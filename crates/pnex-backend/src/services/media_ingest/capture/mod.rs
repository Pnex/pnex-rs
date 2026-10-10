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
//! storage write credential.

pub mod decoder;
pub mod fetch;
pub mod hls;
pub mod icy;
pub mod rtsp;
pub mod sandbox;
pub mod segmenter;
pub mod video;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::media_ingest::{
    CaptureOn, CaptureState, ClockSource, MediaStreamKind, MediaTracks, FPS_MAX, FPS_MIN,
    FRAME_MAX_WIDTH,
};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use uuid::Uuid;

use self::fetch::{Auth, Fetcher};
use self::sandbox::SandboxMode;
use self::segmenter::Segmenter;
use super::segments::{self, NewSegment};
use crate::models::_entities::{media_segments, media_streams};
use crate::services::media::{MediaSettings, MediaStore};
use crate::services::secrets::{store as secret_store, Keyring};

/// Period of the supervisor scan (and lease renewal).
const SCAN_EVERY: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(5);
const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// A run longer than this resets the backoff.
const HEALTHY_RUN: Duration = Duration::from_secs(300);
/// No decoded audio for this long = stalled stream.
const SILENT_INPUT: Duration = Duration::from_secs(60);

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

/// How a run ended without error.
enum RunEnd {
    /// The source finished (finite file, HLS `ENDLIST`).
    Finished,
    /// Asked to stop.
    Stopped,
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

/// One input of a decoder: its format and its bytes.
pub struct Track {
    pub format: decoder::InputFormat,
    pub chunks: tokio::sync::mpsc::Receiver<Result<axum::body::Bytes, CaptureError>>,
}

/// Duplicates a muxed input for the audio and the video decoders. A side
/// that stops reading is dropped; the other goes on.
fn tee(t: Track) -> (Track, Track) {
    let (a_tx, a_rx) = tokio::sync::mpsc::channel(4);
    let (v_tx, v_rx) = tokio::sync::mpsc::channel(4);
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

async fn run_once(
    ctx: &AppContext,
    settings: &CaptureSettings,
    ffmpeg: &std::path::Path,
    sink: &Sink,
    stream: &media_streams::Model,
    seq: &mut i64,
    mut stop: watch::Receiver<bool>,
) -> Result<RunEnd, CaptureError> {
    let url = super::streams::clean_url(&stream.url).map_err(|_| CaptureError::Unreachable)?;
    let kind = MediaStreamKind::from_wire(&stream.kind).ok_or(CaptureError::FormatUnsupported)?;
    let tracks = MediaTracks::from_wire(&stream.tracks).unwrap_or_default();
    let probe = matches!(sink, Sink::Probe(_));
    // A stream with video captures its audio only when it gets transcribed
    // (an audio-only stream without profile is never scanned); a test
    // never publishes frames (D175).
    let want_audio = tracks.has_audio()
        && (tracks == MediaTracks::Audio || probe || stream.asr_profile_id.is_some());
    let want_video = tracks.has_video() && !probe;
    let secret = secret_of(ctx, stream).await?;
    let mut first_pdt = None;
    let (audio, video) = if kind == MediaStreamKind::Rtsp {
        let creds = match secret.as_deref() {
            Some(s) => Some(rtsp::credentials(s).ok_or(CaptureError::SecretUnreadable)?),
            None => None,
        };
        let opened = tokio::select! {
            r = rtsp::open(&url, creds, want_audio, want_video) => r?,
            _ = stop.changed() => return Ok(RunEnd::Stopped),
        };
        (opened.audio, opened.video)
    } else {
        let auth = match secret.as_deref() {
            Some(s) => Some(Auth::parse(s, &url).ok_or(CaptureError::SecretUnreadable)?),
            None => None,
        };
        let mut opened = tokio::select! {
            r = Fetcher::new(auth)?.open(kind, &url) => r?,
            _ = stop.changed() => return Ok(RunEnd::Stopped),
        };
        // In-band metadata → O2 `mx_<slug>` (D170); a stream test stores
        // nothing. The task ends with the fetcher (its sender is dropped).
        if let Some(mut events) = opened.metadata.take().filter(|_| !probe) {
            let (ctx, slug, org_id) = (ctx.clone(), stream.slug.clone(), stream.org_id);
            tokio::spawn(async move {
                while let Some(ev) = events.recv().await {
                    if let Err(e) = super::metadata::write(&ctx, org_id, &slug, &ev).await {
                        tracing::warn!(stream = %slug, error = %e, "media metadata event not written");
                    }
                }
            });
        }
        first_pdt = opened.first_pdt;
        let format = opened.format;
        let track = Track {
            format,
            chunks: opened.chunks,
        };
        match (
            want_audio && format.has_audio(),
            want_video && format.has_video(),
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
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Decoded>(8);
    let mut decoders: Vec<(Decoder, &'static str)> = Vec::new();
    if let Some(track) = audio {
        let argv = decoder::argv(ffmpeg, track.format);
        let (d, mut out) = spawn_decoder(settings, argv, &stream.slug, track)?;
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
        let fps = stream.fps.clamp(FPS_MIN, FPS_MAX) as u32;
        let argv = decoder::video_argv(ffmpeg, track.format, fps, FRAME_MAX_WIDTH);
        let (d, mut out) = spawn_decoder(settings, argv, &stream.slug, track)?;
        let tx = tx.clone();
        let slug = stream.slug.clone();
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
    let mut valkey = None;
    if decoders.iter().any(|(_, t)| *t == "video") {
        valkey = crate::services::shared_valkey::conn(&ctx.config).await;
        if valkey.is_none() {
            tracing::warn!(stream = %stream.slug, "valkey not configured: video frames are not published");
        }
    }
    // Frame sequence from the clock: a restart never reuses a live key.
    let mut frame_seq = Utc::now().timestamp_millis() as u32;

    let (t0, clock) = match first_pdt {
        Some(pdt) => (pdt, ClockSource::Pdt),
        None => (Utc::now(), ClockSource::Host),
    };
    let mut cutter = Segmenter::new(
        t0,
        stream.segment_secs.max(1) as u32,
        stream.overlap_secs.max(0) as u32,
    );
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
        if !got_audio && !got_video && !probe {
            // A test never touches the state of the stream's own capture.
            set_state(&ctx.db, stream.id, CaptureState::Running, None).await;
        }
        match ev {
            Decoded::Pcm(bytes) => {
                got_audio = true;
                for cut in cutter.push(&bytes) {
                    store_cut(ctx, sink, stream, seq, cut, clock).await?;
                }
            }
            Decoded::Frame(frame) => {
                got_video = true;
                if let Some(conn) = valkey.as_mut() {
                    if let Err(e) =
                        video::publish(conn, stream.org_id, &stream.slug, frame_seq, &frame).await
                    {
                        tracing::debug!(stream = %stream.slug, error = %e, "video frame not published");
                    }
                }
                frame_seq = frame_seq.wrapping_add(1);
            }
        }
    };
    if matches!(outcome, Ok(RunEnd::Finished)) {
        if let Some(cut) = cutter.finish() {
            store_cut(ctx, sink, stream, seq, cut, clock).await?;
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

async fn store_cut(
    ctx: &AppContext,
    sink: &Sink,
    stream: &media_streams::Model,
    seq: &mut i64,
    cut: segmenter::Cut,
    clock: ClockSource,
) -> Result<(), CaptureError> {
    let seg = NewSegment {
        seq: *seq,
        started_at: cut.started_at,
        ended_at: cut.ended_at,
        clock_source: clock,
        wav: cut.wav,
    };
    *seq += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tee_feeds_both_sides_and_survives_one_leaving() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let (mut a, v) = tee(Track {
            format: decoder::H264,
            chunks: rx,
        });
        tx.send(Ok(axum::body::Bytes::from_static(b"one")))
            .await
            .unwrap();
        let mut v = v.chunks;
        assert_eq!(&a.chunks.recv().await.unwrap().unwrap()[..], b"one");
        assert_eq!(&v.recv().await.unwrap().unwrap()[..], b"one");
        // The video decoder goes away: audio keeps flowing.
        drop(v);
        tx.send(Ok(axum::body::Bytes::from_static(b"two")))
            .await
            .unwrap();
        assert_eq!(&a.chunks.recv().await.unwrap().unwrap()[..], b"two");
        drop(tx);
        assert!(a.chunks.recv().await.is_none());
    }
}
