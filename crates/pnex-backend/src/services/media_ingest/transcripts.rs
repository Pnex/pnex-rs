//! Transcriptions in O2 logs (media-ingest.md D165): one document per
//! segment in `tx_<slug>`, never in Postgres. Stream names are built here
//! from the slug of a stream row of the org; the search text is an escaped
//! literal (`match_all`), bounded.

use loco_rs::app::AppContext;
use pnex_core::media_ingest::{is_valid_slug, transcript_stream, TranscriptRecord};

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

/// Longest search text accepted.
pub const QUERY_MAX: usize = 200;
/// Default search window when `from` is absent: 7 days.
const DEFAULT_WINDOW_US: i64 = 7 * 24 * 3600 * 1_000_000;

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// Fields of one transcribed segment.
pub struct Doc<'a> {
    pub started_at_us: i64,
    pub ended_at_us: i64,
    pub slug: &'a str,
    pub segment_id: uuid::Uuid,
    pub text: &'a str,
    /// Word timings, serialized (O2 flattens nested objects, D84).
    pub words_json: String,
    /// Speaker turns `[{label, start_ms, end_ms}]`, serialized; `[]`
    /// without diarization. Labels are stream-local, never identities.
    pub speakers_json: String,
    /// Speech found by the VAD, ms; `None` when the profile has none.
    pub speech_ms: Option<u32>,
    pub lang: &'a str,
    pub asr_model: &'a str,
    pub asr_model_version: Option<i64>,
    /// Batch analysis of a file (D211): position of the segment in the
    /// file (ms) and the run it belongs to (D276); `None` for a live stream.
    pub file: Option<FilePos>,
}

/// Where a segment of an analysed file sits (D211, D276).
pub struct FilePos {
    pub offset_ms: i64,
    pub run: uuid::Uuid,
    pub origin: pnex_core::media_review::RunOrigin,
    /// Version transcribed; `None` for platform subtitles.
    pub version: Option<uuid::Uuid>,
}

pub fn document_of(d: &Doc<'_>) -> serde_json::Value {
    let mut doc = serde_json::json!({
        "_timestamp": d.started_at_us,
        "ended_at": d.ended_at_us,
        "stream": d.slug,
        "segment_id": d.segment_id.to_string(),
        // `message`: a default full-text field of O2, so `match_all` works
        // without stream settings (same field as the event streams, D84).
        "message": d.text,
        "words": d.words_json,
        "speakers": d.speakers_json,
        "speech_ms": d.speech_ms,
        "lang": d.lang,
        "asr_model": d.asr_model,
        "asr_model_version": d.asr_model_version,
    });
    if let Some(f) = &d.file {
        doc["offset_ms"] = f.offset_ms.into();
        doc["run"] = f.run.to_string().into();
        doc["origin"] = f.origin.wire().into();
        if let Some(v) = f.version {
            doc["version"] = v.to_string().into();
        }
    }
    doc
}

/// Delays between write attempts: a passcode just reset by provisioning
/// is refused by O2 for a few seconds; a transcription must not be lost
/// to a transient error.
const WRITE_RETRIES: [u64; 3] = [2, 5, 10];

/// Writes one document (lazy O2 provisioning of the org), retried on
/// transient failures.
pub async fn write(ctx: &AppContext, org_id: i64, doc: &Doc<'_>) -> Result<(), String> {
    if !is_valid_slug(doc.slug) {
        return Err("invalid stream slug".into());
    }
    ingest_retried(
        ctx,
        org_id,
        &transcript_stream(doc.slug),
        &[document_of(doc)],
    )
    .await
}

