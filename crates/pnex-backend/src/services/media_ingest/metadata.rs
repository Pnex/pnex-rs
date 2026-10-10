//! In-band metadata in O2 logs (media-ingest.md D170): one document per
//! ICY title change in `mx_<slug>`, never in Postgres. Events, not ranges:
//! a flow decides what becomes a time range. Stream names are built here
//! from the slug of a stream row of the org.

use loco_rs::app::AppContext;
use pnex_core::media_ingest::{
    is_valid_slug, metadata_stream, MetadataRecord, METADATA_KIND_ICY_TITLE,
};

use super::capture::fetch::MetadataEvent;
use super::transcripts::ingest_retried;
use crate::services::openobserve::{provisioned_credentials, Client, OpenobserveSettings};

/// Default read window when `from` is absent: 7 days.
const DEFAULT_WINDOW_US: i64 = 7 * 24 * 3600 * 1_000_000;

pub fn document_of(slug: &str, ev: &MetadataEvent) -> serde_json::Value {
    serde_json::json!({
        "_timestamp": ev.at.timestamp_micros(),
        "stream": slug,
        "kind": METADATA_KIND_ICY_TITLE,
        // Default full-text field of O2, as for transcriptions (D165).
        "message": ev.title,
    })
}

/// Writes one ICY title event (lazy O2 provisioning, retried).
pub async fn write(
    ctx: &AppContext,
    org_id: i64,
    slug: &str,
    ev: &MetadataEvent,
) -> Result<(), String> {
    if !is_valid_slug(slug) {
        return Err("invalid stream slug".into());
    }
    ingest_retried(
        ctx,
        org_id,
        &metadata_stream(slug),
        &[document_of(slug, ev)],
    )
    .await
}

fn record_of(hit: &serde_json::Value) -> MetadataRecord {
    let ts_us = hit["_timestamp"].as_i64().unwrap_or(0);
    MetadataRecord {
        ts: chrono::DateTime::from_timestamp_micros(ts_us)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
        stream: hit["stream"].as_str().unwrap_or_default().to_string(),
        kind: hit["kind"].as_str().unwrap_or_default().to_string(),
        text: hit["message"].as_str().unwrap_or_default().to_string(),
    }
}

/// Newest events of one stream (slug already resolved as a stream of the
/// org by the caller): `(count, page)`.
pub async fn list(
    ctx: &AppContext,
    org_id: i64,
    slug: &str,
    from_us: Option<i64>,
    to_us: Option<i64>,
    offset: i64,
    limit: i64,
) -> Result<(i64, Vec<MetadataRecord>), String> {
    if !is_valid_slug(slug) {
        return Ok((0, Vec::new()));
    }
    let Some(client) = OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
    else {
        return Ok((0, Vec::new()));
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok((0, Vec::new()));
    };
    let end = to_us.unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
    let start = from_us.unwrap_or(end - DEFAULT_WINDOW_US);
    let table = metadata_stream(slug);
    let rows = format!("SELECT * FROM \"{table}\" ORDER BY _timestamp DESC");
    let page = match client
        .search_logs(
            &creds.o2_org,
            &rows,
            start,
            end,
            offset,
            limit,
            &creds.email_passcode,
        )
        .await
    {
        Ok(r) => r,
        // Never written yet: no event.
        Err(e) if e.contains("not found") => return Ok((0, Vec::new())),
        Err(e) => return Err(e),
    };
    let count = client
        .search_logs(
            &creds.o2_org,
            &format!("SELECT count(*) AS n FROM \"{table}\""),
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
    Ok((count, page.hits.iter().map(record_of).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_names_the_kind_and_keeps_the_title_in_message() {
        let ev = MetadataEvent {
            at: chrono::DateTime::from_timestamp(1_760_000_000, 0).unwrap(),
            title: "Artiste - Morceau".into(),
        };
        let doc = document_of("inter", &ev);
        assert_eq!(doc["_timestamp"], 1_760_000_000_000_000_i64);
        assert_eq!(doc["kind"], "icy_title");
        assert_eq!(doc["message"], "Artiste - Morceau");
        assert_eq!(record_of(&doc).text, "Artiste - Morceau");
        assert_eq!(metadata_stream("inter"), "mx_inter");
    }
}
