//! An import's own steps (data-pipelines.md D344), run by the server for an
//! import run and journaled in `ingest_steps` under that run. No flow, no
//! runtime: the config comes from the import, never from a request.

use futures_util::StreamExt;
use pnex_core::datasets::DatasetTransactionKind;
use pnex_core::err_codes;
use pnex_core::import_read::UNZIPPED_MAX;
use pnex_core::ingest::{IngestConfig, IngestMsg, IngestStep, PORT_OK};
use pnex_core::ontology::flow::FlowNodeRef;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::{archive, cleanup, download, filter, finish, query, resolve, table, Cx};
use crate::services::artifact_store::S3Store;

/// Bytes read at a time from a staged file into a table.
const READ_CHUNK: u64 = 8 * 1024 * 1024;

/// The journal identity of an import's steps (`ingest_steps`, staging keys).
fn node_of(org_id: i64, import_id: Uuid) -> FlowNodeRef {
    FlowNodeRef {
        org_id,
        flow_id: 0,
        version: 0,
        node_id: format!("import:{import_id}"),
    }
}

/// Why the steps stopped: a machine code and its args.
pub type StepFailure = (String, Value);

/// Runs `steps` for import run `run_id` and returns the file they leave
/// staged (bytes, name). The staging of the run is cleaned either way.
pub async fn run(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    import_id: Uuid,
    run_id: Uuid,
    steps: &[IngestConfig],
) -> Result<(Vec<u8>, String), StepFailure> {
    let node = node_of(org_id, import_id);
    let cx = Cx {
        db,
        store,
        node: &node,
        run_id,
    };
    let out = steps_then_file(&cx, steps).await;
    let done = cleanup(&cx, IngestMsg::default()).await;
    if done.code.is_some() {
        tracing::warn!(import = %import_id, run = %run_id, "import staging not cleaned");
    }
    out
}

/// Runs `steps` and loads the CSV they leave staged into a table, as a new
/// version, read in a stream. The staging of the run is cleaned either way.
#[allow(clippy::too_many_arguments)]
pub async fn run_into_table(
    db: &DatabaseConnection,
    store: &S3Store,
    org_id: i64,
    import_id: Uuid,
    run_id: Uuid,
    steps: &[IngestConfig],
    table: Uuid,
    delimiter: Option<u8>,
) -> Result<(), StepFailure> {
    let node = node_of(org_id, import_id);
    let cx = Cx {
        db,
        store,
        node: &node,
        run_id,
    };
    let out = async {
        let key = steps_then_key(&cx, steps).await?;
        let size = staged_size(&cx, &key).await?;
        let chunks =
            futures_util::stream::iter((0..size).step_by(READ_CHUNK as usize)).then(|at| {
                let key = key.as_str();
                async move { store.get_range(key, at..(at + READ_CHUNK).min(size)).await }
            });
        crate::services::datasets::load_csv(
            db,
            store,
            org_id,
            table,
            DatasetTransactionKind::Snapshot,
            delimiter,
            Box::pin(chunks),
        )
        .await
        .map(|_| ())
        .map_err(|e| (e.code().to_string(), Value::Null))
    }
    .await;
    let done = cleanup(&cx, IngestMsg::default()).await;
    if done.code.is_some() {
        tracing::warn!(import = %import_id, run = %run_id, "import staging not cleaned");
    }
    out
}

/// Size of a staged file, which must be there.
async fn staged_size(cx: &Cx<'_>, key: &str) -> Result<u64, StepFailure> {
    match cx.store.size(key).await {
        Ok(Some(n)) => Ok(n),
        Ok(None) => Err((err_codes::INGEST_ENTRY_MISSING.to_string(), Value::Null)),
        Err(_) => Err((
            err_codes::INGEST_STAGING_UNAVAILABLE.to_string(),
            Value::Null,
        )),
    }
}

async fn steps_then_file(
    cx: &Cx<'_>,
    steps: &[IngestConfig],
) -> Result<(Vec<u8>, String), StepFailure> {
    let key = steps_then_key(cx, steps).await?;
    let n = staged_size(cx, &key).await?;
    if n > UNZIPPED_MAX {
        let args = serde_json::json!({ "bytes": n, "max": UNZIPPED_MAX });
        return Err((err_codes::IMPORT_SOURCE_TOO_LARGE.to_string(), args));
    }
    let bytes = match cx.store.get_bytes(&key).await {
        Ok(Some(b)) => b,
        _ => {
            return Err((
                err_codes::INGEST_STAGING_UNAVAILABLE.to_string(),
                Value::Null,
            ))
        }
    };
    let name = key.rsplit('/').next().unwrap_or_default().to_string();
    Ok((bytes, name))
}

/// Runs the steps; returns the key of the file they leave staged.
async fn steps_then_key(cx: &Cx<'_>, steps: &[IngestConfig]) -> Result<String, StepFailure> {
    let mut msg = IngestMsg {
        run_id: cx.run_id.to_string(),
        ..Default::default()
    };
    for cfg in steps {
        let mut cfg = cfg.clone();
        // A run asked for is a run: an unchanged file is read again.
        cfg.skip_if_unchanged = Some(false);
        let out = match cfg.step {
            IngestStep::SourceResolve => resolve::run(&cfg, msg.clone()).await,
            IngestStep::FileDownload => download::run(cx, &cfg, msg.clone()).await,
            IngestStep::ArchiveExtract => archive::run(cx, &cfg, msg.clone()).await,
            IngestStep::TableInspect => table::run(cx, &cfg, msg.clone()).await,
            IngestStep::RowsFilter => filter::run(cx, &cfg, msg.clone()).await,
            IngestStep::ConnectorQuery => query::run(cx, &cfg, msg.clone()).await,
        };
        let reply = finish(cx, &cfg, msg, out).await.map_err(|_| {
            (
                err_codes::INGEST_STAGING_UNAVAILABLE.to_string(),
                Value::Null,
            )
        })?;
        if reply.port != PORT_OK {
            let code = reply
                .code
                .unwrap_or_else(|| err_codes::IMPORT_SOURCE_UNREADABLE.to_string());
            return Err((code, reply.args));
        }
        msg = reply.msg;
    }
    if msg.staging_key.is_empty() {
        return Err((err_codes::INGEST_ENTRY_MISSING.to_string(), Value::Null));
    }
    Ok(msg.staging_key)
}
