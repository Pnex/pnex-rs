//! Position history (cases-workspace.md K26): every complete GPS pair of a
//! device lands in the org's O2 `positions` logs stream, so a case can
//! replay trajectories. `device_positions` keeps only the latest one (D38).
//!
//! Best effort and batched (`notify_journal` school, but one O2 call per org
//! and per batch instead of per point): telemetry is never blocked, a full
//! queue drops the position. Reads never provision O2.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use loco_rs::prelude::*;
use pnex_core::track::TrackPoint;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const POSITIONS_STREAM: &str = "positions";
/// Queued positions waiting for the writer; beyond, new ones are dropped.
const QUEUE_MAX: usize = 50_000;
/// Positions drained per writer pass (all orgs together).
const BATCH_MAX: usize = 1_000;

/// One complete position of a device. `t_us` = observation time (µs, UTC).
#[derive(Debug, Clone, PartialEq)]
pub struct PositionEntry {
    pub org_id: i64,
    pub device_id: String,
    pub t_us: i64,
    pub lon: f64,
    pub lat: f64,
    pub alt: Option<f64>,
    pub accuracy_m: Option<f64>,
    pub speed_mps: Option<f64>,
    pub heading_deg: Option<f64>,
}

/// Flat O2 document of one position (absent optionals are left out).
pub fn document_of(e: &PositionEntry) -> Value {
    let mut doc = json!({
        "_timestamp": e.t_us,
        "device_id": e.device_id,
        "lon": e.lon,
        "lat": e.lat,
        "origin": "telemetry",
    });
    let obj = doc.as_object_mut().expect("json object");
    for (key, v) in [
        ("alt", e.alt),
        ("accuracy_m", e.accuracy_m),
        ("speed_mps", e.speed_mps),
        ("heading_deg", e.heading_deg),
    ] {
        if let Some(v) = v {
            obj.insert(key.into(), v.into());
        }
    }
    doc
}

/// Track point of an O2 hit, `None` when a coordinate is missing.
pub fn point_of(hit: &Value) -> Option<TrackPoint> {
    let t_us = hit.get("_timestamp").and_then(Value::as_i64)?;
    Some(TrackPoint {
        t_ms: t_us / 1_000,
        lon: hit.get("lon").and_then(Value::as_f64)?,
        lat: hit.get("lat").and_then(Value::as_f64)?,
        alt: hit.get("alt").and_then(Value::as_f64),
    })
}

static WRITER: LazyLock<Mutex<Option<mpsc::Sender<PositionEntry>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Queues a position for the background writer (never blocks).
pub fn submit(entry: PositionEntry) {
    let tx = WRITER.lock().expect("position log writer").clone();
    match tx {
        Some(tx) if tx.try_send(entry).is_ok() => {}
        _ => tracing::debug!("position log queue full or writer not running, position dropped"),
    }
}

async fn record(
    ctx: &AppContext,
    client: &Client,
    org_id: i64,
    docs: &[Value],
) -> std::result::Result<(), String> {
    let creds = ensure_org_credentials(&ctx.db, client, org_id).await?;
    client
        .ingest_json(&creds.o2_org, POSITIONS_STREAM, docs, &creds.email_passcode)
        .await
}

async fn flush(ctx: &AppContext, client: Option<&Client>, batch: Vec<PositionEntry>) {
    let Some(client) = client else {
        return;
    };
    let mut by_org: HashMap<i64, Vec<Value>> = HashMap::new();
    for e in &batch {
        by_org.entry(e.org_id).or_default().push(document_of(e));
    }
    for (org_id, docs) in by_org {
        if let Err(e) = record(ctx, client, org_id, &docs).await {
            tracing::warn!(
                org = org_id,
                count = docs.len(),
                "position log write failed: {e}"
            );
        }
    }
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::channel::<PositionEntry>(QUEUE_MAX);
    *WRITER.lock().expect("position log writer") = Some(tx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let client = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s));
        let mut drain = crate::services::drain::register();
        loop {
            let first = tokio::select! {
                _ = drain.stopped() => break,
                e = rx.recv() => match e {
                    Some(e) => e,
                    None => break,
                },
            };
            let mut batch = vec![first];
            while batch.len() < BATCH_MAX {
                match rx.try_recv() {
                    Ok(e) => batch.push(e),
                    Err(_) => break,
                }
            }
            flush(&ctx, client.as_ref(), batch).await;
        }
        let mut rest = Vec::new();
        while let Ok(e) = rx.try_recv() {
            rest.push(e);
        }
        flush(&ctx, client.as_ref(), rest).await;
    });
}

/// Positions of devices over `[start_us, end_us]`, oldest first, at most
/// `cap` points, as `(device_id, point)`. `None` = O2 absent or org never
/// provisioned (the UI says "positions unavailable").
pub async fn read(
    db: &sea_orm::DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    device_ids: &[String],
    start_us: i64,
    end_us: i64,
    cap: i64,
) -> std::result::Result<Option<Vec<(String, TrackPoint)>>, String> {
    let Some(client) = client else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(db, org_id).await? else {
        return Ok(None);
    };
    if device_ids.is_empty() {
        return Ok(Some(Vec::new()));
    }
    // Device ids come from the org's registry; quotes escaped all the same.
    let ids: Vec<String> = device_ids
        .iter()
        .map(|id| format!("'{}'", id.replace('\'', "''")))
        .collect();
    let sql = format!(
        // `*`, not a column list: O2 refuses a column no document has yet (`alt`).
        "SELECT * FROM \"{POSITIONS_STREAM}\" WHERE device_id IN ({}) ORDER BY _timestamp ASC",
        ids.join(",")
    );
    let page = client
        .search_logs(
            &creds.o2_org,
            &sql,
            start_us,
            end_us,
            0,
            cap,
            &creds.email_passcode,
        )
        .await?;
    Ok(Some(
        page.hits
            .iter()
            .filter_map(|h| {
                let id = h.get("device_id").and_then(Value::as_str)?.to_string();
                Some((id, point_of(h)?))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_roundtrips_to_a_track_point() {
        let e = PositionEntry {
            org_id: 1,
            device_id: "t-07".into(),
            t_us: 1_760_103_000_123_456,
            lon: 2.6207,
            lat: 48.9613,
            alt: Some(61.0),
            accuracy_m: None,
            speed_mps: Some(3.2),
            heading_deg: None,
        };
        let doc = document_of(&e);
        assert!(doc.get("accuracy_m").is_none() && doc.get("heading_deg").is_none());
        assert_eq!(doc["origin"], "telemetry");
        let p = point_of(&doc).unwrap();
        assert_eq!(
            p,
            TrackPoint {
                t_ms: 1_760_103_000_123,
                lon: 2.6207,
                lat: 48.9613,
                alt: Some(61.0)
            }
        );
        assert!(point_of(&json!({"_timestamp": 1, "lon": 2.0})).is_none());
    }
}
