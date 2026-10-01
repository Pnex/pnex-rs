//! Events → OpenObserve logs (camera-video.md D84): write path used by the
//! internal flow endpoint (`event-log` node), read path for the Events page.
//!
//! Doctrine of the O2 module kept: the write path may provision the org
//! lazily (like telemetry ingestion); the user read path never provisions —
//! an org without events simply reads `available: false`.

use loco_rs::prelude::*;
use pnex_core::events::{EventInput, EventRecord};

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

/// Default search window when the caller gives none.
const DEFAULT_WINDOW_US: i64 = 24 * 3600 * 1_000_000;

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// Document stored for one event. The payload is serialized to a string:
/// O2 flattens nested objects (`payload.x` → `payload_x`), a string keeps
/// the user's JSON intact.
pub fn document_of(ev: &EventInput, ts_us: i64) -> serde_json::Value {
    let mut doc = serde_json::json!({
        "_timestamp": ts_us,
        "level": ev.level.wire(),
        "message": ev.message,
        "node_id": ev.node_id,
        "payload": serde_json::to_string(&ev.payload).unwrap_or_default(),
    });
    if let Some(t) = &ev.topic {
        doc["topic"] = serde_json::Value::String(t.clone());
    }
    if let Some(f) = ev.flow_id {
        doc["flow_id"] = serde_json::Value::from(f);
    }
    doc
}

/// Writes one event (lazy O2 provisioning of the org).
pub async fn record(ctx: &AppContext, ev: &EventInput) -> std::result::Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, ev.org_id).await?;
    let ts_us = chrono::Utc::now().timestamp_micros();
    client
        .ingest_json(
            &creds.o2_org,
            &ev.stream,
            &[document_of(ev, ts_us)],
            &creds.email_passcode,
        )
        .await
}

/// Search filters (all optional).
#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    pub stream: String,
    pub level: Option<String>,
    pub text: Option<String>,
    pub from_us: Option<i64>,
    pub to_us: Option<i64>,
    pub offset: i64,
    pub limit: i64,
}

/// SQL of a search — `stream` must already be a normalized `ev_…` name
/// (charset `[a-z0-9_]`, safe to quote); literals are escaped.
pub fn search_sql(q: &EventQuery) -> String {
    format!(
        "SELECT * FROM \"{}\"{} ORDER BY _timestamp DESC",
        q.stream,
        where_clause(q)
    )
}

/// Total of a search — O2 `total` only counts the returned page.
pub fn count_sql(q: &EventQuery) -> String {
    format!(
        "SELECT count(*) AS n FROM \"{}\"{}",
        q.stream,
        where_clause(q)
    )
}

fn where_clause(q: &EventQuery) -> String {
    let esc = |s: &str| s.replace('\'', "''");
    let mut clauses = Vec::new();
    if let Some(level) = q.level.as_deref().filter(|l| !l.is_empty()) {
        clauses.push(format!("level = '{}'", esc(level)));
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

/// Stored document → DTO (payload string parsed back; tolerant of events
/// written by other tools).
pub fn record_of(stream: &str, hit: &serde_json::Value) -> EventRecord {
    let payload = match hit.get("payload") {
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone()))
        }
        Some(other) => other.clone(),
        None => serde_json::Value::Null,
    };
    EventRecord {
        ts_us: hit["_timestamp"].as_i64().unwrap_or(0),
        stream: stream.to_string(),
        level: hit["level"].as_str().unwrap_or("info").to_string(),
        message: hit["message"].as_str().unwrap_or_default().to_string(),
        topic: hit["topic"].as_str().map(str::to_string),
        flow_id: hit["flow_id"].as_i64(),
        node_id: hit["node_id"].as_str().unwrap_or_default().to_string(),
        payload,
    }
}

/// Result of a read: `None` = O2 not configured or org never provisioned.
pub struct SearchResult {
    pub total: i64,
    pub records: Vec<EventRecord>,
}

pub async fn search(
    ctx: &AppContext,
    org_id: i64,
    q: &EventQuery,
) -> std::result::Result<Option<SearchResult>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp_micros();
    let end = q.to_us.unwrap_or(now);
    let start = q.from_us.unwrap_or(end - DEFAULT_WINDOW_US);
    let res = client
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
    match res {
        Ok(r) => {
            let records: Vec<EventRecord> =
                r.hits.iter().map(|h| record_of(&q.stream, h)).collect();
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
            Ok(Some(SearchResult {
                total: total.max(q.offset + records.len() as i64),
                records,
            }))
        }
        // A stream that was never written is "not found" for O2: no events.
        Err(e) if e.contains("Search stream not found") || e.contains("not found") => {
            Ok(Some(SearchResult {
                total: 0,
                records: Vec::new(),
            }))
        }
        Err(e) => Err(e),
    }
}

/// Event streams (`ev_…`) of the org.
pub async fn streams(ctx: &AppContext, org_id: i64) -> std::result::Result<Vec<String>, String> {
    let Some(client) = client(ctx) else {
        return Ok(Vec::new());
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = client
        .log_streams(&creds.o2_org, &creds.email_passcode)
        .await?
        .into_iter()
        .filter(|n| n.starts_with(pnex_core::events::EVENT_STREAM_PREFIX))
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::events::EventLevel;

    #[test]
    fn sql_escapes_literals() {
        let q = EventQuery {
            stream: "ev_doors".into(),
            level: Some("warn".into()),
            text: Some("it's open".into()),
            ..Default::default()
        };
        assert_eq!(
            search_sql(&q),
            "SELECT * FROM \"ev_doors\" WHERE level = 'warn' AND match_all('it''s open') ORDER BY _timestamp DESC"
        );
    }

    #[test]
    fn document_roundtrip() {
        let ev = EventInput {
            org_id: 1,
            stream: "ev_x".into(),
            level: EventLevel::Warn,
            message: "hello".into(),
            topic: Some("cam".into()),
            flow_id: Some(3),
            node_id: "n1".into(),
            payload: serde_json::json!({"a": {"b": 1}}),
        };
        let doc = document_of(&ev, 42);
        let back = record_of("ev_x", &doc);
        assert_eq!(back.ts_us, 42);
        assert_eq!(back.level, "warn");
        assert_eq!(back.payload, serde_json::json!({"a": {"b": 1}}));
        assert_eq!(back.flow_id, Some(3));
        assert_eq!(back.topic.as_deref(), Some("cam"));
    }
}