/// Writes several documents of one stream in one request (platform
/// subtitles, D276).
pub async fn write_many(
    ctx: &AppContext,
    org_id: i64,
    slug: &str,
    docs: &[Doc<'_>],
) -> Result<(), String> {
    if !is_valid_slug(slug) {
        return Err("invalid stream slug".into());
    }
    let body: Vec<serde_json::Value> = docs.iter().map(document_of).collect();
    for chunk in body.chunks(500) {
        ingest_retried(ctx, org_id, &transcript_stream(slug), chunk).await?;
    }
    Ok(())
}

/// The documents of one run of an analysed file, in media order (D276):
/// `(offset_ms, duration_ms, text, words_json, speakers_json)`. `None`
/// when the text store is not configured or not provisioned.
pub async fn run_documents(
    ctx: &AppContext,
    org_id: i64,
    slug: &str,
    run: uuid::Uuid,
    from_us: i64,
    to_us: i64,
) -> Result<Option<Vec<(i64, i64, String, String, String)>>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    if !is_valid_slug(slug) {
        return Ok(Some(Vec::new()));
    }
    // `run` is a parsed UUID: no quoting issue.
    let sql = format!(
        "SELECT * FROM \"{}\" WHERE run = '{run}' ORDER BY offset_ms ASC",
        transcript_stream(slug)
    );
    let page = client
        .search_logs(
            &creds.o2_org,
            &sql,
            from_us,
            to_us,
            0,
            RUN_MAX_DOCS,
            &creds.email_passcode,
        )
        .await;
    let hits = match page {
        Ok(r) => r.hits,
        Err(e) if e.contains("not found") => Vec::new(),
        Err(e) => return Err(e),
    };
    Ok(Some(
        hits.iter()
            .map(|h| {
                let start = h["_timestamp"].as_i64().unwrap_or(0);
                let end = h["ended_at"].as_i64().unwrap_or(start);
                (
                    h["offset_ms"].as_i64().unwrap_or(0),
                    (end - start) / 1000,
                    h["message"].as_str().unwrap_or_default().to_string(),
                    h["words"].as_str().unwrap_or("[]").to_string(),
                    h["speakers"].as_str().unwrap_or("[]").to_string(),
                )
            })
            .collect(),
    ))
}

/// The documents of a live stream between `from_us` and `to_us`, oldest
/// first: `(start_us, end_us, text, words_json, speakers_json)`. `None`
/// when the text store is not configured or not provisioned.
pub async fn window_documents(
    ctx: &AppContext,
    org_id: i64,
    slug: &str,
    from_us: i64,
    to_us: i64,
) -> Result<Option<Vec<(i64, i64, String, String, String)>>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(None);
    };
    if !is_valid_slug(slug) {
        return Ok(Some(Vec::new()));
    }
    let sql = format!(
        "SELECT * FROM \"{}\" ORDER BY _timestamp ASC",
        transcript_stream(slug)
    );
    let page = client
        .search_logs(
            &creds.o2_org,
            &sql,
            from_us,
            to_us,
            0,
            RUN_MAX_DOCS,
            &creds.email_passcode,
        )
        .await;
    let hits = match page {
        Ok(r) => r.hits,
        Err(e) if e.contains("not found") => Vec::new(),
        Err(e) => return Err(e),
    };
    Ok(Some(
        hits.iter()
            .map(|h| {
                let start = h["_timestamp"].as_i64().unwrap_or(0);
                (
                    start,
                    h["ended_at"].as_i64().unwrap_or(start),
                    h["message"].as_str().unwrap_or_default().to_string(),
                    h["words"].as_str().unwrap_or("[]").to_string(),
                    h["speakers"].as_str().unwrap_or("[]").to_string(),
                )
            })
            .collect(),
    ))
}

/// Documents read for one run: 4 h of 30 s segments, or a long subtitle.
const RUN_MAX_DOCS: i64 = 5_000;

/// Ingests documents in an O2 logs stream of the org (stream name built by
/// the caller from a checked slug), retried on transient failures.
pub(crate) async fn ingest_retried(
    ctx: &AppContext,
    org_id: i64,
    stream: &str,
    body: &[serde_json::Value],
) -> Result<(), String> {
    let Some(client) = client(ctx) else {
        return Err("openobserve is not configured".into());
    };
    let mut last = String::new();
    for delay in std::iter::once(0).chain(WRITE_RETRIES) {
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
        let creds = match ensure_org_credentials(&ctx.db, &client, org_id).await {
            Ok(c) => c,
            Err(e) => {
                last = e;
                continue;
            }
        };
        match client
            .ingest_json(&creds.o2_org, stream, body, &creds.email_passcode)
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// A search over the transcriptions of the given slugs (already resolved
/// as stream rows of the org by the caller).
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub slugs: Vec<String>,
    pub text: Option<String>,
    pub from_us: Option<i64>,
    pub to_us: Option<i64>,
    pub offset: i64,
    pub limit: i64,
}

fn sql(slug: &str, text: Option<&str>, count: bool) -> String {
    let esc = |s: &str| s.replace('\'', "''");
    let filter = text
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| format!(" WHERE match_all('{}')", esc(t)))
        .unwrap_or_default();
    let table = transcript_stream(slug);
    if count {
        format!("SELECT count(*) AS n FROM \"{table}\"{filter}")
    } else {
        format!("SELECT * FROM \"{table}\"{filter} ORDER BY _timestamp DESC")
    }
}

