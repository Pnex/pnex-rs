//! Capture health (media-ingest.md D160 supervision, D171 technical
//! series). Every [`HEALTH_EVERY`] one pod (lease `media-health`) samples
//! each enabled stream from its segment rows, writes
//! `media_capture_up`, `media_capture_gap_seconds`,
//! `media_segment_lag_seconds` and `media_coverage_ratio` (label
//! `stream`) to the org's O2 metrics, and raises the "silent stream"
//! alert: in-app, plus the stream's notify channel when one is set.

use std::collections::HashSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use pnex_core::media_ingest::{CaptureState, MediaStreamKind, SegmentState};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::{Expr, Func};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::models::_entities::{media_segments, media_streams, notify_channels};
use crate::services::openobserve::promwrite::{encode_labelled, LabelledPoint};
use crate::services::openobserve::{ensure_org_credentials, Client, OpenobserveSettings};

const HEALTH_EVERY: Duration = Duration::from_secs(30);
/// No audio for this long = silent stream (D160: "flux muet > 2 min").
pub const SILENT_AFTER_SECS: i64 = 120;
/// Window of `media_coverage_ratio`.
const COVERAGE_WINDOW_SECS: i64 = 3600;

/// One sample of a stream's health.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub up: bool,
    /// Seconds since the end of the last captured segment (since the
    /// stream was last configured when it has none).
    pub gap_secs: i64,
    /// Age of the oldest segment waiting for its transcription.
    pub lag_secs: i64,
    /// Transcribed (or silent) audio over the last hour, 0..=1.
    pub coverage: f64,
}

impl Health {
    /// Silent: an enabled live stream with no audio for
    /// [`SILENT_AFTER_SECS`].
    pub fn silent(&self) -> bool {
        self.gap_secs >= SILENT_AFTER_SECS
    }
}

/// Pure part of the sample, kept apart for the tests.
pub fn compute(
    stream: &media_streams::Model,
    last_end: Option<DateTime<Utc>>,
    oldest_pending: Option<DateTime<Utc>>,
    covered_segments: i64,
    now: DateTime<Utc>,
) -> Health {
    let configured: DateTime<Utc> = stream.updated_at.into();
    let since = last_end.map_or(configured, |t| t.max(configured));
    let covered = (covered_segments * i64::from(stream.segment_secs.max(1))) as f64;
    Health {
        up: CaptureState::from_wire(&stream.capture_state) == Some(CaptureState::Running),
        gap_secs: (now - since).num_seconds().max(0),
        lag_secs: oldest_pending.map_or(0, |t| (now - t).num_seconds().max(0)),
        coverage: (covered / COVERAGE_WINDOW_SECS as f64).clamp(0.0, 1.0),
    }
}

pub async fn sample(
    db: &DatabaseConnection,
    stream: &media_streams::Model,
    now: DateTime<Utc>,
) -> Result<Health, DbErr> {
    let of_stream =
        || media_segments::Entity::find().filter(media_segments::Column::StreamId.eq(stream.id));
    let last_end: Option<Option<DateTimeWithTimeZone>> = of_stream()
        .select_only()
        .expr(Func::max(Expr::col(media_segments::Column::EndedAt)))
        .into_tuple()
        .one(db)
        .await?;
    let oldest_pending: Option<Option<DateTimeWithTimeZone>> = of_stream()
        .filter(media_segments::Column::State.is_in([
            SegmentState::Captured.wire(),
            SegmentState::Queued.wire(),
            SegmentState::Transcribing.wire(),
        ]))
        .select_only()
        .expr(Func::min(Expr::col(media_segments::Column::StartedAt)))
        .into_tuple()
        .one(db)
        .await?;
    let window: DateTimeWithTimeZone =
        (now - chrono::Duration::seconds(COVERAGE_WINDOW_SECS)).into();
    let covered = sea_orm::PaginatorTrait::count(
        of_stream()
            .filter(media_segments::Column::StartedAt.gte(window))
            .filter(media_segments::Column::State.is_in([
                SegmentState::Transcribed.wire(),
                SegmentState::SkippedSilence.wire(),
            ])),
        db,
    )
    .await?;
    Ok(compute(
        stream,
        last_end.flatten().map(Into::into),
        oldest_pending.flatten().map(Into::into),
        covered as i64,
        now,
    ))
}

fn points(slug: &str, h: &Health, now: DateTime<Utc>) -> [LabelledPoint; 4] {
    let p = |name, value| LabelledPoint {
        name,
        labels: vec![("stream", slug.to_string())],
        value,
        timestamp_ms: now.timestamp_millis(),
    };
    [
        p("media_capture_up", if h.up { 1.0 } else { 0.0 }),
        p("media_capture_gap_seconds", h.gap_secs as f64),
        p("media_segment_lag_seconds", h.lag_secs as f64),
        p("media_coverage_ratio", h.coverage),
    ]
}

