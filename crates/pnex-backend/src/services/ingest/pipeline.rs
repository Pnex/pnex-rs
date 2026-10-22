//! An import's own steps (data-pipelines.md D344): the same steps as the
//! flow `ingest` node, run by the server for an import run and journaled
//! in `ingest_steps` under that run. No flow, no runtime: the config comes
//! from the import, never from a request.

use pnex_core::err_codes;
use pnex_core::import_read::UNZIPPED_MAX;
use pnex_core::ingest::{IngestConfig, IngestMsg, IngestStep, PORT_OK};
use pnex_core::ontology::flow::FlowNodeRef;
use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use super::{archive, cleanup, download, filter, finish, resolve, table, Cx, Outcome};
use crate::services::artifact_store::S3Store;

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

async fn steps_then_file(
    cx: &Cx<'_>,
    steps: &[IngestConfig],
) -> Result<(Vec<u8>, String), StepFailure> {
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
            // Refused by the mapping check; never run here.
            IngestStep::OntologyImport | IngestStep::StagingCleanup => {
                Outcome::fail(err_codes::FIELD_INVALID, Value::Null)
            }
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
    let missing = || (err_codes::INGEST_ENTRY_MISSING.to_string(), Value::Null);
    if msg.staging_key.is_empty() {
        return Err(missing());
    }
    let unavailable = || {
        (
            err_codes::INGEST_STAGING_UNAVAILABLE.to_string(),
            Value::Null,
        )
    };
    match cx.store.size(&msg.staging_key).await {
        Ok(Some(n)) if n > UNZIPPED_MAX => {
            return Err((err_codes::IMPORT_SOURCE_TOO_LARGE.to_string(), Value::Null))
        }
        Ok(Some(_)) => {}
        Ok(None) => return Err(missing()),
        Err(_) => return Err(unavailable()),
    }
    let bytes = match cx.store.get_bytes(&msg.staging_key).await {
        Ok(Some(b)) => b,
        _ => return Err(unavailable()),
    };
    let name = msg
        .staging_key
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    Ok((bytes, name))
}
