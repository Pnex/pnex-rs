//! Notification delivery journal → OpenObserve logs (D86).
//!
//! Every delivery attempt — websocket bus, Test buttons, OTA outcomes and
//! the `pnex-notify` flow node sends (through `/internal/notify/journal`) —
//! lands in the org's [`NOTIFY_DELIVERY_STREAM`] stream. Nothing is kept in
//! the relational database: retention is O2's (D72), no pruner.
//!
//! Writes are best effort: a journal failure never fails a delivery (it is
//! logged). Reads never provision the org (the events doctrine, D84).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use loco_rs::prelude::*;
use pnex_core::{NotifyDelivery, NotifyDeliveryEntry, NOTIFY_DELIVERY_STREAM};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

/// Default search window (and the "last status" badge window).
const DEFAULT_WINDOW_US: i64 = 30 * 24 * 3600 * 1_000_000;

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// Flat O2 document of one attempt (absent optionals are left out).
/// `message` (subject + error) is the full-text field: O2 `match_all` only
/// searches its default full-text keys, `message` being one of them.
pub fn document_of(e: &NotifyDeliveryEntry, ts_us: i64) -> serde_json::Value {
    let message = [e.subject.as_deref(), e.error.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" — ");
    let mut doc = serde_json::json!({
        "_timestamp": ts_us,
        "message": message,
        "channel_id": e.channel_id.to_string(),
        "channel_kind": e.channel_kind,
        "source": e.source,
        "status": e.status,
    });
    let obj = doc.as_object_mut().expect("json object");
    if let Some(t) = e.template_id {
        obj.insert("template_id".into(), t.to_string().into());
    }
    if let Some(h) = e.http_status {
        obj.insert("http_status".into(), h.into());
    }
    if let Some(err) = &e.error {
        obj.insert("error".into(), err.clone().into());
    }
    if let Some(s) = &e.subject {
        obj.insert("subject".into(), s.clone().into());
    }
    if let Some(f) = e.flow_id {
        obj.insert("flow_id".into(), f.into());
    }
    if let Some(n) = &e.node_id {
        obj.insert("node_id".into(), n.clone().into());
    }
    if let Some(d) = e.delivered {
        obj.insert("delivered".into(), d.into());
    }
    doc
}

/// Stored document → DTO (tolerant: a malformed hit keeps its defaults).
pub fn delivery_of(hit: &serde_json::Value) -> NotifyDelivery {
    let s = |k: &str| hit.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let uuid = |k: &str| {
        hit.get(k)
            .and_then(|v| v.as_str())
            .and_then(|v| Uuid::parse_str(v).ok())
    };
    let ts_us = hit["_timestamp"].as_i64().unwrap_or(0);
    NotifyDelivery {
        ts_us,
        channel_id: uuid("channel_id").unwrap_or_default(),
        channel_kind: s("channel_kind").unwrap_or_default(),
        template_id: uuid("template_id"),
        source: s("source").unwrap_or_default(),
        status: s("status").unwrap_or_default(),
        http_status: hit["http_status"]
            .as_u64()
            .and_then(|v| u16::try_from(v).ok()),
        error: s("error").filter(|v| !v.is_empty()),
        subject: s("subject").filter(|v| !v.is_empty()),
        flow_id: hit["flow_id"].as_i64(),
        node_id: s("node_id").filter(|v| !v.is_empty()),
        delivered: hit["delivered"].as_i64(),
        created_at: chrono::DateTime::from_timestamp_micros(ts_us)
            .unwrap_or_default()
            .to_rfc3339(),
    }
}

/// Journals one attempt (lazy O2 provisioning of the org).
pub async fn record(
    ctx: &AppContext,
    entry: &NotifyDeliveryEntry,
) -> std::result::Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, entry.org_id).await?;
    let ts_us = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json(
            &creds.o2_org,
            NOTIFY_DELIVERY_STREAM,
            &[document_of(entry, ts_us)],
            &creds.email_passcode,
        )
        .await
}

/// Downlink of the background writer — replaced at each boot (a test binary
/// boots one app per test, each on its own runtime).
static WRITER: LazyLock<Mutex<Option<mpsc::UnboundedSender<NotifyDeliveryEntry>>>> =
    LazyLock::new(|| Mutex::new(None));

