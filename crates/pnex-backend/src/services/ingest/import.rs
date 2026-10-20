//! `ontology_import` (D322): an existing declarative import (D301) run on
//! the staged file instead of an upload or its URL — or, for an API import
//! (`per_object`, e.g. Recherche d'entreprises), on its own source. The
//! record cap applies here, after `rows_filter`.

use pnex_core::err_codes;
use pnex_core::import_read::UNZIPPED_MAX;
use pnex_core::ingest::{ImportAction, IngestConfig, IngestMsg};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{input_of, unavailable, Cx, Outcome};
use crate::services::ontology::imports;
use crate::services::ontology::OntologyError;

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let Ok(id) = Uuid::parse_str(cfg.import_id.trim()) else {
        return Outcome::fail(err_codes::ONTOLOGY_NOT_FOUND, Value::Null);
    };
    // A file when one is staged; an API import ignores it.
    let file = match input_of(cfg, &msg) {
        Ok(input) => match cx.store.size(&input.staging_key).await {
            Ok(Some(n)) if n > UNZIPPED_MAX => {
                return Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, Value::Null)
            }
            Ok(Some(_)) => match cx.store.get_bytes(&input.staging_key).await {
                Ok(Some(b)) => Some((b, input.name)),
                _ => return unavailable(),
            },
            Ok(None) => None,
            Err(_) => return unavailable(),
        },
        Err(_) => None,
    };
    let outcome = imports::run_from_flow(
        cx.db,
        cx.node.org_id,
        id,
        file,
        cfg.action == ImportAction::Apply,
        cfg.max_closures_pct(),
    )
    .await;
    let r = match outcome {
        Ok(r) => r,
        Err(OntologyError::NotFound) => {
            return Outcome::fail(
                err_codes::ONTOLOGY_NOT_FOUND,
                json!({ "import_id": id.to_string() }),
            )
        }
        Err(OntologyError::Import { code, args }) => {
            return Outcome::fail(code, args.unwrap_or(Value::Null))
        }
        Err(e) => {
            tracing::warn!(import = %id, error = %e, "flow import not run");
            return Outcome::fail(err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null);
        }
    };
    let manifest = json!({
        "import_id": id.to_string(),
        "run_id": r.run.id,
        "run_number": r.run.number,
        "state": r.run.state,
        "rows": r.run.rows,
        "created": r.counts.created,
        "updated": r.counts.updated,
        "closed": r.counts.closed,
        "links_opened": r.counts.links_opened,
        "links_updated": r.counts.links_updated,
        "errors": r.counts.errors,
        "warnings": r.counts.warnings,
    });
    match r.code {
        Some(code) => {
            let mut out = Outcome::fail(code, r.args);
            out.manifest = manifest;
            out
        }
        None => Outcome::ok(msg, manifest),
    }
}
