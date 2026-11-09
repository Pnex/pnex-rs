//! SBOMs and scan events in OpenObserve (agent-environments.md D728): both are JSON text,
//! to consult and to follow over time. One stream holds each SBOM document as written, the
//! other one event per scan and per finding. The database keeps the checksum only.

use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::environments::{ScanHistoryEntry, ScanReport};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const SBOM_STREAM: &str = "environment_sboms";
pub const EVENTS_STREAM: &str = "environment_scan_events";
const EVENTS_BATCH: usize = 1000;
const HOUR_US: i64 = 3_600_000_000;
const HISTORY_WINDOW_US: i64 = 90 * 24 * HOUR_US;

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn sha256(text: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes())))
}

/// Who a scan is about, on every record, so that each one can be read alone.
fn subject(r: &QueryResult) -> Result<Value, sea_orm::DbErr> {
    let repository: String = r.try_get("", "repository")?;
    let digest: Option<String> = r.try_get("", "digest")?;
    Ok(json!({
        "environment_id": r.try_get::<Uuid>("", "environment_id")?.to_string(),
        "environment": r.try_get::<String>("", "name")?,
        "version": r.try_get::<i32>("", "number")?,
        "build_id": r.try_get::<Uuid>("", "build_id")?.to_string(),
        "image": format!("{repository}@{}", digest.unwrap_or_default()),
    }))
}

async fn scan_row(
    db: &DatabaseConnection,
    org_id: i64,
    job: Uuid,
) -> Result<Option<QueryResult>, sea_orm::DbErr> {
    db.query_one_raw(stmt(
        "SELECT s.report, b.id AS build_id, b.repository, b.digest, b.number, \
           e.id AS environment_id, e.name \
         FROM environment_scans s \
         JOIN environment_builds b ON b.id = s.build_id \
         JOIN environments e ON e.id = b.environment_id \
         WHERE s.org_id = $1 AND s.job_id = $2",
        vec![org_id.into(), job.into()],
    ))
    .await
}

fn merged(base: &Value, more: Value) -> Value {
    let mut out = base.clone();
    if let (Some(out), Value::Object(more)) = (out.as_object_mut(), more) {
        out.extend(more);
    }
    out
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stored {
    Yes,
    /// This deployment has no OpenObserve, or this job is no scan: nothing to wait for.
    Skipped,
}

/// Writes the SBOM of a scan job as received. `Err`: try again.
pub async fn store_sbom(
    ctx: &AppContext,
    org_id: i64,
    job: Uuid,
    document: &str,
) -> Result<Stored, String> {
    let Some(client) = client(ctx) else {
        tracing::warn!(%job, "no OpenObserve on this deployment: SBOM dropped");
        return Ok(Stored::Skipped);
    };
    let Some(row) = scan_row(&ctx.db, org_id, job)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(Stored::Skipped);
    };
    let creds = ensure_org_credentials(&ctx.db, &client, org_id).await?;
    let sha = sha256(document);
    let record = merged(
        &subject(&row).map_err(|e| e.to_string())?,
        json!({
            "_timestamp": Utc::now().timestamp_micros(),
            "scan_job": job.to_string(),
            "format": "CycloneDX",
            "sha256": sha,
            "bytes": document.len(),
            "document": document,
        }),
    );
    client
        .ingest_json(&creds.o2_org, SBOM_STREAM, &[record], &creds.email_passcode)
        .await?;
    // The stream exists from here on. Failing to set it is logged: it would only mean the
    // default retention, and the next SBOM tries again.
    // Long by default: an SBOM must outlive the image it describes (D755).
    let days = crate::services::fabric_settings::retention(ctx)
        .await
        .sbom_days;
    if let Err(e) = client
        .set_log_stream_retention(&creds.o2_org, SBOM_STREAM, days)
        .await
    {
        tracing::warn!(error = %e, "SBOM stream retention not set");
    }
    ctx.db
        .execute_raw(stmt(
            "UPDATE environment_scans SET sbom_sha256 = $3 WHERE org_id = $1 AND job_id = $2",
            vec![org_id.into(), job.into(), sha.into()],
        ))
        .await
        .map_err(|e| e.to_string())?;
    Ok(Stored::Yes)
}