/// Queues one attempt for the background writer — the delivery path never
/// waits on O2 and needs no `AppContext` (OTA transitions only hold a db).
pub fn submit(entry: NotifyDeliveryEntry) {
    let tx = WRITER.lock().expect("notify journal writer").clone();
    match tx {
        Some(tx) if tx.send(entry).is_ok() => {}
        _ => tracing::debug!("notify journal writer not running, entry dropped"),
    }
}

/// Background writer, spawned at boot (`after_routes`).
pub fn spawn_writer(ctx: &AppContext) {
    let (tx, mut rx) = mpsc::unbounded_channel::<NotifyDeliveryEntry>();
    *WRITER.lock().expect("notify journal writer") = Some(tx);
    let ctx = ctx.clone();
    tokio::spawn(async move {
        // The sender lives in a static: shutdown comes from the drain
        // registry, then the entries already queued are written.
        let mut drain = crate::services::drain::register();
        loop {
            let entry = tokio::select! {
                _ = drain.stopped() => break,
                entry = rx.recv() => match entry {
                    Some(entry) => entry,
                    None => break,
                },
            };
            write_entry(&ctx, &entry).await;
        }
        while let Ok(entry) = rx.try_recv() {
            write_entry(&ctx, &entry).await;
        }
    });
}

async fn write_entry(ctx: &AppContext, entry: &NotifyDeliveryEntry) {
    if let Err(e) = record(ctx, entry).await {
        tracing::warn!(
            channel = %entry.channel_id,
            status = %entry.status,
            "notify journal write failed: {e}"
        );
    }
}

/// Search filters (all optional).
#[derive(Debug, Clone, Default)]
pub struct DeliveryQuery {
    pub channel_id: Option<Uuid>,
    pub status: Option<String>,
    pub source: Option<String>,
    pub text: Option<String>,
    pub from_us: Option<i64>,
    pub to_us: Option<i64>,
    pub offset: i64,
    pub limit: i64,
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// SQL of a journal search (literals escaped).
pub fn search_sql(q: &DeliveryQuery) -> String {
    format!(
        "SELECT * FROM \"{NOTIFY_DELIVERY_STREAM}\"{} ORDER BY _timestamp DESC",
        where_clause(q)
    )
}

/// Total of a search — O2 `total` only counts the returned page.
pub fn count_sql(q: &DeliveryQuery) -> String {
    format!(
        "SELECT count(*) AS n FROM \"{NOTIFY_DELIVERY_STREAM}\"{}",
        where_clause(q)
    )
}

fn where_clause(q: &DeliveryQuery) -> String {
    let mut clauses = Vec::new();
    if let Some(id) = q.channel_id {
        clauses.push(format!("channel_id = '{id}'"));
    }
    if let Some(status) = q.status.as_deref().filter(|v| !v.is_empty()) {
        clauses.push(format!("status = '{}'", esc(status)));
    }
    if let Some(source) = q.source.as_deref().filter(|v| !v.is_empty()) {
        clauses.push(format!("source = '{}'", esc(source)));
    }
    if let Some(text) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        clauses.push(format!("match_all('{}')", esc(text)));
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    }
}

/// Last status per channel, one aggregate query — the channel cards badge.
pub fn last_status_sql(channel_ids: &[Uuid]) -> String {
    let ids: Vec<String> = channel_ids.iter().map(|id| format!("'{id}'")).collect();
    format!(
        "SELECT channel_id, first_value(status ORDER BY _timestamp DESC) AS status, \
         max(_timestamp) AS ts FROM \"{NOTIFY_DELIVERY_STREAM}\" \
         WHERE channel_id IN ({}) GROUP BY channel_id",
        ids.join(", ")
    )
}

/// A stream never written reads as empty, not as an error.
fn is_missing_stream(e: &str) -> bool {
    e.contains("Search stream not found") || e.contains("not found")
}

