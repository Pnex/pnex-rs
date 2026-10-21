//! Open-data ingestion steps on the server side (ingestion.md D317–D322).
//! The flow runtime is not trusted: each call names its deployed `ingest`
//! node, whose config is read from the deployed version (never from the
//! request). Bytes live in the S3 staging area
//! (`org_{id}/staging/{flow}/{run}/…`), never on a local disk nor in the
//! flow messages; every step is journaled in `ingest_steps` with its
//! manifest (node panel, run log) and the objects it wrote (quota, cleanup,
//! purge).

mod archive;
mod download;
mod filter;
mod import;
mod resolve;
mod table;

use std::time::Duration;

use chrono::{DateTime, Utc};
use pnex_core::err_codes;
use pnex_core::flow::FlowNodeKind;
use pnex_core::ingest::{
    bounded_manifest, glob_match, IngestConfig, IngestMsg, IngestReply, IngestRequest, IngestStep,
    StagedEntry, PORT_ERROR, PORT_OK, STAGING_QUOTA_BYTES, STAGING_TTL_DAYS,
};
use pnex_core::ontology::flow::FlowNodeRef;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::ontology::flow_nodes::{deployed, NodeError};

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// What one step ran into.
pub(crate) struct Cx<'a> {
    pub db: &'a DatabaseConnection,
    pub store: &'a S3Store,
    pub node: &'a FlowNodeRef,
    pub run_id: Uuid,
}

impl Cx<'_> {
    /// Staging key of a file of this run (`sub` = `raw`, `x`, `f`).
    pub fn key(&self, sub: &str, name: &str) -> String {
        format!(
            "{}{}/{}/{sub}/{}",
            staging_prefix(self.node.org_id),
            self.node.flow_id,
            self.run_id,
            safe_name(name)
        )
    }

    /// Staging bytes the org holds now.
    pub async fn used_bytes(&self) -> Result<u64, sea_orm::DbErr> {
        let row = self
            .db
            .query_one_raw(stmt(
                "SELECT coalesce(sum(bytes), 0)::bigint AS b FROM ingest_steps \
                 WHERE org_id = $1 AND purged_at IS NULL",
                vec![self.node.org_id.into()],
            ))
            .await?;
        Ok(row
            .and_then(|r| r.try_get::<i64>("", "b").ok())
            .unwrap_or(0)
            .max(0) as u64)
    }

    /// Bytes this step may still write before the org quota.
    pub async fn room(&self) -> Result<u64, Box<Outcome>> {
        let used = self
            .used_bytes()
            .await
            .map_err(|_| Box::new(unavailable()))?;
        match STAGING_QUOTA_BYTES.checked_sub(used) {
            Some(n) if n > 0 => Ok(n),
            _ => Err(Box::new(Outcome::fail(
                err_codes::INGEST_STAGING_QUOTA,
                json!({ "used": used.to_string() }),
            ))),
        }
    }
}

pub fn staging_prefix(org_id: i64) -> String {
    format!("org_{org_id}/staging/")
}

/// The file name part of a key: ASCII letters, digits, `.`, `_`, `-`.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let mut s: String = base
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .take(120)
        .collect();
    if s.trim_matches('.').is_empty() {
        s = "file".into();
    }
    s
}

/// The result of a step: the next message, its port, the manifest, and
/// what it left in the staging area.
pub(crate) struct Outcome {
    pub port: usize,
    pub msg: IngestMsg,
    pub manifest: Value,
    pub code: Option<&'static str>,
    pub args: Value,
    pub written: Vec<StagedEntry>,
    pub sha256: Option<String>,
    pub keep_until: Option<DateTime<Utc>>,
}

impl Outcome {
    pub fn ok(msg: IngestMsg, manifest: Value) -> Self {
        Self {
            port: PORT_OK,
            msg,
            manifest,
            code: None,
            args: Value::Null,
            written: Vec::new(),
            sha256: None,
            keep_until: None,
        }
    }

