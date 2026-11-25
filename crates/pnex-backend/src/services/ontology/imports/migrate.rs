//! Imports become org pipelines (pipelines.md D1050, step 3): run once at
//! boot, one import at a time, each in its own transaction. A pack's import
//! becomes the pack's copy, or the org's version when the org edited it
//! (D424); an import of the org reads its Table. What it wrote is credited to
//! the pipeline, its Table is freed, its run history goes. A Table import
//! keeps the base of its last applied run: an edit made by hand survives.

use pnex_core::ontology::api::Pack;
use pnex_core::ontology::import::{self as imp, ImportMode, ImportSource, Mapping};
use pnex_core::pipeline::doc::{Document, Step};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, FromQueryResult, TransactionTrait};
use serde_json::Value;
use uuid::Uuid;

use super::{
    copy_of, current_content, diff, keep_base, pack_content, pack_copy, stmt, sync, ImportRow,
    Result, RowsWriter, IMPORT_COLUMNS,
};
use crate::services::artifact_store::S3Store;

/// Converts every import left; one that fails stays for the next boot.
/// Returns how many were converted.
pub async fn run(db: &DatabaseConnection, store: Option<&S3Store>) -> Result<usize> {
    // Dropped once empty (migration 000173): nothing left to convert.
    if !imports_left(db).await? {
        return Ok(0);
    }
    let rows = ImportRow::find_by_statement(stmt(
        &format!("SELECT {IMPORT_COLUMNS} FROM object_imports ORDER BY org_id, name"),
        vec![],
    ))
    .all(db)
    .await?;
    let packs = super::super::packs::shipped();
    let mut done = 0;
    for row in &rows {
        match one(db, store, &packs, &rows, row).await {
            Ok(true) => done += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(org = row.org_id, import = %row.id, error = %e,
                "import not converted into a pipeline"),
        }
    }
    Ok(done)
}

/// Whether the table of imports is still there.
pub(crate) async fn imports_left<C: ConnectionTrait>(db: &C) -> Result<bool> {
    let row = db
        .query_one_raw(stmt(
            "SELECT to_regclass('object_imports') IS NOT NULL AS left",
            vec![],
        ))
        .await?;
    Ok(row.is_some_and(|r| r.try_get::<bool>("", "left").unwrap_or(false)))
}

/// The pipeline document of `row`, and the pack's copy when a pack ships it.
fn document(
    packs: &[Pack],
    all: &[ImportRow],
    row: &ImportRow,
) -> Option<(Document, Option<Document>)> {
    let m = row.mapping().ok()?;
    let own = imp::PackImport {
        name: row.name.clone(),
        source: row.source(),
        source_url: row.source_url.clone(),
        mode: row.mode(),
        recurrence: row.recurrence.clone(),
        max_pages: None,
        mapping: m.clone(),
    };
    let pack = row
        .pack_key
        .as_deref()
        .and_then(|k| packs.iter().find(|p| p.key == k));
    if let Some((pack, shipped)) = pack.and_then(|p| {
        p.imports
            .iter()
            .find(|i| i.name == row.name)
            .map(|i| (p, i))
    }) {
        let theirs = pack_copy(pack, shipped)?;
        let current = current_content(row);
        let written = row.pack_written.as_ref().map(|w| &w["written"]);
        let pristine = current.as_ref()
            == Some(&pack_content(
                &shipped.mapping,
                shipped.source,
                shipped.source_url.as_deref(),
            ))
            || (current.is_some() && (written.is_none() || written == current.as_ref()));
        // An edit of the older shape (a URL read by the import itself) has no
        // pipeline to run: the pack's version replaces it.
        let mine = (!pristine && m.pipeline.is_some())
            .then(|| pack_copy(pack, &own))
            .flatten();
        if !pristine && mine.is_none() {
            tracing::warn!(org = row.org_id, import = %row.id, "edited pack import of an older shape: the pack's version");
        }
        return Some((mine.unwrap_or_else(|| theirs.clone()), Some(theirs)));
    }
    let after = m
        .after
        .iter()
        .filter_map(|k| {
            all.iter()
                .filter(|r| r.org_id == row.org_id)
                .find(|r| r.mapping().is_ok_and(|o| o.key == *k))
        })
        .map(|r| r.name.clone())
        .collect();
    // Its Table, or one named after it where a member sends the file.
    let table = match row.source() {
        ImportSource::Dataset => None,
        _ => Some(row.name.clone()),
    };
    if matches!(row.source(), ImportSource::Url | ImportSource::Api) && m.pipeline.is_none() {
        tracing::warn!(org = row.org_id, import = %row.id,
            "import reading a URL itself: its pipeline reads the Table named after it");
    }
    Some((copy_of(&own, table.as_deref().unwrap_or("?"), after)?, None))
}