/// One pass over every enabled stream. `alerted` holds the streams whose
/// current silence was already notified.
// shortcut: `alerted` lives in memory, a lease takeover re-notifies a
// silence once; persist it if that becomes noisy.
pub async fn tick(ctx: &AppContext, alerted: &mut HashSet<Uuid>) -> Result<(), DbErr> {
    let now = Utc::now();
    let streams = media_streams::Entity::find()
        .filter(media_streams::Column::Enabled.eq(true))
        .filter(media_streams::Column::DeletedAt.is_null())
        .filter(media_streams::Column::AsrProfileId.is_not_null())
        .all(&ctx.db)
        .await?;
    let mut by_org: std::collections::BTreeMap<i64, Vec<LabelledPoint>> = Default::default();
    let mut live = HashSet::new();
    for stream in &streams {
        let health = sample(&ctx.db, stream, now).await?;
        by_org
            .entry(stream.org_id)
            .or_default()
            .extend(points(&stream.slug, &health, now));
        // A file ends by itself: its end is not a silence.
        if stream.kind == MediaStreamKind::HttpFile.wire() {
            continue;
        }
        live.insert(stream.id);
        if !health.silent() {
            alerted.remove(&stream.id);
        } else if alerted.insert(stream.id) {
            alert_silent(ctx, stream, &health).await;
        }
    }
    alerted.retain(|id| live.contains(id));
    if let Some(settings) = OpenobserveSettings::from_config(&ctx.config) {
        let client = Client::new(&settings);
        for (org_id, pts) in by_org {
            let Some(pb) = encode_labelled(&pts) else {
                continue;
            };
            let res = match ensure_org_credentials(&ctx.db, &client, org_id).await {
                Ok(creds) => {
                    client
                        .ingest_prometheus(&creds.o2_org, &pb, &creds.email_passcode)
                        .await
                }
                Err(e) => Err(e),
            };
            if let Err(e) = res {
                tracing::debug!(org_id, error = %e, "media health metrics not written");
            }
        }
    }
    Ok(())
}

/// Canonical English text (no language context here, like OTA); the
/// structured `meta` lets a consumer localize.
fn silent_message(stream: &media_streams::Model, h: &Health) -> pnex_notify::Message {
    let minutes = h.gap_secs / 60;
    let reason = stream
        .capture_error
        .as_deref()
        .map(|e| format!(" (last capture error: {e})"))
        .unwrap_or_default();
    pnex_notify::Message {
        subject: Some(format!("Stream {} is silent", stream.name)),
        body: format!(
            "No audio was captured from the stream {} for {minutes} min{reason}.",
            stream.name
        ),
        meta: serde_json::json!({
            "kind": "media-silent",
            "stream": stream.slug,
            "gap_secs": h.gap_secs,
            "capture_error": stream.capture_error,
        }),
    }
}

async fn alert_silent(ctx: &AppContext, stream: &media_streams::Model, h: &Health) {
    use crate::services::notify;
    tracing::info!(stream = %stream.slug, org = stream.org_id, gap = h.gap_secs, "media stream silent");
    let msg = silent_message(stream, h);
    let entry = |channel_id| pnex_core::NotifyDeliveryEntry {
        org_id: stream.org_id,
        channel_id,
        source: "media".into(),
        ..Default::default()
    };
    // In-app, through the org's first websocket channel (OTA school).
    let in_app = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(stream.org_id))
        .filter(notify_channels::Column::Kind.eq("websocket"))
        .order_by_asc(notify_channels::Column::Id)
        .one(&ctx.db)
        .await
        .ok()
        .flatten();
    if let Some(ch) = &in_app {
        notify::deliver_in_app(entry(ch.id), &msg);
    }
    let Some(channel_id) = stream.notify_channel_id else {
        return;
    };
    if in_app.as_ref().is_some_and(|c| c.id == channel_id) {
        return;
    }
    let channel = notify_channels::Entity::find_by_id(channel_id)
        .filter(notify_channels::Column::OrgId.eq(stream.org_id))
        .filter(notify_channels::Column::Enabled.eq(true))
        .one(&ctx.db)
        .await
        .ok()
        .flatten();
    let Some(channel) = channel else {
        return;
    };
    if channel.kind == "websocket" {
        notify::deliver_in_app(entry(channel.id), &msg);
        return;
    }
    send_channel(ctx, stream.org_id, &channel, &msg).await;
}

