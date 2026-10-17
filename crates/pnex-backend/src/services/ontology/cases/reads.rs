//! Access log of persons' tracks (cases-workspace.md K38): every read of
//! tracks that holds a person (the timeline, the detectors, a frozen state)
//! lands in the org's O2 `case_reads` stream: who, which case, which
//! objects, which window. Ids only: a purged person leaves no name behind.
//!
//! Best effort and queued (`changes` school): a log failure never fails the
//! read it describes. Reads of the log never provision O2.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use loco_rs::prelude::*;
use pnex_core::ontology::api::CaseRead;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::super::{links, Result};
use super::case_of;
use crate::models::_entities::users;
use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const CASE_READS_STREAM: &str = "case_reads";
/// How far back the log is read.
const WINDOW_US: i64 = 365 * 24 * 3600 * 1_000_000;

/// Who reads tracks, and for what (`tracks`, `detect`, `snapshot`).
#[derive(Debug, Clone, Copy)]
pub struct Reader {
    pub user_id: Option<i64>,
    pub purpose: &'static str,
}

/// One logged read.
#[derive(Debug, Clone)]
pub struct Entry {
    pub org_id: i64,
    pub case_id: Uuid,
    pub reader: Reader,
    pub objects: Vec<Uuid>,
    pub window_ms: (i64, i64),
}

fn document_of(e: &Entry, ts_us: i64) -> Value {
    let ids: Vec<String> = e.objects.iter().map(Uuid::to_string).collect();
    json!({
        "_timestamp": ts_us,
        "case_id": e.case_id.to_string(),
        "user_id": e.reader.user_id,
        "purpose": e.reader.purpose,
        // Comma-joined: O2 flattens arrays poorly.
        "objects": ids.join(","),
        "from_ms": e.window_ms.0,
        "to_ms": e.window_ms.1,
    })
}

static WRITER: LazyLock<Mutex<Option<mpsc::UnboundedSender<Entry>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Queues a read for the background writer.
pub fn submit(entry: Entry) {
    let tx = WRITER.lock().expect("case reads writer").clone();
    match tx {
        Some(tx) if tx.send(entry).is_ok() => {}
        _ => tracing::debug!("case reads writer not running, read not logged"),
    }
}

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

async fn record(ctx: &AppContext, e: &Entry) -> std::result::Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, e.org_id).await?;
    let ts = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json(
            &creds.o2_org,
            CASE_READS_STREAM,
            &[document_of(e, ts)],
            &creds.email_passcode,
        )
        .await
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Entry>();
    *WRITER.lock().expect("case reads writer") = Some(tx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut drain = crate::services::drain::register();
        loop {
            let e = tokio::select! {
                _ = drain.stopped() => break,
                e = rx.recv() => match e {
                    Some(e) => e,
                    None => break,
                },
            };
            if let Err(err) = record(&ctx, &e).await {
                tracing::warn!(case = %e.case_id, "case read log write failed: {err}");
            }
        }
        while let Ok(e) = rx.try_recv() {
            let _ = record(&ctx, &e).await;
        }
    });
}

/// One page of the reads of a case over the last year, newest first, with
/// the total count. `None` = O2 absent or never provisioned (the controller
/// answers 503 `case-reads-unavailable`).
pub async fn reads(
    db: &DatabaseConnection,
    client: Option<&Client>,
    org_id: i64,
    case_id: Uuid,
    window: crate::controllers::pagination::PageParams,
) -> Result<Option<(i64, Vec<CaseRead>)>> {
    case_of(db, org_id, case_id).await?;
    let Some(client) = client else {
        return Ok(None);
    };
    let Ok(Some(creds)) = provisioned_credentials(db, org_id).await else {
        return Ok(None);
    };
    let end = chrono::Utc::now().timestamp_micros();
    // A UUID parsed by the controller: no quote to escape.
    let sql = format!(
        "SELECT * FROM \"{CASE_READS_STREAM}\" WHERE case_id = '{case_id}' ORDER BY _timestamp DESC"
    );
    let Ok(page) = client
        .search_logs(
            &creds.o2_org,
            &sql,
            end - WINDOW_US,
            end,
            window.offset,
            window.limit,
            &creds.email_passcode,
        )
        .await
    else {
        // No stream yet: nothing was read.
        return Ok(Some((0, Vec::new())));
    };
    let ids_of = |h: &Value| -> Vec<Uuid> {
        h.get("objects")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split(',')
            .filter_map(|s| Uuid::parse_str(s).ok())
            .collect()
    };
    let all: Vec<Uuid> = page.hits.iter().flat_map(&ids_of).collect();
    let objs = links::refs(db, org_id, all).await?;
    let user_ids: Vec<i64> = page
        .hits
        .iter()
        .filter_map(|h| h.get("user_id").and_then(Value::as_i64))
        .collect();
    let names: HashMap<i64, String> = if user_ids.is_empty() {
        HashMap::new()
    } else {
        users::Entity::find()
            .filter(users::Column::Id.is_in(user_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|u| (u.id, u.full_name.unwrap_or(u.email)))
            .collect()
    };
    let ms_ts = |k: &str, h: &Value| {
        h.get(k)
            .and_then(Value::as_i64)
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default()
    };
    let total = page.total.max(window.offset + page.hits.len() as i64);
    Ok(Some((
        total,
        page.hits
            .iter()
            .map(|h| {
                let ids = ids_of(h);
                CaseRead {
                    at: h
                        .get("_timestamp")
                        .and_then(Value::as_i64)
                        .and_then(chrono::DateTime::from_timestamp_micros)
                        .map(|t| t.to_rfc3339())
                        .unwrap_or_default(),
                    who: h
                        .get("user_id")
                        .and_then(Value::as_i64)
                        .and_then(|u| names.get(&u).cloned()),
                    purpose: h
                        .get("purpose")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    objects: ids
                        .iter()
                        .filter_map(|i| objs.get(i))
                        .map(links::object_ref)
                        .collect(),
                    gone: ids.iter().filter(|i| !objs.contains_key(i)).count(),
                    from: ms_ts("from_ms", h),
                    to: ms_ts("to_ms", h),
                }
            })
            .collect(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_is_logged_with_ids_only() {
        let id = Uuid::new_v4();
        let d = document_of(
            &Entry {
                org_id: 1,
                case_id: Uuid::nil(),
                reader: Reader {
                    user_id: Some(7),
                    purpose: "tracks",
                },
                objects: vec![id],
                window_ms: (1_000, 2_000),
            },
            5,
        );
        assert_eq!(d["objects"], id.to_string());
        assert_eq!(
            (d["user_id"].as_i64(), d["purpose"].as_str()),
            (Some(7), Some("tracks"))
        );
        assert_eq!(
            (d["from_ms"].as_i64(), d["to_ms"].as_i64()),
            (Some(1_000), Some(2_000))
        );
    }
}