fn record_of(hit: &serde_json::Value) -> TranscriptRecord {
    let ts_us = hit["_timestamp"].as_i64().unwrap_or(0);
    TranscriptRecord {
        ts: chrono::DateTime::from_timestamp_micros(ts_us)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
        stream: hit["stream"].as_str().unwrap_or_default().to_string(),
        segment_id: hit["segment_id"].as_str().unwrap_or_default().to_string(),
        text: hit["message"]
            .as_str()
            .or_else(|| hit["text"].as_str())
            .unwrap_or_default()
            .to_string(),
        words: hit["words"].as_str().unwrap_or_default().to_string(),
        speakers: hit["speakers"].as_str().unwrap_or_default().to_string(),
        lang: hit["lang"].as_str().unwrap_or_default().to_string(),
        asr_model: hit["asr_model"].as_str().unwrap_or_default().to_string(),
    }
}

/// Newest first. Multi-stream searches query each stream and merge (the
/// list of slugs is short: streams of one org).
pub async fn search(
    ctx: &AppContext,
    org_id: i64,
    q: &Query,
) -> Result<(i64, Vec<TranscriptRecord>), String> {
    let Some(client) = client(ctx) else {
        return Ok((0, Vec::new()));
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok((0, Vec::new()));
    };
    let now = chrono::Utc::now().timestamp_micros();
    let end = q.to_us.unwrap_or(now);
    let start = q.from_us.unwrap_or(end - DEFAULT_WINDOW_US);
    let mut total = 0i64;
    let mut records = Vec::new();
    // Each stream returns up to offset + limit rows; the merge pages them.
    let size = q.offset + q.limit;
    for slug in q.slugs.iter().filter(|s| is_valid_slug(s)) {
        let page = client
            .search_logs(
                &creds.o2_org,
                &sql(slug, q.text.as_deref(), false),
                start,
                end,
                0,
                size,
                &creds.email_passcode,
            )
            .await;
        match page {
            Ok(r) => {
                records.extend(r.hits.iter().map(record_of));
                let n = client
                    .search_logs(
                        &creds.o2_org,
                        &sql(slug, q.text.as_deref(), true),
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
                total += n;
            }
            // Never written yet: no transcription.
            Err(e) if e.contains("not found") => {}
            Err(e) => return Err(e),
        }
    }
    records.sort_by(|a, b| b.ts.cmp(&a.ts));
    let page: Vec<TranscriptRecord> = records
        .into_iter()
        .skip(q.offset.max(0) as usize)
        .take(q.limit.max(0) as usize)
        .collect();
    Ok((total, page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_escapes_the_text_and_names_the_stream_from_the_slug() {
        assert_eq!(
            sql("inter", Some("l'Élysée"), false),
            "SELECT * FROM \"tx_inter\" WHERE match_all('l''Élysée') ORDER BY _timestamp DESC"
        );
        assert_eq!(
            sql("inter", None, true),
            "SELECT count(*) AS n FROM \"tx_inter\""
        );
        assert_eq!(
            sql("inter", Some("  "), false),
            "SELECT * FROM \"tx_inter\" ORDER BY _timestamp DESC"
        );
    }

    #[test]
    fn file_segments_carry_their_offset_and_run() {
        let doc = |file| Doc {
            started_at_us: 1,
            ended_at_us: 2,
            slug: "file_ab12cd34",
            segment_id: uuid::Uuid::nil(),
            text: "bonjour",
            words_json: "[]".into(),
            speakers_json: "[]".into(),
            speech_ms: None,
            lang: "fr",
            asr_model: "parakeet",
            asr_model_version: None,
            file,
        };
        let live = document_of(&doc(None));
        assert!(live.get("offset_ms").is_none() && live.get("run").is_none());
        let run = uuid::Uuid::new_v4();
        let file = document_of(&doc(Some(FilePos {
            offset_ms: 29_000,
            run,
            origin: pnex_core::media_review::RunOrigin::Platform,
            version: None,
        })));
        assert_eq!(file["offset_ms"], 29_000);
        assert_eq!(file["run"], run.to_string());
        assert_eq!(file["origin"], "platform");
        assert!(file.get("version").is_none());
    }
}
