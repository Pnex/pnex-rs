//! Journal of runner jobs in OpenObserve (agent-environments.md D719). Before finishing a
//! job, its runner sends the last lines of each container, the secrets it delivered masked;
//! the server masks again what looks like a credential and keeps one record per line. The
//! database keeps nothing of it: a run carries the link, not the lines.

use chrono::Utc;
use loco_rs::app::AppContext;
use pnex_core::environments::redact_tokens;
use pnex_core::runners::{RunnerJobLogLine, RunnerJobLogs, LOG_LINE_MAX_BYTES};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::openobserve::{
    ensure_org_credentials, provisioned_credentials, Client, OpenobserveSettings,
};

pub const LOGS_STREAM: &str = "runner_job_logs";
const BATCH: usize = 1000;
const HOUR_US: i64 = 3_600_000_000;
/// Written in front of a container whose older lines the runner left out.
pub const TRUNCATED_LINE: &str = "[older lines left out]";

fn client(ctx: &AppContext) -> Option<Client> {
    OpenobserveSettings::from_config(&ctx.config).map(|s| Client::new(&s))
}

/// `text` cut to `max` bytes on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One record per line, in the order the containers ran. `seq` keeps that order within the
/// microsecond the upload arrived in.
fn records_of(job: Uuid, logs: &RunnerJobLogs, at_us: i64) -> Vec<Value> {
    let mut out = Vec::new();
    for c in &logs.containers {
        let lines = c.truncated.then_some(TRUNCATED_LINE).into_iter();
        for line in lines.chain(c.text.lines()) {
            let seq = out.len() as i64;
            out.push(json!({
                "_timestamp": at_us + seq,
                "job_id": job.to_string(),
                "container": c.container,
                "seq": seq,
                "line": redact_tokens(cut(line, LOG_LINE_MAX_BYTES)),
            }));
        }
    }
    out
}

/// Keeps the journal of a job. Without OpenObserve on this deployment, it is dropped.
pub async fn store(
    ctx: &AppContext,
    org_id: i64,
    job: Uuid,
    logs: &RunnerJobLogs,
) -> Result<(), String> {
    let Some(client) = client(ctx) else {
        tracing::warn!(%job, "no OpenObserve on this deployment: job logs dropped");
        return Ok(());
    };
    let creds = ensure_org_credentials(&ctx.db, &client, org_id).await?;
    let records = records_of(job, logs, Utc::now().timestamp_micros());
    for batch in records.chunks(BATCH) {
        client
            .ingest_json(&creds.o2_org, LOGS_STREAM, batch, &creds.email_passcode)
            .await?;
    }
    Ok(())
}

/// The journal of a job created at `since_us`, oldest line first. `Ok(None)`: no OpenObserve.
pub async fn lines(
    ctx: &AppContext,
    org_id: i64,
    job: Uuid,
    since_us: i64,
    limit: i64,
    offset: i64,
) -> Result<Option<(i64, Vec<RunnerJobLogLine>)>, String> {
    let Some(client) = client(ctx) else {
        return Ok(None);
    };
    let Some(creds) = provisioned_credentials(&ctx.db, org_id).await? else {
        return Ok(Some((0, vec![])));
    };
    let found = client
        .search_logs(
            &creds.o2_org,
            &format!(
                "SELECT container, line FROM \"{LOGS_STREAM}\" WHERE job_id = '{job}' \
                 ORDER BY _timestamp ASC"
            ),
            since_us - HOUR_US,
            Utc::now().timestamp_micros() + HOUR_US,
            offset,
            limit,
            &creds.email_passcode,
        )
        .await;
    // A stream nothing wrote to yet answers with an error: no line is no failure.
    let found = match found {
        Ok(found) => found,
        Err(e) if e.contains("not found") => return Ok(Some((0, vec![]))),
        Err(e) => return Err(e),
    };
    let text = |hit: &Value, key: &str| hit[key].as_str().unwrap_or_default().to_string();
    let rows = found
        .hits
        .iter()
        .map(|hit| RunnerJobLogLine {
            container: text(hit, "container"),
            line: text(hit, "line"),
        })
        .collect();
    Ok(Some((found.total, rows)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::runners::RunnerContainerLog;

    #[test]
    fn each_line_is_one_ordered_record_masked_and_bounded() {
        let token = format!("pnagr_{}", "a".repeat(40));
        let logs = RunnerJobLogs {
            containers: vec![
                RunnerContainerLog {
                    container: "clone-0".into(),
                    text: format!("cloning\nkey {token}\n"),
                    truncated: true,
                },
                RunnerContainerLog {
                    container: "job".into(),
                    text: format!("{}\n", "日".repeat(LOG_LINE_MAX_BYTES)),
                    truncated: false,
                },
            ],
        };
        let records = records_of(Uuid::nil(), &logs, 100);
        let lines: Vec<&str> = records
            .iter()
            .map(|r| r["line"].as_str().unwrap())
            .collect();
        assert_eq!(&lines[..3], [TRUNCATED_LINE, "cloning", "key [masked]"]);
        assert!(lines[3].len() <= LOG_LINE_MAX_BYTES && lines[3].starts_with('日'));
        assert_eq!(records[3]["container"], "job");
        let at: Vec<i64> = records
            .iter()
            .map(|r| r["_timestamp"].as_i64().unwrap())
            .collect();
        assert_eq!(at, [100, 101, 102, 103]);
    }
}