async fn one(
    db: &DatabaseConnection,
    store: Option<&S3Store>,
    packs: &[Pack],
    all: &[ImportRow],
    row: &ImportRow,
) -> Result<bool> {
    let Some((mut doc, theirs)) = document(packs, all, row) else {
        tracing::warn!(org = row.org_id, import = %row.id, "import mapping unreadable: not converted");
        return Ok(false);
    };
    // A Table import reads the Table it names by id.
    let dataset = match row.source() {
        ImportSource::Dataset => row
            .source_url
            .as_deref()
            .and_then(|u| u.parse::<Uuid>().ok()),
        _ => None,
    };
    if let Some(id) = dataset {
        let name: Option<String> = db
            .query_one_raw(stmt(
                "SELECT name FROM datasets WHERE id = $1 AND org_id = $2",
                vec![id.into(), row.org_id.into()],
            ))
            .await?
            .and_then(|r| r.try_get("", "name").ok());
        let Some(name) = name else {
            tracing::warn!(org = row.org_id, import = %row.id, "its Table is gone: not converted");
            return Ok(false);
        };
        for s in &mut doc.steps {
            if let Step::Sql(q) = s {
                q.tables.insert("source".into(), name.clone());
            }
        }
    }
    let id = Uuid::new_v4();
    let seeded = match (store, dataset) {
        (Some(store), Some(table)) => seed(db, store, row, &doc, id, table).await.unwrap_or_else(|e| {
            tracing::warn!(import = %row.id, error = %e, "base not seeded: the first run compares to the ontology");
            None
        }),
        _ => None,
    };
    let document = serde_json::to_string_pretty(&doc).map_err(|e| DbErr::Custom(e.to_string()))?;
    let from_pack = theirs.as_ref() == Some(&doc);
    let written = theirs
        .map(|t| serde_json::to_string_pretty(&t).map(|t| serde_json::json!({ "written": t })))
        .transpose()
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    let tx = db.begin().await?;
    // Another pod converts it: nothing to do here.
    if tx
        .query_one_raw(stmt(
            "SELECT id FROM object_imports WHERE id = $1 FOR UPDATE SKIP LOCKED",
            vec![row.id.into()],
        ))
        .await?
        .is_none()
    {
        return Ok(false);
    }
    let taken = tx
        .query_one_raw(stmt(
            "SELECT 1 AS one FROM pipelines WHERE org_id = $1 AND name = $2",
            vec![row.org_id.into(), row.name.clone().into()],
        ))
        .await?
        .is_some();
    let name = match taken {
        true => format!("{} (import)", row.name),
        false => row.name.clone(),
    };
    let author = (!from_pack).then_some(row.created_by).flatten();
    tx.execute_raw(stmt(
        "INSERT INTO pipelines (id, org_id, name, document, pack, pack_written, recurrence, \
           next_run_at, scheduled_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        vec![
            id.into(),
            row.org_id.into(),
            name.into(),
            document.clone().into(),
            written.as_ref().and(row.pack_key.clone()).into(),
            written.into(),
            row.recurrence.clone().into(),
            row.next_run_at.into(),
            row.recurrence.as_ref().and(author).into(),
        ],
    ))
    .await?;
    tx.execute_raw(stmt(
        "INSERT INTO pipeline_versions (pipeline_id, version, document, author_id) \
         VALUES ($1, 1, $2, $3)",
        vec![id.into(), document.into(), author.into()],
    ))
    .await?;
    let from = diff::source_like(row.id);
    let to = format!("pipeline:{id}");
    for table in [
        "objects",
        "resource_edges",
        "link_versions",
        "market_observations",
    ] {
        tx.execute_raw(stmt(
            &format!("UPDATE {table} SET source_ref = $3 WHERE org_id = $1 AND source_ref LIKE $2"),
            vec![row.org_id.into(), from.clone().into(), to.clone().into()],
        ))
        .await?;
    }
    for sql in [
        "UPDATE datasets SET import_id = NULL WHERE import_id = $1",
        "DELETE FROM data_builds WHERE target_kind = 'import' AND target_id = $1",
        "DELETE FROM object_import_runs WHERE import_id = $1",
        "DELETE FROM object_imports WHERE id = $1",
    ] {
        tx.execute_raw(stmt(sql, vec![row.id.into()])).await?;
    }
    tx.execute_raw(stmt(
        "DELETE FROM ingest_steps WHERE org_id = $1 AND node_id = $2",
        vec![row.org_id.into(), format!("import:{}", row.id).into()],
    ))
    .await?;
    tx.commit().await?;
    if let Some((step, mapping, base, skipped)) = seeded {
        let w = RowsWriter {
            org_id: row.org_id,
            pipeline: &id.to_string(),
            step: &step,
            source_ref: &to,
            author: None,
            reads: &[],
            snapshot: row.mode() == ImportMode::Snapshot,
            dry: false,
        };
        keep_base(db, &w, &mapping, &base, skipped).await?;
    }
    Ok(true)
}

