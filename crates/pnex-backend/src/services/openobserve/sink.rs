//! Batcher télémétrie → OpenObserve (parité batching ES Django 500/10 s) :
//! le WS pousse ses points dans un canal (jamais bloqué), une tâche les
//! groupe par org et flushe en **Prometheus remote-write**
//! (`/api/{org}/prometheus/api/v1/write`, cf. promwrite.rs — les points
//! atterrissent dans les metrics de l'org, pas dans les logs) — max
//! `batch_max` points ou `batch_flush_secs` de délai. Credentials résolus
//! par org (cache mémoire → base → provisioning idempotent). Échec flush :
//! un retry, puis abandon loggé (la collecte n'est jamais bloquée par O2).

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

/// Pont canal → sink (try_send : canal plein = point abandonné, la boucle
/// WS continue).
struct ChannelSink(mpsc::Sender<TelemetryPoint>);

impl TelemetrySink for ChannelSink {
    fn send(&self, point: TelemetryPoint) {
        let _ = self.0.try_send(point);
    }
}

/// Installe le batcher si OpenObserve est configuré (sinon le sink noop
/// reste en place — cas des tests).
pub fn spawn_batcher(ctx: &AppContext) {
    let Some(o2) = OpenobserveSettings::from_config(&ctx.config) else {
        return;
    };
    let client = Client::new(&o2);
    let ingest = IngestSettings::from_config(&ctx.config);
    spawn_batcher_with(ctx.db.clone(), client, ingest);
}

/// Variante testable : db + client + réglages explicites.
pub fn spawn_batcher_with(db: DatabaseConnection, client: Client, ingest: IngestSettings) {
    let (tx, rx) = mpsc::channel::<TelemetryPoint>(4096);
    telemetry::set_sink(Arc::new(ChannelSink(tx)));
    tokio::spawn(run(db, client, ingest, rx));
}

/// Credentials de l'org : cache mémoire, sinon résolu via la base (et
/// provisioning idempotent si nécessaire).
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
            // Self-healing flows : si ce credential vient d'être provisionné
            // (1ʳᵉ ingestion de l'org), les flows déployés avant portent une
            // estampille `pnex_o2_org` vide — la reprojection la comble sans
            // attendre un deploy manuel. Erreurs : log seulement (le flux de
            // télémétrie ne doit jamais dépendre du reprojection flows).
            let db = db.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::controllers::flows::reproject_and_signal(&db).await {
                    tracing::warn!(err = %e, "reprojection flows post-provisioning O2 : échec");
                }
            });
            Some(c)
        }
        Err(e) => {
            tracing::warn!(org = org_id, err = %e, "provisioning O2 en échec — lot abandonné");
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
        return; // aucun point numérique dans le lot
    };
    match client
        .ingest_prometheus(&cred.o2_org, &body, &cred.email_passcode)
        .await
    {
        Ok(()) => {
            tracing::debug!(org = org_id, n = points.len(), "flush O2 ok");
        }
        Err(first) => {
            // Retry unique (réseau passager), puis abandon loggé.
            tokio::time::sleep(Duration::from_secs(1)).await;
            match client
                .ingest_prometheus(&cred.o2_org, &body, &cred.email_passcode)
                .await
            {
                Ok(()) => {}
                Err(second) => {
                    // Invalide le cache : credentials périmés possibles
                    // (user/passcode révoqués côté O2).
                    creds.remove(&org_id);
                    tracing::warn!(
                        org = org_id, n = points.len(), err = %second,
                        "flush O2 échoué après retry (1er : {first})"
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

    loop {
        let mut force_flush = false;
        tokio::select! {
            point = rx.recv() => {
                match point {
                    Some(p) => {
                        let bucket = pending.entry(p.org_id).or_default();
                        bucket.push(p);
                        if bucket.len() >= ingest.batch_max {
                            force_flush = true;
                        }
                    }
                    None => break, // canal fermé : dernier flush
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