/// Journal page — `None` = O2 not configured or org never provisioned.
pub async fn search(
    ctx: &AppContext,
    org_id: i64,
    q: &DeliveryQuery,
) -> std::result::Result<Option<(i64, Vec<NotifyDelivery>)>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    let end = q
        .to_us
        .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
    let start = q.from_us.unwrap_or(end - DEFAULT_WINDOW_US);
    let page = client
        .search_logs(
            &creds.o2_org,
            &search_sql(q),
            start,
            end,
            q.offset,
            q.limit,
            &creds.email_passcode,
        )
        .await;
    let page = match page {
        Ok(r) => r,
        Err(e) if is_missing_stream(&e) => return Ok(Some((0, Vec::new()))),
        Err(e) => return Err(e),
    };
    let rows: Vec<NotifyDelivery> = page.hits.iter().map(delivery_of).collect();
    let total = client
        .search_logs(
            &creds.o2_org,
            &count_sql(q),
            start,
            end,
            0,
            1,
            &creds.email_passcode,
        )
        .await?
        .hits
        .first()
        .and_then(|h| h["n"].as_i64())
        .unwrap_or(0);
    Ok(Some((total.max(q.offset + rows.len() as i64), rows)))
}

/// `channel_id → (status, RFC 3339 of the attempt)` over the default
/// window. Best effort: any O2 trouble reads as "no badge".
pub async fn last_status_by_channel(
    ctx: &AppContext,
    org_id: i64,
    channel_ids: &[Uuid],
) -> HashMap<Uuid, (String, String)> {
    if channel_ids.is_empty() {
        return HashMap::new();
    }
    let Some(client) = client(ctx) else {
        return HashMap::new();
    };
    let creds = match provisioned_credentials(&ctx.db, org_id).await {
        Ok(Some(c)) => c,
        _ => return HashMap::new(),
    };
    let end = chrono::Utc::now().timestamp_micros();
    let res = client
        .search_logs(
            &creds.o2_org,
            &last_status_sql(channel_ids),
            end - DEFAULT_WINDOW_US,
            end + 1,
            0,
            channel_ids.len() as i64,
            &creds.email_passcode,
        )
        .await;
    let hits = match res {
        Ok(r) => r.hits,
        Err(e) => {
            if !is_missing_stream(&e) {
                tracing::warn!("notify journal last status: {e}");
            }
            return HashMap::new();
        }
    };
    hits.iter()
        .filter_map(|h| {
            let id = Uuid::parse_str(h["channel_id"].as_str()?).ok()?;
            let status = h["status"].as_str()?.to_string();
            let at = chrono::DateTime::from_timestamp_micros(h["ts"].as_i64()?)?.to_rfc3339();
            Some((id, (status, at)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_roundtrip() {
        let entry = NotifyDeliveryEntry {
            org_id: 1,
            channel_id: Uuid::from_u128(7),
            channel_kind: "webhook".into(),
            template_id: Some(Uuid::from_u128(9)),
            source: "flow".into(),
            status: "failed".into(),
            http_status: Some(502),
            error: Some("HTTP 502".into()),
            subject: Some("door".into()),
            flow_id: Some(4),
            node_id: Some("n1".into()),
            delivered: None,
        };
        let doc = document_of(&entry, 1_790_000_000_000_000);
        assert!(doc.get("delivered").is_none());
        let back = delivery_of(&doc);
        assert_eq!(back.channel_id, entry.channel_id);
        assert_eq!(back.template_id, entry.template_id);
        assert_eq!(back.http_status, Some(502));
        assert_eq!(back.flow_id, Some(4));
        assert_eq!(back.status, "failed");
        assert!(back.created_at.starts_with("2026-"));
    }

    #[test]
    fn sql_filters_and_escapes() {
        let q = DeliveryQuery {
            channel_id: Some(Uuid::nil()),
            status: Some("failed".into()),
            text: Some("it's".into()),
            ..Default::default()
        };
        assert_eq!(
            search_sql(&q),
            "SELECT * FROM \"notify_deliveries\" WHERE channel_id = '00000000-0000-0000-0000-000000000000' \
             AND status = 'failed' AND match_all('it''s') ORDER BY _timestamp DESC"
        );
        assert!(count_sql(&q)
            .starts_with("SELECT count(*) AS n FROM \"notify_deliveries\" WHERE channel_id"));
        assert!(
            last_status_sql(&[Uuid::nil()]).contains("IN ('00000000-0000-0000-0000-000000000000')")
        );
    }
}