    pub fn fail(code: &'static str, args: Value) -> Self {
        Self {
            port: PORT_ERROR,
            msg: IngestMsg::default(),
            manifest: Value::Null,
            code: Some(code),
            args,
            written: Vec::new(),
            sha256: None,
            keep_until: None,
        }
    }
}

pub(crate) fn unavailable() -> Outcome {
    Outcome::fail(err_codes::INGEST_STAGING_UNAVAILABLE, Value::Null)
}

/// The staged file a reading step works on: the extracted entry matching
/// `entry`, else the current file.
pub(crate) fn input_of(cfg: &IngestConfig, msg: &IngestMsg) -> Result<StagedEntry, Box<Outcome>> {
    let missing = || {
        Box::new(Outcome::fail(
            err_codes::INGEST_ENTRY_MISSING,
            json!({ "entry": if cfg.entry.is_empty() { "—" } else { cfg.entry.as_str() } }),
        ))
    };
    if cfg.entry.trim().is_empty() {
        if msg.staging_key.is_empty() {
            return Err(missing());
        }
        let name = msg.staging_key.rsplit('/').next().unwrap_or_default();
        return Ok(StagedEntry {
            name: name.to_string(),
            staging_key: msg.staging_key.clone(),
            bytes: 0,
        });
    }
    msg.entries
        .iter()
        .find(|e| glob_match(cfg.entry.trim(), &e.name))
        .cloned()
        .ok_or_else(missing)
}

/// A staged key is only read inside its own org's staging area: the
/// runtime could forge any key.
pub(crate) fn owned(node: &FlowNodeRef, key: &str) -> bool {
    key.starts_with(&staging_prefix(node.org_id)) && !key.contains("..")
}

/// Runs the step of the deployed node `req.node` and journals it.
pub async fn run(
    db: &DatabaseConnection,
    store: &S3Store,
    req: &IngestRequest,
) -> Result<IngestReply, NodeError> {
    let FlowNodeKind::Ingest { config } = deployed(db, &req.node).await? else {
        return Err(NodeError::NotDeployed);
    };
    let run_id = Uuid::parse_str(&req.msg.run_id).unwrap_or_else(|_| Uuid::new_v4());
    let cx = Cx {
        db,
        store,
        node: &req.node,
        run_id,
    };
    let mut msg = req.msg.clone();
    msg.run_id = run_id.to_string();
    // Keys of another org are dropped before any step reads them.
    if !msg.staging_key.is_empty() && !owned(&req.node, &msg.staging_key) {
        msg.staging_key.clear();
    }
    if !msg.raw_key.is_empty() && !owned(&req.node, &msg.raw_key) {
        msg.raw_key.clear();
    }
    msg.entries.retain(|e| owned(&req.node, &e.staging_key));

    let out = match config.check_deployable() {
        Some((code, message)) => Outcome::fail(code, json!({ "message": message })),
        None => match config.step {
            IngestStep::SourceResolve => resolve::run(&config, msg.clone()).await,
            IngestStep::FileDownload => download::run(&cx, &config, msg.clone()).await,
            IngestStep::ArchiveExtract => archive::run(&cx, &config, msg.clone()).await,
            IngestStep::TableInspect => table::run(&cx, &config, msg.clone()).await,
            IngestStep::RowsFilter => filter::run(&cx, &config, msg.clone()).await,
            // Outlives this call: journaled by its own task, polled.
            IngestStep::OntologyImport => return import::step(&cx, &config, msg).await,
            IngestStep::StagingCleanup => cleanup(&cx, msg.clone()).await,
        },
    };
    Ok(finish(&cx, &config, msg, out).await?)
}