#[derive(Debug)]
pub enum SbomError {
    /// No scan of this build holds a document, or it left the stream.
    Missing,
    Unavailable(String),
}

/// The SBOM of the latest scan of a build that has one, checked against its checksum.
pub async fn read_sbom(ctx: &AppContext, org_id: i64, build: Uuid) -> Result<String, SbomError> {
    let unavailable = |e: String| SbomError::Unavailable(e);
    let row = ctx
        .db
        .query_one_raw(stmt(
            "SELECT job_id, sbom_sha256, (extract(epoch FROM created_at) * 1000000)::bigint AS at_us \
             FROM environment_scans \
             WHERE org_id = $1 AND build_id = $2 AND sbom_sha256 IS NOT NULL \
             ORDER BY created_at DESC LIMIT 1",
            vec![org_id.into(), build.into()],
        ))
        .await
        .map_err(|e| unavailable(e.to_string()))?
        .ok_or(SbomError::Missing)?;
    let job: Uuid = row
        .try_get("", "job_id")
        .map_err(|e| unavailable(e.to_string()))?;
    let expected: String = row
        .try_get("", "sbom_sha256")
        .map_err(|e| unavailable(e.to_string()))?;
    let at_us: i64 = row
        .try_get("", "at_us")
        .map_err(|e| unavailable(e.to_string()))?;
    let client = client(ctx).ok_or_else(|| unavailable("no OpenObserve".into()))?;
    let creds = provisioned_credentials(&ctx.db, org_id)
        .await
        .map_err(unavailable)?
        .ok_or(SbomError::Missing)?;
    let found = client
        .search_logs(
            &creds.o2_org,
            &format!("SELECT document FROM \"{SBOM_STREAM}\" WHERE scan_job = '{job}'"),
            at_us - HOUR_US,
            Utc::now().timestamp_micros() + HOUR_US,
            0,
            1,
            &creds.email_passcode,
        )
        .await
        .map_err(unavailable)?;
    let document = found
        .hits
        .first()
        .and_then(|hit| hit["document"].as_str())
        .ok_or(SbomError::Missing)?;
    if sha256(document) != expected {
        return Err(unavailable(format!(
            "SBOM of {job} does not match its checksum"
        )));
    }
    Ok(document.to_string())
}

/// The events of one scan: its summary, then each finding.
fn events_of(subject: &Value, job: Uuid, report: &ScanReport, at_us: i64) -> Vec<Value> {
    let counts = report.summary();
    let mut events = vec![merged(
        subject,
        json!({
            "_timestamp": at_us,
            "kind": "scan",
            "scan_job": job.to_string(),
            "components": report.components.len(),
            "critical": counts.critical,
            "high": counts.high,
            "medium": counts.medium,
            "low": counts.low,
            "unknown": counts.unknown,
            "fixable": counts.fixable,
        }),
    )];
    events.extend(report.findings.iter().map(|f| {
        merged(
            subject,
            json!({
                "_timestamp": at_us,
                "kind": "finding",
                "scan_job": job.to_string(),
                "advisory": f.id,
                "package": f.package,
                "purl": f.purl,
                "installed": f.installed,
                "fixed": f.fixed,
                "severity": f.severity,
            }),
        )
    }));
    events
}

/// Journals a recorded scan. Best effort: the report stays in the database.
pub async fn journal_scan(ctx: &AppContext, org_id: i64, job: Uuid) -> Result<(), String> {
    let Some(client) = client(ctx) else {
        return Ok(());
    };
    let Some(row) = scan_row(&ctx.db, org_id, job)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let Some(report) = row
        .try_get::<Option<Value>>("", "report")
        .map_err(|e| e.to_string())?
        .and_then(|v| serde_json::from_value::<ScanReport>(v).ok())
    else {
        return Ok(());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, org_id).await?;
    let subject = subject(&row).map_err(|e| e.to_string())?;
    let events = events_of(&subject, job, &report, Utc::now().timestamp_micros());
    for batch in events.chunks(EVENTS_BATCH) {
        client
            .ingest_json(&creds.o2_org, EVENTS_STREAM, batch, &creds.email_passcode)
            .await?;
    }
    let days = crate::services::fabric_settings::retention(ctx)
        .await
        .scan_events_days;
    if let Err(e) = client
        .set_log_stream_retention(&creds.o2_org, EVENTS_STREAM, days)
        .await
    {
        tracing::warn!(error = %e, "scan events stream retention not set");
    }
    Ok(())
}