/// The base of the pipeline `id` from the import's last applied run on
/// `table`: its `write: objects` step, the mapping it applies, the plan.
async fn seed(
    db: &DatabaseConnection,
    store: &S3Store,
    row: &ImportRow,
    doc: &Document,
    id: Uuid,
    table: Uuid,
) -> Result<Option<(String, Value, diff::Base, Vec<String>)>> {
    let Some((step, target)) = doc.steps.iter().find_map(|s| match s {
        Step::Write(w) => w.objects.as_ref().map(|o| (w.from.clone(), o)),
        _ => None,
    }) else {
        return Ok(None);
    };
    let Some(last) = db
        .query_one_raw(stmt(
            "SELECT import_version, dataset_transactions AS t, skipped_keys AS s \
             FROM object_import_runs WHERE import_id = $1 AND state = 'applied' \
             AND dataset_transactions IS NOT NULL ORDER BY number DESC LIMIT 1",
            vec![row.id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let version: i32 = last.try_get("", "import_version")?;
    let read: Vec<Uuid> = last.try_get("", "t")?;
    let skipped: Vec<String> = last
        .try_get::<Option<Vec<String>>>("", "s")?
        .unwrap_or_default();
    // Planned on another version of the mapping: it says nothing of this one.
    if version != row.version {
        return Ok(None);
    }
    let files = crate::services::datasets::files_of_transactions(db, row.org_id, table, &read)
        .await
        .map_err(|e| DbErr::Custom(format!("{e:?}")))?;
    let Some(files) = files else {
        return Ok(None);
    };
    let schema = crate::services::ontology::schema(db, row.org_id).await?;
    let Some(mapping) =
        crate::services::pipeline_engine::store::objects_mapping(&id.to_string(), target, &schema)
    else {
        return Ok(None);
    };
    let types: Vec<_> = schema.types.iter().map(|(t, _, _)| t.clone()).collect();
    let links: Vec<_> = schema.links.iter().map(|(l, _, _)| l.clone()).collect();
    let shape = imp::Schema {
        types: &types,
        links: &links,
    };
    let (planned, _) = sync::plan(store, &files, &mapping, &shape, true).await?;
    let json =
        serde_json::to_value::<&Mapping>(&mapping).map_err(|e| DbErr::Custom(e.to_string()))?;
    Ok(Some((step, json, planned.base(), skipped)))
}