/// Journals a step's outcome and answers the node.
pub(crate) async fn finish(
    cx: &Cx<'_>,
    config: &IngestConfig,
    msg: IngestMsg,
    out: Outcome,
) -> Result<IngestReply, sea_orm::DbErr> {
    let run_id = cx.run_id;
    // An error passes the message on as it came, for the error branch.
    let next = if out.port == PORT_ERROR {
        msg
    } else {
        IngestMsg {
            run_id: run_id.to_string(),
            ..out.msg.clone()
        }
    };
    let mut manifest = out.manifest.clone();
    // The error's args stay with its manifest (run log, node panel).
    if out.code.is_some() && !out.args.is_null() {
        if !manifest.is_object() {
            manifest = json!({});
        }
        manifest["args"] = out.args.clone();
    }
    let manifest = bounded_manifest(manifest);
    journal(cx, config, &out, &manifest).await?;
    Ok(IngestReply {
        port: out.port,
        msg: next,
        manifest,
        code: out.code.map(str::to_string),
        args: out.args,
        pending: false,
    })
}

async fn journal(
    cx: &Cx<'_>,
    cfg: &IngestConfig,
    out: &Outcome,
    manifest: &Value,
) -> Result<(), sea_orm::DbErr> {
    let bytes: u64 = out.written.iter().map(|e| e.bytes).sum();
    cx.db
        .execute_raw(stmt(
            "INSERT INTO ingest_steps (id, org_id, flow_id, flow_version, node_id, run_id, step, \
               port, code, manifest, sha256, keys, bytes, keep_until) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
            vec![
                Uuid::new_v4().into(),
                cx.node.org_id.into(),
                cx.node.flow_id.into(),
                cx.node.version.into(),
                cx.node.node_id.chars().take(64).collect::<String>().into(),
                cx.run_id.into(),
                cfg.step.key().into(),
                (out.port as i16).into(),
                out.code.map(str::to_string).into(),
                manifest.clone().into(),
                out.sha256.clone().into(),
                serde_json::to_value(&out.written)
                    .unwrap_or_default()
                    .into(),
                (bytes as i64).into(),
                out.keep_until.into(),
            ],
        ))
        .await?;
    Ok(())
}

/// Keys of journal rows, flattened.
fn keys_of(rows: &[sea_orm::QueryResult]) -> Vec<(Uuid, Vec<StagedEntry>)> {
    rows.iter()
        .filter_map(|r| {
            let id = r.try_get::<Uuid>("", "id").ok()?;
            let keys = r.try_get::<Value>("", "keys").ok()?;
            Some((id, serde_json::from_value(keys).unwrap_or_default()))
        })
        .collect()
}

/// Deletes the objects of journal rows and marks them purged.
async fn purge_rows(
    db: &DatabaseConnection,
    store: &S3Store,
    rows: &[sea_orm::QueryResult],
) -> (Vec<String>, u64) {
    let (mut deleted, mut freed) = (Vec::new(), 0u64);
    for (id, keys) in keys_of(rows) {
        let mut ok = true;
        for e in &keys {
            match store.delete_key(&e.staging_key).await {
                Ok(()) => {
                    deleted.push(e.staging_key.clone());
                    freed += e.bytes;
                }
                Err(err) => {
                    tracing::warn!(key = %e.staging_key, error = %err, "staging purge failed");
                    ok = false;
                }
            }
        }
        // A failed delete keeps the row live: the next sweep retries.
        if ok {
            let _ = db
                .execute_raw(stmt(
                    "UPDATE ingest_steps SET purged_at = now() WHERE id = $1",
                    vec![id.into()],
                ))
                .await;
        }
    }
    (deleted, freed)
}

