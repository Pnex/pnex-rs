//! The journal of the steps a pipeline plays for an import (`ingest_steps`,
//! its run history). The imports' own `steps:` (pipelines.md D344) are
//! retired: an org's pipeline fills a Table, its import reads it.

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use uuid::Uuid;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

const STEP_COLUMNS: &str = "id, run_id, node_id, flow_version, step, port, code, manifest, \
     bytes, purged_at, created_at";

fn step_view(r: &sea_orm::QueryResult) -> Option<pnex_core::ingest::IngestStepView> {
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
}

/// Steps journaled for the import's run `run_id`, in order.
pub async fn run_steps<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    run_id: Uuid,
) -> Result<Vec<pnex_core::ingest::IngestStepView>, sea_orm::DbErr> {
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT {STEP_COLUMNS} FROM ingest_steps \
                 WHERE org_id = $1 AND node_id = $2 AND run_id = $3 ORDER BY created_at"
            ),
            vec![
                org_id.into(),
                format!("import:{import_id}").into(),
                run_id.into(),
            ],
        ))
        .await?;
    Ok(rows.iter().filter_map(step_view).collect())
}