fn count_of(hit: &Value, key: &str) -> u32 {
    hit[key].as_u64().unwrap_or(0) as u32
}

/// Scans of an environment over the last 90 days, newest first. `Ok(None)`: no OpenObserve.
pub async fn history(
    ctx: &AppContext,
    org_id: i64,
    environment: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Option<(i64, Vec<ScanHistoryEntry>)>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(Some((0, vec![])));
    };
    let now = Utc::now().timestamp_micros();
    let found = client
        .search_logs(
            &creds.o2_org,
            &format!(
                "SELECT * FROM \"{EVENTS_STREAM}\" \
                 WHERE kind = 'scan' AND environment_id = '{environment}' \
                 ORDER BY _timestamp DESC"
            ),
            now - HISTORY_WINDOW_US,
            now + HOUR_US,
            offset,
            limit,
            &creds.email_passcode,
        )
        .await;
    // A stream nothing wrote to yet answers with an error: no scan is no failure.
    let found = match found {
        Ok(found) => found,
        Err(e) if e.contains("not found") => return Ok(Some((0, vec![]))),
        Err(e) => return Err(e),
    };
    let text = |hit: &Value, key: &str| hit[key].as_str().unwrap_or_default().to_string();
    let rows = found
        .hits
        .iter()
        .map(|hit| ScanHistoryEntry {
            at: chrono::DateTime::from_timestamp_micros(hit["_timestamp"].as_i64().unwrap_or(0))
                .unwrap_or_default()
                .to_rfc3339(),
            image: text(hit, "image"),
            version: hit["version"].as_i64().unwrap_or(0) as i32,
            components: count_of(hit, "components"),
            counts: pnex_core::environments::ScanCounts {
                critical: count_of(hit, "critical"),
                high: count_of(hit, "high"),
                medium: count_of(hit, "medium"),
                low: count_of(hit, "low"),
                unknown: count_of(hit, "unknown"),
                fixable: count_of(hit, "fixable"),
            },
        })
        .collect::<Vec<_>>();
    // The `total` of a paged search is the size of the page: the count is asked apart.
    let counted = client
        .search_logs(
            &creds.o2_org,
            &format!(
                "SELECT count(*) AS n FROM \"{EVENTS_STREAM}\" \
                 WHERE kind = 'scan' AND environment_id = '{environment}'"
            ),
            now - HISTORY_WINDOW_US,
            now + HOUR_US,
            0,
            1,
            &creds.email_passcode,
        )
        .await?;
    let total = counted
        .hits
        .first()
        .and_then(|h| h["n"].as_i64())
        .unwrap_or(rows.len() as i64);
    Ok(Some((total, rows)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::environments::{ScanComponent, ScanFinding};

    #[test]
    fn a_scan_gives_one_summary_and_one_event_per_finding() {
        let report = ScanReport {
            components: vec![ScanComponent {
                purl: "pkg:apk/a@1".into(),
                name: "a".into(),
                version: "1".into(),
            }],
            findings: vec![ScanFinding {
                id: "CVE-1".into(),
                purl: Some("pkg:apk/a@1".into()),
                package: "a".into(),
                installed: "1".into(),
                fixed: Some("2".into()),
                severity: "critical".into(),
            }],
        };
        let subject = json!({ "environment": "coder", "version": 3, "image": "r/a@sha256:1" });
        let events = events_of(&subject, Uuid::nil(), &report, 7);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["kind"], "scan");
        assert_eq!(events[0]["critical"], 1);
        assert_eq!(events[0]["components"], 1);
        assert_eq!(events[1]["kind"], "finding");
        assert_eq!(events[1]["advisory"], "CVE-1");
        // Each record reads alone.
        for event in &events {
            assert_eq!(event["environment"], "coder");
            assert_eq!(event["_timestamp"], 7);
        }
    }
}
