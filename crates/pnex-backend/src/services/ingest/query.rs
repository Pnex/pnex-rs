//! `connector_query` (data-pipelines.md D352): runs one read-only query on an outside
//! database and streams its result as CSV into the staging, where the next steps of
//! the import read it like a downloaded file. The result is untrusted data.

use std::time::Duration;

use chrono::Utc;
use futures_util::TryStreamExt;
use pnex_core::connectors::QUERY_MAX_BYTES;
use pnex_core::err_codes;
use pnex_core::ingest::{IngestConfig, IngestMsg, StagedEntry};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, SqlSafeStr};
use uuid::Uuid;

use super::{stmt, unavailable, Cx, Outcome};
use crate::services::connectors;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const FILE_NAME: &str = "query.csv";

/// R2: what an import reads from a connector goes into a table every member reads,
/// so its author must still hold the right to use connectors. Checked on each run:
/// a schedule keeps running after the author's role changed.
async fn author_may_use(cx: &Cx<'_>) -> bool {
    use crate::models::_entities::{
        organization_members as om, sea_orm_active_enums::OrgMemberRole,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let Some(import) = cx
        .node
        .node_id
        .strip_prefix("import:")
        .and_then(|i| i.parse::<Uuid>().ok())
    else {
        return false;
    };
    let author: Option<i64> = cx
        .db
        .query_one_raw(stmt(
            "SELECT created_by FROM object_imports WHERE org_id = $1 AND id = $2",
            vec![cx.node.org_id.into(), import.into()],
        ))
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get("", "created_by").ok());
    let Some(author) = author else {
        return false;
    };
    let role: Option<OrgMemberRole> = om::Entity::find()
        .filter(om::Column::OrgId.eq(cx.node.org_id))
        .filter(om::Column::UserId.eq(author))
        .one(cx.db)
        .await
        .ok()
        .flatten()
        .map(|m| m.role);
    role.is_some_and(|r| crate::services::iam::authorize(&r, "connectors:use").allowed())
}

fn refused(code: &'static str) -> Outcome {
    Outcome::fail(code, Value::Null)
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, _msg: IngestMsg) -> Outcome {
    if cfg.query.trim().is_empty() || cfg.query.len() > QUERY_MAX_BYTES {
        return refused(err_codes::CONNECTOR_QUERY_INVALID);
    }
    if !author_may_use(cx).await {
        return refused(err_codes::CONNECTOR_FORBIDDEN);
    }
    let Some(ring) = crate::services::markets::keyring() else {
        return refused(err_codes::CONNECTOR_UNREACHABLE);
    };
    let room = match cx.room().await {
        Ok(n) => n,
        Err(o) => return *o,
    };
    let cap = cfg.max_bytes();
    let target = connectors::target(cx.db, ring, cx.node.org_id, Some(&cfg.connector), None).await;
    let target = match target {
        Ok(t) => t,
        Err(code) => return refused(code),
    };
    let mut conn = match target.connect(QUERY_TIMEOUT).await {
        Ok(c) => c,
        Err(code) => return refused(code),
    };
    // The extended protocol parses one statement alone. Parsed, it cannot close the
    // parenthesis below or chain another statement; the driver would run those. The
    // SQL is the admin's on purpose, on their own database.
    if let Err(e) = (&mut conn)
        .prepare(sqlx::AssertSqlSafe(cfg.query.clone()).into_sql_str())
        .await
    {
        // The server's text can quote the data: the code stays, the text in the logs.
        let state = e
            .as_database_error()
            .and_then(|d| d.code())
            .map(|c| c.to_string());
        tracing::info!(error = %e, "connector query refused");
        return Outcome::fail(
            err_codes::CONNECTOR_QUERY_INVALID,
            json!({ "sqlstate": state }),
        );
    }
    let copy = format!("COPY ({}\n) TO STDOUT (FORMAT csv, HEADER true)", cfg.query);
    let key = cx.key("raw", FILE_NAME);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let (mut hasher, mut bytes) = (Sha256::new(), 0u64);
    let copied = async {
        let mut stream = conn.copy_out_raw(&copy).await.map_err(|e| {
            tracing::info!(error = %e, "connector query failed");
            refused(err_codes::CONNECTOR_QUERY_INVALID)
        })?;
        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    bytes += chunk.len() as u64;
                    if bytes > cap {
                        let args = json!({ "bytes": bytes, "max": cap });
                        return Err(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
                    }
                    if bytes > room {
                        return Err(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
                    }
                    hasher.update(&chunk);
                    if w.write(chunk).await.is_err() {
                        return Err(unavailable());
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    tracing::info!(error = %e, "connector query stopped");
                    return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
                }
            }
        }
    }
    .await;
    if let Err(out) = copied {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let _ = conn.close().await;
    let sha256 = format!("{:x}", hasher.finalize());
    // No host or user in the journal (R4).
    let manifest = json!({
        "connector": cfg.connector,
        "bytes": bytes,
        "sha256": sha256,
        "fetched_at": Utc::now().to_rfc3339(),
    });
    let entry = StagedEntry {
        name: FILE_NAME.to_string(),
        staging_key: key.clone(),
        bytes,
    };
    let mut out = Outcome::ok(
        IngestMsg {
            raw_key: key.clone(),
            staging_key: key,
            ..Default::default()
        },
        manifest,
    );
    out.written = vec![entry];
    out.sha256 = Some(sha256);
    out.keep_until = Some(Utc::now() + chrono::Duration::days(cfg.keep_raw_days() as i64));
    out
}