/// `staging_cleanup`: the run's extracted and filtered files go now; the
/// raw download stays until its `keep_raw_days`.
async fn cleanup(cx: &Cx<'_>, msg: IngestMsg) -> Outcome {
    let rows = match cx
        .db
        .query_all_raw(stmt(
            "SELECT id, keys FROM ingest_steps WHERE org_id = $1 AND run_id = $2 \
               AND purged_at IS NULL AND (keep_until IS NULL OR keep_until <= now())",
            vec![cx.node.org_id.into(), cx.run_id.into()],
        ))
        .await
    {
        Ok(r) => r,
        Err(_) => return unavailable(),
    };
    let (deleted, freed) = purge_rows(cx.db, cx.store, &rows).await;
    let next = IngestMsg {
        run_id: msg.run_id,
        url: msg.url,
        ..Default::default()
    };
    Outcome::ok(
        next,
        json!({ "deleted_keys": deleted, "freed_bytes": freed }),
    )
}

/// Expired staging objects: past their `keep_until`, or older than the TTL
/// without one. Then objects of the staging prefixes no live row names
/// (a step that crashed between its write and its journal), older than a
/// day. Returns the deleted keys count.
pub async fn purge(db: &DatabaseConnection, store: &S3Store) -> Result<usize, String> {
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT id, keys FROM ingest_steps WHERE purged_at IS NULL AND ( \
                   (keep_until IS NULL AND created_at < now() - interval '{STAGING_TTL_DAYS} days') \
                   OR keep_until < now()) LIMIT 1000"
            ),
            vec![],
        ))
        .await
        .map_err(|e| e.to_string())?;
    let (deleted, _) = purge_rows(db, store, &rows).await;
    let mut count = deleted.len();
    let orgs = db
        .query_all_raw(stmt("SELECT id FROM organizations", vec![]))
        .await
        .map_err(|e| e.to_string())?;
    let old = Utc::now() - chrono::Duration::days(1);
    for org in orgs.iter().filter_map(|r| r.try_get::<i64>("", "id").ok()) {
        let objects = store.list_prefix(&staging_prefix(org)).await?;
        if objects.is_empty() {
            continue;
        }
        let live: std::collections::HashSet<String> = db
            .query_all_raw(stmt(
                "SELECT e ->> 'staging_key' AS k FROM ingest_steps, \
                   jsonb_array_elements(keys) e WHERE org_id = $1 AND purged_at IS NULL",
                vec![org.into()],
            ))
            .await
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|r| r.try_get::<String>("", "k").ok())
            .collect();
        for (key, _, at) in objects {
            if !live.contains(&key) && at.is_some_and(|t| t < old) {
                store.delete_key(&key).await?;
                count += 1;
            }
        }
    }
    Ok(count)
}

const PURGE_EVERY: Duration = Duration::from_secs(3600);

/// Hourly staging purge, on one pod.
pub fn spawn_purge(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let config = ctx.config.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(300)).await;
        loop {
            if crate::services::singleton::my_turn(&db, "ingest-staging-purge", PURGE_EVERY).await {
                match crate::services::artifact_store::S3Config::from_config(&config).store() {
                    Ok(store) => match purge(&db, &store).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!(deleted = n, "staging purge"),
                        Err(e) => tracing::warn!("staging purge failed: {e}"),
                    },
                    Err(e) => tracing::debug!("staging purge skipped: {e}"),
                }
            }
            tokio::time::sleep(PURGE_EVERY).await;
        }
    });
}

