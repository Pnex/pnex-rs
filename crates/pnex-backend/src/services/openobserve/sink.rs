//! Telemetry batcher → OpenObserve (500 points / 10 s):
//! the WS pushes its points into a channel (never blocked), a task groups
//! them per org and flushes as **Prometheus remote-write**
//! (`/api/{org}/prometheus/api/v1/write`, see promwrite.rs — points
//! land in the org's metrics, not in the logs) — at most
//! `batch_max` points or `batch_flush_secs` of delay. Credentials resolved
//! per org (memory cache → database → idempotent provisioning). Flush failure:
//! one retry, then a logged drop (collection is never blocked by O2).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use loco_rs::app::AppContext;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;

use crate::services::openobserve::client::Client;
use crate::services::openobserve::promwrite;
use crate::services::openobserve::{ensure_org_credentials, OpenobserveSettings, OrgCredentials};
use crate::services::settings::IngestSettings;
use crate::services::telemetry::{self, TelemetryPoint, TelemetrySink};

/// Channel → sink bridge (try_send: full channel = point dropped, the WS
/// loop goes on).
struct ChannelSink(mpsc::Sender<TelemetryPoint>);

impl TelemetrySink for ChannelSink {
    fn send(&self, point: TelemetryPoint) {
        // Live-only points (edge agent keys without `record_o2`, D95) never
        // reach OpenObserve — the Valkey tap above already cached them.
        if !point.record {
            return;
        }
        let _ = self.0.try_send(point);
    }
}

/// Boot composition of the telemetry sink chain (`after_routes`): the O2
/// channel sink when OpenObserve is configured (noop otherwise), the Valkey
/// last-value tap when `settings.valkey.url` is set, all under the GPS tap.
/// The Valkey tap is installed independently of O2 so a Valkey-only
/// deployment still caches.
pub async fn spawn_batcher(ctx: &AppContext) {
    let o2 = OpenobserveSettings::from_config(&ctx.config);
    let ingest = IngestSettings::from_config(&ctx.config);
    let valkey = crate::services::last_cache::connect_opt(&ctx.config).await;

    let inner: Arc<dyn TelemetrySink> = match &o2 {
        Some(o2) => {
            let (tx, rx) = mpsc::channel::<TelemetryPoint>(4096);
            tokio::spawn(run(ctx.db.clone(), Client::new(o2), ingest, rx));
            Arc::new(ChannelSink(tx))
        }
        None => telemetry::noop_sink(),
    };
    let chain = match valkey {
        Some(conn) => crate::services::pois::GpsTapSink::new(
            ctx.db.clone(),
            crate::services::last_cache::ValkeyTapSink::new(conn, inner),
        ),
        None => crate::services::pois::GpsTapSink::new(ctx.db.clone(), inner),
    };
    telemetry::set_sink(chain);
}

/// Testable variant: explicit db + client + settings.
pub fn spawn_batcher_with(db: DatabaseConnection, client: Client, ingest: IngestSettings) {
    let (tx, rx) = mpsc::channel::<TelemetryPoint>(4096);
    // GPS tap (D38): the installed sink updates `device_positions` for
    // the `latitude`/`longitude`/`gps_*` metrics BEFORE forwarding to the
    // O2 batcher — never blocking, errors logged (services/pois.rs).
    telemetry::set_sink(crate::services::pois::GpsTapSink::new(
        db.clone(),
        Arc::new(ChannelSink(tx)),
    ));
    tokio::spawn(run(db, client, ingest, rx));
}

/// The org's credentials: memory cache, otherwise resolved via the database (and
/// idempotent provisioning if needed).
async fn credentials_for(
    db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
    creds: &mut HashMap<i64, OrgCredentials>,
) -> Option<OrgCredentials> {
    if let Some(c) = creds.get(&org_id) {
        return Some(c.clone());
    }
    match ensure_org_credentials(db, client, org_id).await {
        Ok(c) => {
            creds.insert(org_id, c.clone());
            // Self-healing flows: if this credential was just provisioned
            // (the org's 1st ingestion), flows deployed earlier carry an
            // empty `pnex_o2_org` stamp — the reprojection fills it without
            // waiting for a manual deploy. Errors: log only (the telemetry
            // stream must never depend on the flow reprojection).
            let db = db.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::controllers::flows::reproject_and_signal(&db, org_id).await {
                    tracing::warn!(err = %e, "flow reprojection after O2 provisioning: failed");
                }
            });
            Some(c)
        }
        Err(e) => {
            tracing::warn!(org = org_id, err = %e, "O2 provisioning failed — batch dropped");
            None
        }
    }
}

async fn flush(
    db: &DatabaseConnection,
    client: &Client,
    org_id: i64,
    points: &[TelemetryPoint],
    creds: &mut HashMap<i64, OrgCredentials>,
) {
    let Some(cred) = credentials_for(db, client, org_id, creds).await else {
        return;
    };
    let Some(body) = promwrite::encode(points) else {
        return; // no numeric point in the batch
    };
    match client
        .ingest_prometheus(&cred.o2_org, &body, &cred.email_passcode)
        .await
    {
        Ok(()) => {
            tracing::debug!(org = org_id, n = points.len(), "flush O2 ok");
        }
        Err(first) => {
            // Single retry (transient network), then a logged drop.
            tokio::time::sleep(Duration::from_secs(1)).await;
            match client
                .ingest_prometheus(&cred.o2_org, &body, &cred.email_passcode)
                .await
            {
                Ok(()) => {}
                Err(second) => {
                    // Invalidates the cache: credentials possibly stale
                    // (user/passcode revoked on the O2 side).
                    creds.remove(&org_id);
                    tracing::warn!(
                        org = org_id, n = points.len(), err = %second,
                        "O2 flush failed after retry (1st: {first})"
                    );
                }
            }
        }
    }
}

async fn run(
    db: DatabaseConnection,
    client: Client,
    ingest: IngestSettings,
    mut rx: mpsc::Receiver<TelemetryPoint>,
) {
    let flush_every = Duration::from_secs(ingest.batch_flush_secs.max(1));
    let mut pending: HashMap<i64, Vec<TelemetryPoint>> = HashMap::new();
    let mut creds: HashMap<i64, OrgCredentials> = HashMap::new();
    let mut next_flush = tokio::time::Instant::now() + flush_every;
    // The sender lives in a static (the installed sink): the channel never
    // closes, so shutdown is signalled through the drain registry.
    let mut drain = crate::services::drain::register();

    loop {
        let mut force_flush = false;
        tokio::select! {
            _ = drain.stopped() => {
                // Shutdown: take what is already queued, then final flush.
                while let Ok(p) = rx.try_recv() {
                    pending.entry(p.org_id).or_default().push(p);
                }
                break;
            }
            point = rx.recv() => {
                match point {
                    Some(p) => {
                        let bucket = pending.entry(p.org_id).or_default();
                        bucket.push(p);
                        if bucket.len() >= ingest.batch_max {
                            force_flush = true;
                        }
                    }
                    None => break, // channel closed: last flush
                }
            }
            _ = tokio::time::sleep_until(next_flush) => {
                force_flush = true;
            }
        }
        if force_flush {
            next_flush = tokio::time::Instant::now() + flush_every;
            for (org_id, points) in pending.drain() {
                flush(&db, &client, org_id, &points, &mut creds).await;
            }
        }
    }
    for (org_id, points) in pending.drain() {
        flush(&db, &client, org_id, &points, &mut creds).await;
    }
}
