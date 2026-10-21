//! `ontology_import` (D322): an existing declarative import (D301) run on
//! the staged file instead of an upload or its URL — or, for an API import
//! (`per_object`, e.g. Recherche d'entreprises), on its own source. The
//! record cap applies here, after `rows_filter`.
//!
//! An import can outlast any HTTP call: it runs in its own task, which
//! journals the step. The call waits a little, then answers `pending`; the
//! node sends the same request again, which finds the journaled step (the
//! same answer, nothing run twice) or the run still going (`pending`), or
//! a run whose server is gone (failed, `import-run-interrupted`).

use std::time::Duration;

use pnex_core::err_codes;
use pnex_core::import_read::UNZIPPED_MAX;
use pnex_core::ingest::{ImportAction, IngestConfig, IngestMsg, IngestReply};
use pnex_core::ontology::import::ImportRunView;
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{finish, input_of, stmt, Cx, Outcome};
use crate::services::ontology::flow_nodes::NodeError;
use crate::services::ontology::imports;
use crate::services::ontology::OntologyError;

/// Heartbeat of a running import (well under the 10 min staleness).
const BEAT: Duration = Duration::from_secs(60);

/// How long a call waits for its import before answering `pending`;
/// `PNEX_INGEST_WAIT_MS` overrides it (tests).
fn wait() -> Duration {
    std::env::var("PNEX_INGEST_WAIT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(Duration::from_secs(25), Duration::from_millis)
}

fn pending(msg: IngestMsg) -> IngestReply {
    IngestReply {
        msg,
        pending: true,
        ..Default::default()
    }
}

pub(super) async fn step(
    cx: &Cx<'_>,
    cfg: &IngestConfig,
    msg: IngestMsg,
) -> Result<IngestReply, NodeError> {
    if let Some(reply) = journaled(cx, &msg).await? {
        return Ok(reply);
    }
    let Ok(id) = Uuid::parse_str(cfg.import_id.trim()) else {
        let out = Outcome::fail(err_codes::ONTOLOGY_NOT_FOUND, Value::Null);
        return Ok(finish(cx, cfg, msg, out).await?);
    };
    let (org, node_id) = (cx.node.org_id, cx.node.node_id.as_str());
    let not_found = || {
        Outcome::fail(
            err_codes::ONTOLOGY_NOT_FOUND,
            json!({ "import_id": id.to_string() }),
        )
    };
    if let Some(st) = imports::flow_run_of(cx.db, org, cx.run_id, node_id).await? {
        if !st.stale {
            return Ok(pending(msg));
        }
        // Its server is gone: the run's outcome is the step's.
        let run_id = Uuid::parse_str(&st.run.id).unwrap_or_default();
        imports::fail_flow_run(cx.db, run_id, err_codes::IMPORT_RUN_INTERRUPTED).await?;
        let out = match imports::flow_run_of(cx.db, org, cx.run_id, node_id).await? {
            Some(st) => outcome_of(id, &st.run, msg.clone()),
            None => not_found(),
        };
        return Ok(finish(cx, cfg, msg, out).await?);
    }
    let import_run = match imports::open_from_flow(cx.db, org, id, cx.run_id, node_id).await {
        Ok(Some(r)) => r,
        // A concurrent call of the same step opened it.
        Ok(None) => return Ok(pending(msg)),
        Err(OntologyError::NotFound) => return Ok(finish(cx, cfg, msg, not_found()).await?),
        Err(e) => {
            tracing::warn!(import = %id, error = %e, "flow import not opened");
            let out = Outcome::fail(err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null);
            return Ok(finish(cx, cfg, msg, out).await?);
        }
    };
    let (db, store, node, ingest_run) =
        (cx.db.clone(), cx.store.clone(), cx.node.clone(), cx.run_id);
    let cfg = cfg.clone();
    // A poll sends the whole message back: the steps after need it.
    let polled = msg.clone();
    let task = tokio::spawn(async move {
        let cx = Cx {
            db: &db,
            store: &store,
            node: &node,
            run_id: ingest_run,
        };
        let work = run(&cx, &cfg, msg.clone(), id, import_run);
        tokio::pin!(work);
        let mut tick = tokio::time::interval(BEAT);
        tick.tick().await;
        // A panic in `work` ends the beats too: the run turns stale.
        let out = loop {
            tokio::select! {
                out = &mut work => break out,
                _ = tick.tick() => imports::beat(&db, import_run).await,
            }
        };
        let reply = finish(&cx, &cfg, msg.clone(), out).await;
        if let Err(e) = &reply {
            tracing::warn!(run = %import_run, error = %e, "flow import step not journaled");
        }
        reply.ok()
    });
    match tokio::time::timeout(wait(), task).await {
        Ok(Ok(Some(reply))) => Ok(reply),
        // Not journaled: a later poll journals it from the run.
        Ok(Ok(None)) => Ok(pending(polled)),
        Ok(Err(e)) => {
            tracing::warn!(run = %import_run, error = %e, "flow import task failed");
            Ok(pending(polled))
        }
        Err(_) => Ok(pending(polled)),
    }
}

/// The answer already journaled for this step of this pass.
async fn journaled(cx: &Cx<'_>, msg: &IngestMsg) -> Result<Option<IngestReply>, sea_orm::DbErr> {
    let row = cx
        .db
        .query_one_raw(stmt(
            "SELECT port, code, manifest FROM ingest_steps \
             WHERE org_id = $1 AND flow_id = $2 AND node_id = $3 AND run_id = $4 \
               AND step = 'ontology_import' \
             ORDER BY created_at DESC LIMIT 1",
            vec![
                cx.node.org_id.into(),
                cx.node.flow_id.into(),
                cx.node.node_id.chars().take(64).collect::<String>().into(),
                cx.run_id.into(),
            ],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let manifest: Value = row.try_get("", "manifest")?;
    Ok(Some(IngestReply {
        port: row.try_get::<i16>("", "port")?.max(0) as usize,
        msg: msg.clone(),
        args: manifest.get("args").cloned().unwrap_or(Value::Null),
        manifest,
        code: row.try_get("", "code")?,
        pending: false,
    }))
}

fn manifest_of(id: Uuid, run: &ImportRunView) -> Value {
    let c = &run.counts;
    json!({
        "import_id": id.to_string(),
        "run_id": run.id,
        "run_number": run.number,
        "state": run.state,
        "rows": run.rows,
        "created": c.created,
        "updated": c.updated,
        "closed": c.closed,
        "links_opened": c.links_opened,
        "links_updated": c.links_updated,
        "errors": c.errors,
        "warnings": c.warnings,
    })
}

/// A finished run read back from its row (its refusal args are lost).
fn outcome_of(id: Uuid, run: &ImportRunView, msg: IngestMsg) -> Outcome {
    let manifest = manifest_of(id, run);
    match run.state.as_str() {
        "applied" | "preview" => Outcome::ok(msg, manifest),
        _ => {
            let code = run
                .state_code
                .as_deref()
                .and_then(|c| err_codes::ALL.iter().find(|k| **k == c).copied())
                .unwrap_or(err_codes::IMPORT_RUN_INTERRUPTED);
            let mut out = Outcome::fail(code, Value::Null);
            out.manifest = manifest;
            out
        }
    }
}

async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg, id: Uuid, run_id: Uuid) -> Outcome {
    // A run left `running` by an error is closed with its code.
    let failed = |code: &'static str, args: Value| async move {
        if let Err(e) = imports::fail_flow_run(cx.db, run_id, code).await {
            tracing::warn!(run = %run_id, error = %e, "flow import run not closed");
        }
        Outcome::fail(code, args)
    };
    // A file when one is staged; an API import ignores it.
    let file = match input_of(cfg, &msg) {
        Ok(input) => match cx.store.size(&input.staging_key).await {
            Ok(Some(n)) if n > UNZIPPED_MAX => {
                return failed(err_codes::IMPORT_SOURCE_TOO_LARGE, Value::Null).await
            }
            Ok(Some(_)) => match cx.store.get_bytes(&input.staging_key).await {
                Ok(Some(b)) => Some((b, input.name)),
                _ => return failed(err_codes::INGEST_STAGING_UNAVAILABLE, Value::Null).await,
            },
            Ok(None) => None,
            Err(_) => return failed(err_codes::INGEST_STAGING_UNAVAILABLE, Value::Null).await,
        },
        Err(_) => None,
    };
    let outcome = imports::run_from_flow(
        cx.db,
        cx.node.org_id,
        id,
        run_id,
        file,
        cfg.action == ImportAction::Apply,
        cfg.max_closures_pct(),
    )
    .await;
    let r = match outcome {
        Ok(r) => r,
        Err(OntologyError::NotFound) => {
            let args = json!({ "import_id": id.to_string() });
            return failed(err_codes::ONTOLOGY_NOT_FOUND, args).await;
        }
        Err(OntologyError::Import { code, args }) => {
            return failed(code, args.unwrap_or(Value::Null)).await
        }
        Err(e) => {
            tracing::warn!(import = %id, error = %e, "flow import not run");
            return failed(err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null).await;
        }
    };
    let manifest = manifest_of(id, &r.run);
    match r.code {
        Some(code) => {
            let mut out = Outcome::fail(code, r.args);
            out.manifest = manifest;
            out
        }
        None => Outcome::ok(msg, manifest),
    }
}