/// Deploy gate: an `ontology_import` names an import of the org, and an
/// `in_objects` filter a type of its schema.
pub async fn deploy_violations(
    db: &DatabaseConnection,
    org_id: i64,
    graph: &pnex_core::flow::FlowGraph,
) -> Result<Vec<pnex_core::FlowViolation>, sea_orm::DbErr> {
    let mut v = Vec::new();
    for n in &graph.nodes {
        let FlowNodeKind::Ingest { config } = &n.kind else {
            continue;
        };
        if let Some((code, message)) = config.check_deployable() {
            v.push(pnex_core::FlowViolation::new(
                Some(n.id.as_str()),
                code,
                message,
            ));
            continue;
        }
        let (sql, value): (&str, sea_orm::Value) = match config.step {
            IngestStep::OntologyImport => match Uuid::parse_str(config.import_id.trim()) {
                Ok(id) => (
                    "SELECT 1 AS x FROM object_imports WHERE org_id = $1 AND id = $2",
                    id.into(),
                ),
                Err(_) => continue,
            },
            IngestStep::RowsFilter if config.op == pnex_core::ingest::FilterOp::InObjects => (
                "SELECT 1 AS x FROM object_types WHERE org_id = $1 AND key = $2",
                config.object_type.clone().into(),
            ),
            _ => continue,
        };
        if db
            .query_one_raw(stmt(sql, vec![org_id.into(), value]))
            .await?
            .is_none()
        {
            v.push(pnex_core::FlowViolation::new(
                Some(n.id.as_str()),
                "ingest_target_unknown",
                "the import or object type of this ingestion step does not exist in the organization",
            ));
        }
    }
    Ok(v)
}

/// Journal of a flow's ingestion steps, newest first (node panel, run log).
pub async fn steps(
    db: &DatabaseConnection,
    org_id: i64,
    flow_id: i64,
    node_id: Option<&str>,
    limit: u64,
    offset: u64,
) -> Result<(u64, Vec<pnex_core::ingest::IngestStepView>), sea_orm::DbErr> {
    let filter = "org_id = $1 AND flow_id = $2 AND ($3::text IS NULL OR node_id = $3)";
    let values = || -> Vec<sea_orm::Value> {
        vec![
            org_id.into(),
            flow_id.into(),
            node_id.map(str::to_string).into(),
        ]
    };
    let count = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM ingest_steps WHERE {filter}"),
            values(),
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0)
        .max(0) as u64;
    let mut vals = values();
    vals.push((limit as i64).into());
    vals.push((offset as i64).into());
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT id, run_id, node_id, flow_version, step, port, code, manifest, bytes, \
                   purged_at, created_at FROM ingest_steps WHERE {filter} \
                 ORDER BY created_at DESC LIMIT $4 OFFSET $5"
            ),
            vals,
        ))
        .await?;
    let views = rows
        .iter()
        .filter_map(|r| {
            Some(pnex_core::ingest::IngestStepView {
                id: r.try_get::<Uuid>("", "id").ok()?.to_string(),
                run_id: r.try_get::<Uuid>("", "run_id").ok()?.to_string(),
                node_id: r.try_get("", "node_id").ok()?,
                flow_version: r.try_get("", "flow_version").ok()?,
                step: r.try_get("", "step").ok()?,
                port: r.try_get::<i16>("", "port").ok()? as u32,
                code: r.try_get("", "code").ok()?,
                manifest: r.try_get("", "manifest").ok()?,
                bytes: r.try_get::<i64>("", "bytes").ok()?.max(0) as u64,
                purged: r
                    .try_get::<Option<sea_orm::prelude::DateTimeWithTimeZone>>("", "purged_at")
                    .ok()?
                    .is_some(),
                created_at: r
                    .try_get::<sea_orm::prelude::DateTimeWithTimeZone>("", "created_at")
                    .ok()?
                    .to_rfc3339(),
            })
        })
        .collect();
    Ok((count, views))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe() {
        assert_eq!(safe_name("export fiches.zip"), "export_fiches.zip");
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(".."), "file");
        assert_eq!(safe_name("a\\b\\c.csv"), "c.csv");
    }

    #[test]
    fn keys_stay_in_the_org() {
        let node = FlowNodeRef {
            org_id: 4,
            flow_id: 1,
            version: 1,
            node_id: "n1".into(),
        };
        assert!(owned(&node, "org_4/staging/1/r/raw/a.zip"));
        assert!(!owned(&node, "org_44/staging/1/r/raw/a.zip"));
        assert!(!owned(&node, "org_4/media/a.jpg"));
        assert!(!owned(&node, "org_4/staging/../media/a.jpg"));
    }
}