/// Server-side send on a non-websocket channel (vault references resolved
/// in memory only), journaled like the Test button.
async fn send_channel(
    ctx: &AppContext,
    org_id: i64,
    channel: &notify_channels::Model,
    msg: &pnex_notify::Message,
) {
    let Some(ch) = pnex_notify::channel(&channel.kind) else {
        return;
    };
    let outcome = match crate::services::secrets::Keyring::from_config(&ctx.config) {
        Ok(ring) => {
            match crate::services::secrets::notify::sendable(
                &ctx.db,
                &ring,
                org_id,
                &channel.kind,
                &channel.config,
            )
            .await
            {
                Ok(config) => ch.send(&config, msg).await,
                Err(e) => Err(pnex_notify::NotifyError::Config(e.to_string())),
            }
        }
        Err(e) => Err(pnex_notify::NotifyError::Config(e.to_string())),
    };
    let (status, http_status, error) = match &outcome {
        Ok(()) => (pnex_core::DELIVERY_SENT, None, None),
        Err(e) => (
            pnex_core::DELIVERY_FAILED,
            match e {
                pnex_notify::NotifyError::Http { status } => Some(*status),
                _ => None,
            },
            Some(e.to_string()),
        ),
    };
    crate::services::notify_journal::submit(pnex_core::NotifyDeliveryEntry {
        org_id,
        channel_id: channel.id,
        channel_kind: channel.kind.clone(),
        source: "media".into(),
        status: status.into(),
        http_status,
        error,
        subject: msg.subject.clone(),
        ..Default::default()
    });
}

/// Background health loop, one pod at a time; skipped in tests
/// (`ForegroundBlocking`), where [`tick`] is called directly.
pub fn spawn(ctx: &AppContext) {
    use loco_rs::config::WorkerMode;
    if matches!(ctx.config.workers.mode, WorkerMode::ForegroundBlocking) {
        return;
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut alerted = HashSet::new();
        let mut tick_every = tokio::time::interval(HEALTH_EVERY);
        tick_every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick_every.tick().await;
            if !crate::services::singleton::my_turn(&ctx.db, "media-health", HEALTH_EVERY).await {
                alerted.clear();
                continue;
            }
            if let Err(e) = tick(&ctx, &mut alerted).await {
                tracing::warn!(error = %e, "media health pass failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(state: &str, configured_secs_ago: i64) -> media_streams::Model {
        let at: DateTimeWithTimeZone =
            (Utc::now() - chrono::Duration::seconds(configured_secs_ago)).into();
        media_streams::Model {
            id: Uuid::nil(),
            org_id: 1,
            name: "Inter".into(),
            slug: "inter".into(),
            kind: "icecast".into(),
            url: "https://radio.example/live".into(),
            secret_id: None,
            enabled: true,
            capture_on: "server".into(),
            asr_profile_id: Some(Uuid::nil()),
            tracks: "audio".into(),
            segment_secs: 30,
            overlap_secs: 1,
            audio_retention: "none".into(),
            notify_channel_id: None,
            timezone: "UTC".into(),
            tdm_checked_at: None,
            tdm_note: None,
            capture_state: state.into(),
            capture_error: None,
            capture_changed_at: None,
            deleted_at: None,
            created_by: None,
            created_at: at,
            updated_at: at,
        }
    }

    #[test]
    fn health_from_segments() {
        let now = Utc::now();
        let secs = |n| now - chrono::Duration::seconds(n);
        // Running, last audio 10 s ago, one segment waiting 40 s, 60 covered.
        let h = compute(
            &stream("running", 7200),
            Some(secs(10)),
            Some(secs(40)),
            60,
            now,
        );
        assert_eq!(
            h,
            Health {
                up: true,
                gap_secs: 10,
                lag_secs: 40,
                coverage: 0.5
            }
        );
        assert!(!h.silent());
        // Backoff, nothing for 5 min: silent.
        let h = compute(&stream("backoff", 7200), Some(secs(300)), None, 200, now);
        assert!(!h.up && h.silent() && h.lag_secs == 0 && h.coverage == 1.0);
        // Just (re)configured, no segment yet: not silent before 2 min.
        assert!(!compute(&stream("starting", 30), None, None, 0, now).silent());
        assert!(compute(&stream("starting", 600), None, None, 0, now).silent());
        // Old audio from before a reconfiguration does not count.
        assert!(!compute(&stream("running", 30), Some(secs(3600)), None, 0, now).silent());
    }

    #[test]
    fn silent_message_names_the_stream() {
        let mut s = stream("backoff", 600);
        s.capture_error = Some("unreachable".into());
        let h = compute(&s, None, None, 0, Utc::now());
        let m = silent_message(&s, &h);
        assert_eq!(m.subject.as_deref(), Some("Stream Inter is silent"));
        assert!(
            m.body.contains("10 min") && m.body.contains("unreachable"),
            "{}",
            m.body
        );
        assert_eq!(m.meta["kind"], "media-silent");
    }
}
