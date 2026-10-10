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
pub mod sandbox;
pub mod segmenter;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::media_ingest::{CaptureOn, CaptureState, ClockSource, MediaStreamKind};
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
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
        // No profile, no capture: the audio would never be transcribed.
        .filter(media_streams::Column::AsrProfileId.is_not_null())
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

async fn auth_of(
    ctx: &AppContext,
    stream: &media_streams::Model,
    url: &reqwest::Url,
) -> Result<Option<Auth>, CaptureError> {
    let Some(secret) = stream.secret_id else {
        return Ok(None);
    };
    let ring = Keyring::from_config(&ctx.config).map_err(|_| CaptureError::SecretUnreadable)?;
    let value = secret_store::reveal(&ctx.db, &ring, Some(stream.org_id), secret)
        .await
        .map_err(|_| CaptureError::SecretUnreadable)?;
    Auth::parse(&value, url)
        .map(Some)
        .ok_or(CaptureError::SecretUnreadable)
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
    let auth = auth_of(ctx, stream, &url).await?;
    let opened = tokio::select! {
        r = Fetcher::new(auth)?.open(kind, &url) => r?,
        _ = stop.changed() => return Ok(RunEnd::Stopped),
    };

    let argv = sandbox::wrap_argv(&settings.sandbox, decoder::argv(ffmpeg, opened.format));
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
    let mut stdout = child.stdout.take().ok_or(CaptureError::DecoderFailed)?;
    if let Some(stderr) = child.stderr.take() {
        let slug = stream.slug.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(stream = %slug, "ffmpeg: {line}");
            }
        });
    }

    // Fetcher → decoder stdin.
    let mut chunks = opened.chunks;
    let feeder = tokio::spawn(async move {
        while let Some(chunk) = chunks.recv().await {
            let chunk = chunk?;
            if stdin.write_all(&chunk).await.is_err() {
                return Err(CaptureError::DecoderFailed);
            }
        }
        drop(stdin);
        Ok::<(), CaptureError>(())
    });

    // Decoder stdout → segments.
    let (t0, clock) = match opened.first_pdt {
        Some(pdt) => (pdt, ClockSource::Pdt),
        None => (Utc::now(), ClockSource::Host),
    };
    let mut cutter = Segmenter::new(
        t0,
        stream.segment_secs.max(1) as u32,
        stream.overlap_secs.max(0) as u32,
    );
    let mut buf = vec![0u8; 64 * 1024];
    let mut running = false;
    let outcome = loop {
        let read = tokio::select! {
            r = tokio::time::timeout(SILENT_INPUT, stdout.read(&mut buf)) => r,
            _ = stop.changed() => break Ok(RunEnd::Stopped),
        };
        let n = match read {
            Err(_) => break Err(CaptureError::Stalled),
            Ok(Err(_)) => break Err(CaptureError::DecoderFailed),
            Ok(Ok(0)) => break Ok(RunEnd::Finished),
            Ok(Ok(n)) => n,
        };
        if !running {
            running = true;
            set_state(&ctx.db, stream.id, CaptureState::Running, None).await;
        }
        for cut in cutter.push(&buf[..n]) {
            store_cut(ctx, sink, stream, seq, cut, clock).await?;
        }
    };
    if matches!(outcome, Ok(RunEnd::Finished)) {
        if let Some(cut) = cutter.finish() {
            store_cut(ctx, sink, stream, seq, cut, clock).await?;
        }
    }
    let _ = child.start_kill();
    let fed = feeder.await.unwrap_or(Err(CaptureError::DecoderFailed));
    let status = child.wait().await.ok();
    match outcome {
        Ok(RunEnd::Finished) => {
            // The decoder ended: a fetch error explains it better.
            fed?;
            if status.is_some_and(|s| !s.success()) {
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
