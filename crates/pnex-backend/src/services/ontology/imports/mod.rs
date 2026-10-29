//! Declarative imports into the ontology (partner-watch.md D301, D303).
//!
//! An import = a YAML mapping, a source (file sent with each preview, URL,
//! paginated or per-object API), a mode (`incremental` or `snapshot`) and
//! an optional schedule. A preview computes the difference in the
//! background and keeps it on its run; applying writes exactly that
//! difference, in one transaction, on the versions it read (409 else).
//! Scheduled runs apply without preview but are refused without writing on
//! a changed schema or a suspicious drop (D303). Every fact written carries
//! `import:<import id>@<run number>` (D184). Org from the principal (R1);
//! writes need `can_write`, and the type write roles still apply (D188).

pub mod diff;
pub mod source;
pub mod stop;

use std::time::Duration;

use pnex_core::err_codes::{self, FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use pnex_core::ontology::import::{
    self as imp, ImportApply, ImportDiff, ImportMode, ImportPreview, ImportRunView, ImportSource,
    Mapping, ObjectImportInput, ObjectImportView, MAPPING_MAX_BYTES, PAGES_MAX,
};
use pnex_core::ontology::recurrence::Recurrence;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use super::{invalid, schema, Actor, OntologyError, Result};
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;

/// Entries of each list returned by a preview (the run keeps them all).
pub const PREVIEW_SAMPLE: usize = 500;
/// Imports per org.
pub const IMPORTS_MAX: i64 = 200;
const DEFAULT_PAGES: u32 = 50;
/// A run still `running` after this is considered lost (pod restart).
/// Scheduled runs start at this hour (UTC) of their day.
const RUN_HOUR: u32 = 3;

fn coded(code: &'static str) -> OntologyError {
    OntologyError::Import { code, args: None }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

#[derive(Debug, Clone, FromQueryResult)]
struct ImportRow {
    id: Uuid,
    org_id: i64,
    name: String,
    mapping_yaml: String,
    pack_key: Option<String>,
    source: String,
    source_url: Option<String>,
    mode: String,
    recurrence: Option<Value>,
    next_run_at: Option<DateTimeWithTimeZone>,
    max_pages: Option<i32>,
    version: i32,
    created_by: Option<i64>,
    created_at: DateTimeWithTimeZone,
    updated_at: DateTimeWithTimeZone,
    per_object_cursor: Option<String>,
    /// What its pack last wrote (D424): `{written, replaced}`.
    pack_written: Option<Value>,
}

const IMPORT_COLUMNS: &str = "id, org_id, name, mapping_yaml, pack_key, source, source_url, mode, \
     recurrence, next_run_at, max_pages, version, created_by, created_at, updated_at, \
     per_object_cursor, pack_written";

impl ImportRow {
    fn source(&self) -> ImportSource {
        match self.source.as_str() {
            "url" => ImportSource::Url,
            "api" => ImportSource::Api,
            _ => ImportSource::File,
        }
    }

    fn mode(&self) -> ImportMode {
        match self.mode.as_str() {
            "snapshot" => ImportMode::Snapshot,
            _ => ImportMode::Incremental,
        }
    }

    fn mapping(&self) -> Result<Mapping> {
        parse_mapping(&self.mapping_yaml)
    }

    fn view(&self, last_run: Option<ImportRunView>) -> ObjectImportView {
        ObjectImportView {
            id: self.id.to_string(),
            name: self.name.clone(),
            mapping: self.mapping_yaml.clone(),
            pack_key: self.pack_key.clone(),
            source: self.source(),
            source_url: self.source_url.clone(),
            mode: self.mode(),
            recurrence: self.recurrence.clone(),
            next_run_at: self.next_run_at.map(|t| t.to_rfc3339()),
            max_pages: self.max_pages.map(|p| p as u32),
            version: self.version,
            last_run,
            created_at: self.created_at.to_rfc3339(),
            updated_at: self.updated_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, FromQueryResult)]
struct RunRow {
    id: Uuid,
    number: i32,
    state: String,
    state_code: Option<String>,
    state_args: Option<Value>,
    trigger: String,
    import_version: i32,
    rows: i32,
    counts: Value,
    started_at: DateTimeWithTimeZone,
    finished_at: Option<DateTimeWithTimeZone>,
}

const RUN_COLUMNS: &str =
    "id, number, state, state_code, state_args, trigger, import_version, rows, \
     counts, started_at, finished_at";

impl RunRow {
    fn view(&self) -> ImportRunView {
        ImportRunView {
            id: self.id.to_string(),
            number: self.number,
            state: self.state.clone(),
            state_code: self.state_code.clone(),
            state_args: self.state_args.clone(),
            trigger: self.trigger.clone(),
            import_version: self.import_version,
            rows: self.rows.max(0) as usize,
            counts: serde_json::from_value(self.counts.clone()).unwrap_or_default(),
            started_at: self.started_at.to_rfc3339(),
            finished_at: self.finished_at.map(|t| t.to_rfc3339()),
        }
    }
}

/// Parses a mapping (YAML), bounded before parsing.
pub fn parse_mapping(yaml: &str) -> Result<Mapping> {
    if yaml.len() > MAPPING_MAX_BYTES {
        return Err(invalid(
            "mapping",
            format!("{FIELD_MAX_LENGTH}:{MAPPING_MAX_BYTES}"),
        ));
    }
    serde_saphyr::from_str(yaml).map_err(|_| invalid("mapping", FIELD_INVALID))
}

/// Checks an input against the org schema; returns the parsed mapping.
async fn check_input<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    input: &ObjectImportInput,
) -> Result<Mapping> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(invalid("name", FIELD_REQUIRED));
    }
    if name.chars().count() > 255 {
        return Err(invalid("name", format!("{FIELD_MAX_LENGTH}:255")));
    }
    let m = parse_mapping(&input.mapping)?;
    let s = schema(db, org_id).await?;
    let types: Vec<_> = s.types.iter().map(|(t, _, _)| t.clone()).collect();
    let links: Vec<_> = s.links.iter().map(|(l, _, _)| l.clone()).collect();
    imp::check(
        &m,
        &imp::Schema {
            types: &types,
            links: &links,
        },
    )
    .map_err(|(f, t)| invalid(format!("mapping.{f}"), t))?;
    match (
        input.source,
        input.source_url.as_ref().and_then(|u| u.as_deref()),
    ) {
        (ImportSource::File, _) => {
            // Only steps give a file import something to fetch on schedule.
            if input.recurrence.as_ref().is_some_and(Option::is_some) && m.steps.is_empty() {
                return Err(invalid("recurrence", FIELD_INVALID));
            }
        }
        (_, None) => return Err(invalid("source_url", FIELD_REQUIRED)),
        (_, Some(u)) => {
            source::check_url(u).map_err(|t| invalid("source_url", t))?;
        }
    }
    if m.per_object.is_some() && input.source != ImportSource::Api {
        return Err(invalid("mapping.per_object", FIELD_INVALID));
    }
    // Without a parameter the value goes in place of `{value}` in the URL.
    if m.per_object.as_ref().is_some_and(|p| {
        p.param.is_empty()
            && !input
                .source_url
                .as_ref()
                .and_then(|u| u.as_deref())
                .is_some_and(|u| u.contains(source::VALUE))
    }) {
        return Err(invalid("source_url", FIELD_INVALID));
    }
    // A per-object run covers a share of the objects: a snapshot would
    // close what the others hold.
    if m.per_object.is_some() && input.mode == ImportMode::Snapshot {
        return Err(invalid("mode", FIELD_INVALID));
    }
    if let Some(Some(r)) = &input.recurrence {
        Recurrence::parse(r).ok_or_else(|| invalid("recurrence", FIELD_INVALID))?;
    }
    if input
        .max_pages
        .flatten()
        .is_some_and(|p| p == 0 || p > PAGES_MAX)
    {
        return Err(invalid("max_pages", FIELD_INVALID));
    }
    Ok(m)
}

/// Next scheduled start: the first occurrence day whose run hour is ahead.
fn next_run(recurrence: Option<&Value>) -> Option<DateTimeWithTimeZone> {
    let r = Recurrence::parse(recurrence?)?;
    let now = chrono::Utc::now();
    let today = now.date_naive();
    r.dates(today, today + chrono::Days::new(800), 3)
        .into_iter()
        .filter_map(|d| d.and_hms_opt(RUN_HOUR, 0, 0))
        .map(|t| t.and_utc())
        .find(|t| *t > now)
        .map(Into::into)
}

async fn find_row<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<ImportRow> {
    ImportRow::find_by_statement(stmt(
        &format!("SELECT {IMPORT_COLUMNS} FROM object_imports WHERE id = $1 AND org_id = $2"),
        vec![id.into(), org_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(OntologyError::NotFound)
}

async fn last_run<C: ConnectionTrait>(db: &C, import_id: Uuid) -> Result<Option<RunRow>> {
    Ok(RunRow::find_by_statement(stmt(
        &format!(
            "SELECT {RUN_COLUMNS} FROM object_import_runs WHERE import_id = $1 \
             ORDER BY number DESC LIMIT 1"
        ),
        vec![import_id.into()],
    ))
    .one(db)
    .await?)
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<ObjectImportView>> {
    let rows = ImportRow::find_by_statement(stmt(
        &format!("SELECT {IMPORT_COLUMNS} FROM object_imports WHERE org_id = $1 ORDER BY name"),
        vec![org_id.into()],
    ))
    .all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let last = last_run(db, r.id).await?.map(|l| l.view());
        out.push(r.view(last));
    }
    Ok(out)
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<ObjectImportView> {
    let r = find_row(db, org_id, id).await?;
    let last = last_run(db, r.id).await?.map(|l| l.view());
    Ok(r.view(last))
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    input: &ObjectImportInput,
    pack_key: Option<&str>,
) -> Result<ObjectImportView> {
    check_input(db, org_id, input).await?;
    let count = db
        .query_one_raw(stmt(
            "SELECT count(*)::bigint AS n FROM object_imports WHERE org_id = $1",
            vec![org_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0);
    if count >= IMPORTS_MAX {
        return Err(invalid("name", format!("{FIELD_MAX_LENGTH}:{IMPORTS_MAX}")));
    }
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO object_imports (id, org_id, name, mapping_yaml, pack_key, source, source_url, \
           mode, recurrence, next_run_at, max_pages, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        vec![
            id.into(),
            org_id.into(),
            input.name.trim().into(),
            input.mapping.clone().into(),
            pack_key.map(str::to_string).into(),
            input.source.wire().into(),
            input.source_url.clone().flatten().into(),
            input.mode.wire().into(),
            input.recurrence.clone().flatten().into(),
            next_run(input.recurrence.clone().flatten().as_ref()).into(),
            input.max_pages.flatten().map(|p| p as i32).into(),
            user_id.into(),
        ],
    ))
    .await?;
    get(db, org_id, id).await
}

pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    input: &ObjectImportInput,
) -> Result<ObjectImportView> {
    let Some(expected) = input.expected_version else {
        return Err(invalid("expected_version", FIELD_REQUIRED));
    };
    let row = find_row(db, org_id, id).await?;
    // An absent field keeps its value, `null` clears it.
    let input = &ObjectImportInput {
        source_url: Some(input.source_url.clone().unwrap_or(row.source_url.clone())),
        recurrence: Some(input.recurrence.clone().unwrap_or(row.recurrence.clone())),
        max_pages: Some(input.max_pages.unwrap_or(row.max_pages.map(|p| p as u32))),
        ..input.clone()
    };
    check_input(db, org_id, input).await?;
    let done = db
        .execute_raw(stmt(
            "UPDATE object_imports SET name = $3, mapping_yaml = $4, source = $5, source_url = $6, \
               mode = $7, recurrence = $8, next_run_at = $9, max_pages = $10, \
               version = version + 1, updated_at = now() \
             WHERE id = $1 AND org_id = $2 AND version = $11",
            vec![
                id.into(),
                org_id.into(),
                input.name.trim().into(),
                input.mapping.clone().into(),
                input.source.wire().into(),
                input.source_url.clone().flatten().into(),
                input.mode.wire().into(),
                input.recurrence.clone().flatten().into(),
                next_run(input.recurrence.clone().flatten().as_ref()).into(),
                input.max_pages.flatten().map(|p| p as i32).into(),
                expected.into(),
            ],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(OntologyError::VersionConflict {
            current: row.version,
        });
    }
    get(db, org_id, id).await
}

/// Deletes an import and its run journal; what it wrote stays (A2).
pub async fn delete(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<()> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM object_imports WHERE id = $1 AND org_id = $2",
            vec![id.into(), org_id.into()],
        ))
        .await?;
    if done.rows_affected() != 1 {
        return Err(OntologyError::NotFound);
    }
    Ok(())
}

pub async fn runs(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<Vec<ImportRunView>> {
    find_row(db, org_id, id).await?;
    Ok(RunRow::find_by_statement(stmt(
        &format!(
            "SELECT {RUN_COLUMNS} FROM object_import_runs WHERE import_id = $1 \
             ORDER BY number DESC LIMIT 50"
        ),
        vec![id.into()],
    ))
    .all(db)
    .await?
    .iter()
    .map(RunRow::view)
    .collect())
}

/// A run with its difference (sampled for display).
pub async fn run(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    run_id: Uuid,
) -> Result<ImportPreview> {
    find_row(db, org_id, id).await?;
    let row = load_row(db, id, run_id).await?;
    let diff = load_diff(db, run_id, Some(PREVIEW_SAMPLE)).await?;
    let steps = crate::services::ingest::run_steps(db, org_id, id, run_id).await?;
    Ok(ImportPreview {
        run: row.view(),
        diff: diff.unwrap_or_default(),
        steps,
    })
}

async fn load_row<C: ConnectionTrait>(db: &C, import_id: Uuid, run_id: Uuid) -> Result<RunRow> {
    RunRow::find_by_statement(stmt(
        &format!("SELECT {RUN_COLUMNS} FROM object_import_runs WHERE id = $1 AND import_id = $2"),
        vec![run_id.into(), import_id.into()],
    ))
    .one(db)
    .await?
    .ok_or(OntologyError::NotFound)
}

/// A run's difference, each list cut to `sample` entries by Postgres: a
/// preview of tens of thousands of rows weighs tens of MB, and the run is
/// polled while it applies.
async fn load_diff<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    sample: Option<usize>,
) -> Result<Option<ImportDiff>> {
    let column = match sample {
        None => "diff".to_string(),
        Some(n) => {
            let lists = [
                "created",
                "updated",
                "closed",
                "links_opened",
                "links_updated",
                "errors",
                "warnings",
            ]
            .map(|k| {
                format!(
                    "'{k}', jsonb_path_query_array(diff, '$.{k}[0 to {}]')",
                    n.saturating_sub(1)
                )
            })
            .join(", ");
            format!("jsonb_build_object('rows', diff -> 'rows', {lists})")
        }
    };
    Ok(db
        .query_one_raw(stmt(
            &format!(
                "SELECT CASE WHEN diff IS NULL THEN NULL ELSE {column} END AS diff \
                 FROM object_import_runs WHERE id = $1"
            ),
            vec![run_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<Option<Value>>("", "diff").ok().flatten())
        .and_then(|d| serde_json::from_value(d).ok()))
}

/// Opens a run (`running`) and returns it.
async fn open_run<C: ConnectionTrait>(
    db: &C,
    imp: &ImportRow,
    trigger: &str,
    user_id: Option<i64>,
) -> Result<RunRow> {
    let id = Uuid::new_v4();
    // The number is taken under the unique (import, number) constraint.
    db.execute_raw(stmt(
        "INSERT INTO object_import_runs (id, org_id, import_id, number, state, trigger, \
           import_version, created_by) \
         SELECT $1, $2, $3, coalesce(max(number), 0) + 1, 'running', $4, $5, $6 \
         FROM object_import_runs WHERE import_id = $3",
        vec![
            id.into(),
            imp.org_id.into(),
            imp.id.into(),
            trigger.into(),
            imp.version.into(),
            user_id.into(),
        ],
    ))
    .await?;
    load_row(db, imp.id, id).await
}

async fn finish_run<C: ConnectionTrait>(
    db: &C,
    run_id: Uuid,
    state: &str,
    code: Option<&str>,
    args: &Value,
    diff: Option<&ImportDiff>,
    keep_diff: bool,
) -> Result<()> {
    let counts = diff.map(ImportDiff::counts).unwrap_or_default();
    db.execute_raw(stmt(
        "UPDATE object_import_runs SET state = $2, state_code = $3, rows = $4, counts = $5, \
           diff = $6, state_args = $7, \
           finished_at = CASE WHEN $2 = 'preview' THEN NULL ELSE now() END, \
           heartbeat_at = now() \
         WHERE id = $1",
        vec![
            run_id.into(),
            state.into(),
            code.map(str::to_string).into(),
            (diff.map_or(0, |d| d.rows) as i32).into(),
            serde_json::to_value(&counts).unwrap_or_default().into(),
            diff.map(|d| {
                if keep_diff {
                    serde_json::to_value(d)
                } else {
                    // Applied or failed: the rows in error stay readable,
                    // the rest of the difference is not kept.
                    serde_json::to_value(ImportDiff {
                        rows: d.rows,
                        errors: d.errors.iter().take(PREVIEW_SAMPLE).cloned().collect(),
                        warnings: d.warnings.iter().take(PREVIEW_SAMPLE).cloned().collect(),
                        ..Default::default()
                    })
                }
                .unwrap_or_default()
            })
            .into(),
            (!args.is_null()).then(|| args.clone()).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Records of the import's source (`file` = the bytes sent).
async fn records<C: ConnectionTrait>(
    db: &C,
    imp: &ImportRow,
    m: &Mapping,
    file: Option<(&[u8], &str)>,
) -> std::result::Result<Vec<Value>, (&'static str, Value)> {
    let coded = |code: &'static str| (code, Value::Null);
    match (imp.source(), file) {
        // Hostile input (R8 §5): parsed on a blocking thread, where a parser
        // panic only fails this run.
        (ImportSource::File, Some((bytes, name))) => {
            let (bytes, name, m) = (bytes.to_vec(), name.to_string(), m.clone());
            tokio::task::spawn_blocking(move || {
                pnex_core::import_read::read_records(&bytes, &name, &m)
                    .map_err(source::read_refusal)
            })
            .await
            .unwrap_or(Err(coded(source::UNREADABLE)))
        }
        (ImportSource::File, None) => Err(coded(source::UNREADABLE)),
        (ImportSource::Url, _) => {
            source::fetch_file(imp.source_url.as_deref().unwrap_or_default(), m)
                .await
                .map_err(coded)
        }
        (ImportSource::Api, _) => {
            let url = imp.source_url.as_deref().unwrap_or_default();
            let pages = imp.max_pages.map_or(DEFAULT_PAGES, |p| p as u32);
            match &m.per_object {
                None => source::fetch_api(url, m, pages).await.map_err(coded),
                Some(p) => {
                    let values = per_object_values(db, imp, p, pages)
                        .await
                        .map_err(|_| coded(source::UNREACHABLE))?;
                    source::fetch_per_object(url, m, &values)
                        .await
                        .map_err(coded)
                }
            }
        }
    }
}

/// Values of `per_object` to query: in value order, starting after the
/// last one a scheduled run queried and wrapping around, so every object
/// gets its turn even when the source knows nothing about most of them.
async fn per_object_values<C: ConnectionTrait>(
    db: &C,
    imp: &ImportRow,
    p: &imp::PerObject,
    max: u32,
) -> std::result::Result<Vec<String>, sea_orm::DbErr> {
    let (codes, code_prop) = match &p.setting {
        Some(st) => (
            super::packs::setting_value(db, imp.org_id, &st.key)
                .await?
                .as_ref()
                .and_then(Value::as_str)
                .map(imp::PerObjectSetting::codes)
                .unwrap_or_default(),
            Some(st.property.clone()),
        ),
        None => (Vec::new(), None),
    };
    let rows = db
        .query_all_raw(stmt(
            "SELECT properties ->> $3 AS v FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL AND properties ? $3 \
               AND ($6::text IS NULL OR properties ->> $6 = ANY($7)) \
               AND (cardinality($9::text[]) = 0 OR cardinality($8::text[]) > 0 \
                 OR upper(properties ->> $10) = ANY($9)) \
               AND (cardinality($8::text[]) = 0 OR EXISTS (SELECT 1 FROM resource_edges e \
                 WHERE e.org_id = $1 AND e.relation = ANY($8) \
                   AND (e.source_object_id = objects.id OR e.target_object_id = objects.id) \
                   AND e.valid_from <= now() AND (e.valid_to IS NULL OR e.valid_to > now()) \
                   AND (cardinality($9::text[]) = 0 OR EXISTS (SELECT 1 FROM objects o \
                     WHERE o.org_id = $1 AND o.valid_to IS NULL \
                       AND o.id = CASE WHEN e.source_object_id = objects.id \
                         THEN e.target_object_id ELSE e.source_object_id END \
                       AND upper(o.properties ->> $10) = ANY($9))))) \
             ORDER BY (properties ->> $3) <= coalesce($5, ''), properties ->> $3 LIMIT $4",
            vec![
                imp.org_id.into(),
                p.type_key.clone().into(),
                p.property.clone().into(),
                (max as i64).into(),
                imp.per_object_cursor.clone().into(),
                p.only.as_ref().map(|o| o.property.clone()).into(),
                p.only
                    .as_ref()
                    .map(|o| o.values.clone())
                    .unwrap_or_default()
                    .into(),
                p.linked.clone().into(),
                codes.into(),
                code_prop.into(),
            ],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String>("", "v").ok())
        .filter(|v| !v.trim().is_empty())
        .collect())
}

/// Number of the first record as the user sees it: line 2 of a sheet or a
/// CSV (line 1 holds the headers), else 1.
fn first_row(m: &Mapping, name: &str) -> usize {
    let fmt = m.format.or_else(|| imp::SourceFormat::of_name(name));
    match fmt {
        Some(imp::SourceFormat::Csv | imp::SourceFormat::Xlsx) => 2,
        _ => 1,
    }
}

/// Starts a preview: the run is `running`, its difference is computed in
/// the background (poll `run`). A file source needs the file.
pub async fn dry_run(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    id: Uuid,
    file: Option<(Vec<u8>, String)>,
) -> Result<ImportRunView> {
    let imp = find_row(db, org_id, id).await?;
    if imp.source() == ImportSource::File && file.as_ref().is_none_or(|(b, _)| b.is_empty()) {
        return Err(invalid("file", FIELD_REQUIRED));
    }
    let m = imp.mapping()?;
    if stop::stopping() {
        return Err(coded(err_codes::IMPORT_SERVER_STOPPING));
    }
    let run = open_run(db, &imp, "preview", user_id).await?;
    let view = run.view();
    let db = db.clone();
    tokio::spawn(async move {
        let _held = stop::hold(run.id);
        let name = file.as_ref().map(|(_, n)| n.clone()).unwrap_or_default();
        let outcome = match records(
            &db,
            &imp,
            &m,
            file.as_ref().map(|(b, n)| (b.as_slice(), n.as_str())),
        )
        .await
        {
            Err(code) => Err(code),
            Ok(rows) => {
                let s = schema(&db, imp.org_id).await;
                match s {
                    Err(_) => Err((err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null)),
                    Ok(s) => diff::compute(
                        &db,
                        imp.org_id,
                        imp.id,
                        &s,
                        &m,
                        imp.mode() == ImportMode::Snapshot,
                        &rows,
                        first_row(&m, &name),
                    )
                    .await
                    .map_err(|e| match e {
                        OntologyError::Import { code, .. } => (code, Value::Null),
                        e => {
                            tracing::warn!(import = %imp.id, error = %e, "import preview failed");
                            (err_codes::IMPORT_SOURCE_UNREADABLE, Value::Null)
                        }
                    }),
                }
            }
        };
        let r = match &outcome {
            Ok(d) => finish_run(&db, run.id, "preview", None, &Value::Null, Some(d), true).await,
            Err((code, args)) => {
                finish_run(&db, run.id, "failed", Some(code), args, None, false).await
            }
        };
        if let Err(e) = r {
            tracing::warn!(run = %run.id, error = %e, "import run not recorded");
        }
    });
    Ok(view)
}

/// Applies a preview: its difference, on the import version and the
/// object versions it read. Runs in the background (poll `run`).
pub async fn apply(
    db: &DatabaseConnection,
    org_id: i64,
    actor_user: Option<i64>,
    role: OrgMemberRole,
    id: Uuid,
    input: &ImportApply,
) -> Result<ImportRunView> {
    let imp = find_row(db, org_id, id).await?;
    let run_id = Uuid::parse_str(&input.run_id)
        .map_err(|_| invalid("run_id", err_codes::FIELD_INVALID_UUID))?;
    let run = load_row(db, id, run_id).await?;
    let diff = load_diff(db, run_id, None).await?;
    if imp.version != input.expected_version {
        return Err(OntologyError::VersionConflict {
            current: imp.version,
        });
    }
    let Some(diff) = diff.filter(|_| run.state == "preview" && run.import_version == imp.version)
    else {
        return Err(coded(err_codes::IMPORT_RUN_STALE));
    };
    if stop::stopping() {
        return Err(coded(err_codes::IMPORT_SERVER_STOPPING));
    }
    if !diff.errors.is_empty() {
        return Err(coded(err_codes::IMPORT_HAS_ERRORS));
    }
    // Claim the preview: a second apply of the same run finds it taken.
    let claimed = db
        .execute_raw(stmt(
            "UPDATE object_import_runs SET state = 'running', heartbeat_at = now() \
             WHERE id = $1 AND state = 'preview'",
            vec![run_id.into()],
        ))
        .await?;
    if claimed.rows_affected() != 1 {
        return Err(coded(err_codes::IMPORT_RUN_STALE));
    }
    let actor = Actor {
        user_id: actor_user,
        role,
        source_ref: format!("import:{}@{}", imp.id, run.number),
    };
    let view = ImportRunView {
        state: "running".into(),
        ..run.view()
    };
    let db = db.clone();
    tokio::spawn(async move {
        let _held = stop::hold(run.id);
        let outcome = write_locked(&db, &imp, &actor, &diff).await;
        let (state, code) = match &outcome {
            Ok(()) => ("applied", None),
            Err(OntologyError::VersionConflict { .. }) => {
                ("stale", Some(err_codes::IMPORT_RUN_STALE))
            }
            Err(e) => {
                tracing::warn!(import = %imp.id, error = %e, "import apply failed");
                ("failed", Some(error_code(e)))
            }
        };
        if let Err(e) = finish_run(&db, run.id, state, code, &Value::Null, Some(&diff), false).await
        {
            tracing::warn!(run = %run.id, error = %e, "import run not recorded");
        }
        drop(diff);
        release_memory();
    });
    Ok(view)
}

/// Gives the heap a large run freed back to the system: glibc keeps it
/// otherwise, and the pod stays at the run's peak.
pub fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only returns the allocator's free pages.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Machine code journaled for a failed application.
fn error_code(e: &OntologyError) -> &'static str {
    match e {
        OntologyError::Import { code, .. } => code,
        OntologyError::KeyTaken | OntologyError::VersionConflict { .. } => {
            err_codes::IMPORT_RUN_STALE
        }
        OntologyError::WriteForbidden => err_codes::ONTOLOGY_WRITE_FORBIDDEN,
        OntologyError::LinkNotAllowed => err_codes::ONTOLOGY_LINK_NOT_ALLOWED,
        OntologyError::LinkCardinality => err_codes::ONTOLOGY_LINK_CARDINALITY,
        _ => err_codes::IMPORT_SOURCE_UNREADABLE,
    }
}

/// One application per org at a time.
async fn write_locked(
    db: &DatabaseConnection,
    imp: &ImportRow,
    actor: &Actor,
    diff: &ImportDiff,
) -> Result<()> {
    use crate::services::db_lock::{ns, TenantLock};
    let lock = TenantLock::acquire(
        db,
        ns::ONTOLOGY_IMPORT,
        imp.org_id,
        Duration::from_secs(600),
    )
    .await?;
    let out = write(db, imp.org_id, actor, diff).await;
    lock.release().await;
    // D431: the watched objects it updated count a check run.
    if out.is_ok() {
        super::cases::watch::record_import(db, imp.org_id, actor, &imp.name, &diff.updated).await;
    }
    out
}

/// Writes a difference in one transaction: all or nothing.
async fn write(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    diff: &ImportDiff,
) -> Result<()> {
    crate::services::ontology::changes::deferred(async {
        use super::{links, objects};
        let txn = db.begin().await?;
        // Loaded once: per write it costs a query per object type.
        let s = schema(&txn, org_id).await?;
        // Created meanwhile by someone else: the preview is stale.
        if keys_taken(&txn, org_id, &diff.created).await? {
            return Err(OntologyError::KeyTaken);
        }
        // Bulk paths (D347): a few statements per batch, not per row. A
        // `map_pin` goes through its POI, one by one.
        let mut created: Vec<Uuid> = vec![Uuid::nil(); diff.created.len()];
        let (mut at, mut inputs) = (Vec::new(), Vec::new());
        for (i, c) in diff.created.iter().enumerate() {
            if c.type_key == pnex_core::resources::KIND_MAP_PIN {
                created[i] = create_pin(&txn, &s, org_id, actor, c).await?;
            } else {
                at.push(i);
                inputs.push(ObjectInput {
                    type_key: c.type_key.clone(),
                    title: c.title.clone(),
                    properties: c.properties.clone(),
                    expected_version: None,
                });
            }
        }
        let ids = objects::create_many_with(&txn, &s, org_id, actor, &inputs).await?;
        drop(inputs);
        for (i, id) in at.into_iter().zip(ids) {
            created[i] = id;
        }
        let mut current = std::collections::HashMap::new();
        for chunk in diff.updated.chunks(objects::BULK_CHUNK) {
            let ids = chunk
                .iter()
                .map(|u| Uuid::parse_str(&u.id).map_err(|_| OntologyError::NotFound))
                .collect::<Result<Vec<_>>>()?;
            current.extend(links::refs(&txn, org_id, ids).await?);
        }
        let mut updates = Vec::with_capacity(diff.updated.len());
        for u in &diff.updated {
            let id = Uuid::parse_str(&u.id).map_err(|_| OntologyError::NotFound)?;
            let row = current.get(&id).ok_or(OntologyError::NotFound)?;
            if row.version != u.version || row.valid_to.is_some() {
                return Err(OntologyError::VersionConflict {
                    current: row.version,
                });
            }
            let mut props = row.properties.as_object().cloned().unwrap_or_default();
            for (k, ch) in &u.changes {
                props.insert(k.clone(), ch.to.clone());
            }
            updates.push((
                id,
                ObjectInput {
                    type_key: row.type_key.clone(),
                    title: u.new_title.clone().unwrap_or(row.title.clone()),
                    properties: props,
                    expected_version: Some(u.version),
                },
            ));
        }
        objects::update_many_with(&txn, &s, org_id, actor, &updates).await?;
        let end = |e: &imp::DiffEnd| -> Result<String> {
            match (&e.id, e.created) {
                (Some(id), _) => Ok(id.clone()),
                (None, Some(i)) => created
                    .get(i)
                    .map(Uuid::to_string)
                    .ok_or(OntologyError::NotFound),
                _ => Err(OntologyError::NotFound),
            }
        };
        let now = chrono::Utc::now().to_rfc3339();
        // Proposed links wait for a human (proposals.md D375).
        for l in diff.links_opened.iter().filter(|l| l.propose) {
            let input = pnex_core::ontology::proposal::ProposalInput {
                kind: Some(pnex_core::ontology::proposal::ProposalKind::LinkCreate),
                type_key: l.link_type.clone(),
                source_id: Some(end(&l.from)?),
                link_target_id: Some(end(&l.to)?),
                values: l.attributes.clone(),
                origin: actor.source_ref.clone(),
                ..Default::default()
            };
            super::proposals::propose(&txn, org_id, &input).await?;
        }
        let opened = diff
            .links_opened
            .iter()
            .filter(|l| !l.propose)
            .map(|l| {
                Ok(LinkInput {
                    link_type: l.link_type.clone(),
                    source_id: end(&l.from)?,
                    target_id: end(&l.to)?,
                    attributes: l.attributes.clone(),
                    // A start in the future is refused: such a link opens now.
                    valid_from: l.valid_from.clone().filter(|v| v.as_str() <= now.as_str()),
                    valid_to: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        links::create_many_with(&txn, &s, org_id, actor, &opened).await?;
        let changed: Vec<(i64, serde_json::Map<String, Value>)> = diff
            .links_updated
            .iter()
            .map(|l| (l.id, l.attributes.clone()))
            .collect();
        links::update_many_with(&txn, &s, org_id, actor, &changed).await?;
        for c in &diff.closed {
            match c.kind.as_str() {
                "object" => {
                    let id = Uuid::parse_str(&c.id).map_err(|_| OntologyError::NotFound)?;
                    objects::archive(&txn, org_id, actor, id).await?;
                }
                _ => {
                    let id: i64 = c.id.parse().map_err(|_| OntologyError::NotFound)?;
                    links::close(&txn, org_id, actor, id, None).await?;
                }
            }
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        Ok(())
    })
    .await
}

/// True when a live object already holds the key of one to create; one
/// query per type and batch of keys.
async fn keys_taken<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    created: &[imp::DiffCreate],
) -> Result<bool> {
    let mut groups: std::collections::BTreeMap<(&str, &str), Vec<String>> = Default::default();
    for c in created {
        groups
            .entry((c.type_key.as_str(), c.key_prop.as_str()))
            .or_default()
            .push(c.key.clone());
    }
    for ((type_key, key_prop), keys) in groups {
        for chunk in keys.chunks(1_000) {
            let hit = db
                .query_one_raw(stmt(
                    "SELECT 1 AS hit FROM objects WHERE org_id = $1 AND type_key = $2 \
                       AND valid_to IS NULL AND properties ->> $3 = ANY($4) LIMIT 1",
                    vec![
                        org_id.into(),
                        type_key.into(),
                        key_prop.into(),
                        chunk.to_vec().into(),
                    ],
                ))
                .await?;
            if hit.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A `map_pin` establishment: the POI first (its identity row follows by
/// trigger), then its pack properties on the identity.
async fn create_pin<C: ConnectionTrait>(
    db: &C,
    s: &super::OrgSchema,
    org_id: i64,
    actor: &Actor,
    c: &imp::DiffCreate,
) -> Result<Uuid> {
    use crate::services::pois::{create_pin, NewPin};
    let [lat, lon] = c
        .location
        .ok_or_else(|| invalid("location", FIELD_REQUIRED))?;
    let pin = create_pin(
        db,
        org_id,
        NewPin {
            label: &c.title,
            emoji: None,
            location_detail: None,
            latitude: lat,
            longitude: lon,
            metadata: None,
        },
    )
    .await
    .map_err(|_| invalid("location", FIELD_INVALID))?;
    let identity = super::objects::find_by_native(
        db,
        org_id,
        pnex_core::resources::KIND_MAP_PIN,
        &pin.id.to_string(),
    )
    .await?;
    super::objects::update_in_with(
        db,
        s,
        org_id,
        actor,
        identity.id,
        &ObjectInput {
            type_key: c.type_key.clone(),
            title: identity.title.clone(),
            properties: c.properties.clone(),
            expected_version: Some(identity.version),
        },
    )
    .await?;
    Ok(identity.id)
}

// ───────────────────────────── schedule (D303) ─────────────────────────────

/// Role of the import's author today: scheduled runs write with it (R2);
/// an author who left the org (or a viewer) stops the schedule.
async fn author_role<C: ConnectionTrait>(db: &C, imp: &ImportRow) -> Option<OrgMemberRole> {
    use crate::models::_entities::organization_members as om;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let user = imp.created_by?;
    om::Entity::find()
        .filter(om::Column::OrgId.eq(imp.org_id))
        .filter(om::Column::UserId.eq(user))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|m| m.role)
        .filter(|r| {
            matches!(
                r,
                OrgMemberRole::Owner | OrgMemberRole::Admin | OrgMemberRole::Member
            )
        })
}

/// Records of the previous applied run (for the drop check).
async fn previous_rows<C: ConnectionTrait>(db: &C, import_id: Uuid) -> Result<usize> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT rows FROM object_import_runs WHERE import_id = $1 AND state = 'applied' \
             ORDER BY number DESC LIMIT 1",
            vec![import_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i32>("", "rows").ok())
        .unwrap_or(0)
        .max(0) as usize)
}

/// Starts a run in the background and returns it `running` (manual
/// launch or schedule): an import with its own steps runs them, then
/// applies the file they staged (data-pipelines.md D344); a URL or API
/// import fetches its source. Checked and applied with the author's role
/// (D303). A file import has nothing to fetch: it is previewed with a file.
pub async fn start(
    db: &DatabaseConnection,
    store: Option<crate::services::artifact_store::S3Store>,
    org_id: i64,
    id: Uuid,
    user_id: Option<i64>,
    trigger: &str,
) -> Result<ImportRunView> {
    let imp = find_row(db, org_id, id).await?;
    let m = imp.mapping()?;
    if m.steps.is_empty() && imp.source() == ImportSource::File {
        return Err(coded(err_codes::INGEST_ENTRY_MISSING));
    }
    if stop::stopping() {
        return Err(coded(err_codes::IMPORT_SERVER_STOPPING));
    }
    let run = open_run(db, &imp, trigger, user_id).await?;
    let view = run.view();
    let db = db.clone();
    tokio::spawn(async move {
        let _held = stop::hold(run.id);
        let work = async {
            if m.steps.is_empty() {
                let outcome = scheduled(&db, &imp, &m, run.number).await;
                let (state, code, args, diff) = match outcome {
                    Ok(d) => ("applied", None, Value::Null, Some(d)),
                    Err((state, code, args, d)) => {
                        notify_refused(&db, &imp, code, args.clone()).await;
                        (state, Some(code), args, d.map(|d| *d))
                    }
                };
                finish_run(&db, run.id, state, code, &args, diff.as_ref(), false).await?;
                return Ok(());
            }
            let Some(store) = store else {
                let code = err_codes::INGEST_STAGING_UNAVAILABLE;
                return fail_run(&db, run.id, code, Value::Null).await;
            };
            let steps = crate::services::ingest::pipeline::run(
                &db, &store, imp.org_id, imp.id, run.id, &m.steps,
            )
            .await;
            match steps {
                Ok(file) => {
                    let pct = pnex_core::ingest::MAX_CLOSURES_PCT;
                    run_staged(&db, imp.org_id, imp.id, run.id, Some(file), true, pct)
                        .await
                        .map(|_| ())
                }
                Err((code, args)) => fail_run(&db, run.id, &code, args).await,
            }
        };
        // The run beats while it works: a pod gone leaves it stale, not running.
        let beat = async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                beat(&db, run.id).await;
            }
        };
        let out: Result<()> = tokio::select! {
            out = work => out,
            _ = beat => Ok(()),
        };
        release_memory();
        if let Err(e) = out {
            tracing::warn!(import = %imp.id, run = %run.id, error = %e, "import run failed");
            let code = err_codes::IMPORT_SOURCE_UNREADABLE;
            let _ = fail_run(&db, run.id, code, Value::Null).await;
        }
    });
    Ok(view)
}

/// What a run on staged steps got.
pub struct StagedOutcome {
    pub run: ImportRunView,
    pub counts: imp::ImportCounts,
    /// Refusal or failure code; the run is journaled either way.
    pub code: Option<&'static str>,
    pub args: Value,
}

/// Live objects and links this import wrote (provenance `import:<id>@…`).
async fn held<C: ConnectionTrait>(db: &C, org_id: i64, import_id: Uuid) -> Result<usize> {
    let like = format!("import:{import_id}@%");
    let row = db
        .query_one_raw(stmt(
            "SELECT (SELECT count(*) FROM objects WHERE org_id = $1 AND valid_to IS NULL \
               AND source_ref LIKE $2) + (SELECT count(*) FROM resource_edges \
               WHERE org_id = $1 AND valid_to IS NULL AND source_ref LIKE $2) AS n",
            vec![org_id.into(), like.into()],
        ))
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0)
        .max(0) as usize)
}

/// A run on the file its steps staged (D344): checked like a scheduled run
/// (D303), refused when it would close more than `max_closures_pct` % of
/// what the import holds, then kept as a preview (applicable from the
/// Imports tab) or applied with the author's role.
pub async fn run_staged(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    run_id: Uuid,
    file: Option<(Vec<u8>, String)>,
    apply: bool,
    max_closures_pct: u32,
) -> Result<StagedOutcome> {
    let imp = find_row(db, org_id, id).await?;
    let run = load_row(db, imp.id, run_id).await?;
    let m = match imp.mapping() {
        Ok(m) => m,
        Err(e) => {
            let code = Some(error_code(&e));
            finish_run(db, run.id, "failed", code, &Value::Null, None, false).await?;
            return Err(e);
        }
    };
    let name = file.as_ref().map(|(_, n)| n.clone()).unwrap_or_default();
    let rows = match (&file, &m.per_object, imp.source()) {
        (Some((bytes, name)), None, _) => {
            let (bytes, name, m2) = (bytes.clone(), name.clone(), m.clone());
            tokio::task::spawn_blocking(move || {
                pnex_core::import_read::read_records(&bytes, &name, &m2)
                    .map_err(source::read_refusal)
            })
            .await
            .unwrap_or(Err((source::UNREADABLE, Value::Null)))
        }
        (None, None, ImportSource::File) => Err((err_codes::INGEST_ENTRY_MISSING, Value::Null)),
        _ => records(db, &imp, &m, None).await,
    };
    let end = |state: &'static str,
               code: Option<&'static str>,
               args: Value,
               d: Option<ImportDiff>,
               keep: bool| {
        let run_id = run.id;
        let imp = imp.clone();
        async move {
            finish_run(db, run_id, state, code, &args, d.as_ref(), keep).await?;
            if state == "refused" {
                notify_refused(db, &imp, code.unwrap_or_default(), args.clone()).await;
            }
            let r = load_row(db, imp.id, run_id).await?;
            Ok::<_, OntologyError>(StagedOutcome {
                run: r.view(),
                counts: d.as_ref().map(ImportDiff::counts).unwrap_or_default(),
                code,
                args,
            })
        }
    };
    let rows = match rows {
        Ok(r) => r,
        Err((code, args)) => return end("failed", Some(code), args, None, false).await,
    };
    progress(db, run.id, rows.len(), None).await;
    let previous = previous_rows(db, imp.id).await.unwrap_or(0);
    let missing = imp::missing_columns(&m, &rows);
    // A per-object batch may find nothing: not a lost source.
    let lost = rows.is_empty() && previous > 0 && m.per_object.is_none();
    if lost || (!rows.is_empty() && !missing.is_empty()) {
        let columns = if missing.is_empty() {
            "—".to_string()
        } else {
            missing.join(", ")
        };
        let args = serde_json::json!({ "columns": columns });
        return end(
            "refused",
            Some(err_codes::IMPORT_SCHEMA_CHANGED),
            args,
            None,
            false,
        )
        .await;
    }
    let snapshot = imp.mode() == ImportMode::Snapshot;
    let s = match schema(db, imp.org_id).await {
        Ok(s) => s,
        Err(_) => return end("failed", Some(source::UNREADABLE), Value::Null, None, false).await,
    };
    let first = first_row(&m, &name);
    let mut d = match diff::compute(db, imp.org_id, imp.id, &s, &m, snapshot, &rows, first).await {
        Ok(d) => d,
        Err(OntologyError::Import { code, .. }) => {
            return end("failed", Some(code), Value::Null, None, false).await
        }
        Err(_) => return end("failed", Some(source::UNREADABLE), Value::Null, None, false).await,
    };
    // The records are planned: only the difference is written.
    drop(rows);
    progress(db, run.id, d.rows, Some(&d)).await;
    if snapshot && !d.errors.is_empty() {
        return end(
            "refused",
            Some(err_codes::IMPORT_HAS_ERRORS),
            Value::Null,
            Some(d),
            true,
        )
        .await;
    }
    let holding = held(db, imp.org_id, imp.id).await?;
    if holding > 0 && d.closed.len() * 100 > holding * max_closures_pct as usize {
        let args = serde_json::json!({
            "closed": d.closed.len().to_string(),
            "held": holding.to_string(),
            "max_pct": max_closures_pct.to_string(),
        });
        let code = Some(err_codes::INGEST_SUSPICIOUS_CLOSURES);
        return end("refused", code, args, Some(d), true).await;
    }
    if !apply {
        return end("preview", None, Value::Null, Some(d), true).await;
    }
    let Some(role) = author_role(db, &imp).await else {
        let code = Some(err_codes::ONTOLOGY_WRITE_FORBIDDEN);
        return end("refused", code, Value::Null, None, false).await;
    };
    let actor = Actor {
        user_id: imp.created_by,
        role,
        source_ref: format!("import:{}@{}", imp.id, run.number),
    };
    // Incremental: the valid records go, the bad ones stay in the journal.
    let errors = std::mem::take(&mut d.errors);
    let written = write_locked(db, &imp, &actor, &d).await;
    d.errors = errors;
    match written {
        Ok(()) => end("applied", None, Value::Null, Some(d), false).await,
        Err(e) => {
            tracing::warn!(import = %imp.id, error = %e, "staged import failed");
            end("failed", Some(error_code(&e)), Value::Null, Some(d), false).await
        }
    }
}

/// What a running run knows so far: its records, then the planned
/// counts (the write itself is one transaction, all or nothing).
async fn progress(db: &DatabaseConnection, run_id: Uuid, rows: usize, diff: Option<&ImportDiff>) {
    let counts = diff.map(ImportDiff::counts).unwrap_or_default();
    let r = db
        .execute_raw(stmt(
            "UPDATE object_import_runs SET rows = $2, counts = $3, heartbeat_at = now() \
             WHERE id = $1 AND state = 'running'",
            vec![
                run_id.into(),
                (rows as i32).into(),
                serde_json::to_value(&counts).unwrap_or_default().into(),
            ],
        ))
        .await;
    if let Err(e) = r {
        tracing::warn!(run = %run_id, error = %e, "import progress not recorded");
    }
}

/// Keeps a run alive while its server works on it.
pub async fn beat(db: &DatabaseConnection, run_id: Uuid) {
    let r = db
        .execute_raw(stmt(
            "UPDATE object_import_runs SET heartbeat_at = now() WHERE id = $1 AND state = 'running'",
            vec![run_id.into()],
        ))
        .await;
    if let Err(e) = r {
        tracing::warn!(run = %run_id, error = %e, "import heartbeat not recorded");
    }
}

/// Fails a run whose steps or file could not be read; a run that
/// finished meanwhile keeps its outcome.
pub async fn fail_run(
    db: &DatabaseConnection,
    run_id: Uuid,
    code: &str,
    args: Value,
) -> Result<()> {
    db.execute_raw(stmt(
        "UPDATE object_import_runs SET state = 'failed', state_code = $2, state_args = $3, \
           finished_at = now(), heartbeat_at = now() WHERE id = $1 AND state = 'running'",
        vec![
            run_id.into(),
            code.into(),
            (!args.is_null()).then_some(args).into(),
        ],
    ))
    .await?;
    Ok(())
}

type Refusal = (&'static str, &'static str, Value, Option<Box<ImportDiff>>);

async fn scheduled(
    db: &DatabaseConnection,
    imp: &ImportRow,
    m: &Mapping,
    number: i32,
) -> std::result::Result<ImportDiff, Refusal> {
    let refused = |code, args, d| Err(("refused", code, args, d));
    let Some(role) = author_role(db, imp).await else {
        return refused(err_codes::ONTOLOGY_WRITE_FORBIDDEN, Value::Null, None);
    };
    let rows = match records(db, imp, m, None).await {
        Ok(r) => r,
        Err((code, args)) => return Err(("failed", code, args, None)),
    };
    let previous = previous_rows(db, imp.id).await.unwrap_or(0);
    let missing = imp::missing_columns(m, &rows);
    // D303: an unexpected shape writes nothing.
    // A per-object batch may find nothing: not a lost source.
    let lost = rows.is_empty() && previous > 0 && m.per_object.is_none();
    if lost || (!rows.is_empty() && !missing.is_empty()) {
        let columns = if missing.is_empty() {
            "—".to_string()
        } else {
            missing.join(", ")
        };
        return refused(
            err_codes::IMPORT_SCHEMA_CHANGED,
            serde_json::json!({ "columns": columns }),
            None,
        );
    }
    let snapshot = imp.mode() == ImportMode::Snapshot;
    if snapshot && imp::suspicious_drop(previous, rows.len()) {
        return refused(
            err_codes::IMPORT_SUSPICIOUS_DROP,
            serde_json::json!({ "previous": previous.to_string(), "rows": rows.len().to_string() }),
            None,
        );
    }
    let s = match schema(db, imp.org_id).await {
        Ok(s) => s,
        Err(_) => {
            return Err((
                "failed",
                err_codes::IMPORT_SOURCE_UNREADABLE,
                Value::Null,
                None,
            ))
        }
    };
    let mut d = match diff::compute(db, imp.org_id, imp.id, &s, m, snapshot, &rows, 1).await {
        Ok(d) => d,
        Err(OntologyError::Import { code, .. }) => return Err(("failed", code, Value::Null, None)),
        Err(_) => {
            return Err((
                "failed",
                err_codes::IMPORT_SOURCE_UNREADABLE,
                Value::Null,
                None,
            ))
        }
    };
    // A snapshot with bad records would close what they hold: refused.
    if snapshot && !d.errors.is_empty() {
        return refused(err_codes::IMPORT_HAS_ERRORS, Value::Null, Some(Box::new(d)));
    }
    let actor = Actor {
        user_id: imp.created_by,
        role,
        source_ref: format!("import:{}@{number}", imp.id),
    };
    // Incremental: the valid records go, the bad ones stay in the journal.
    let errors = std::mem::take(&mut d.errors);
    if let Err(e) = write_locked(db, imp, &actor, &d).await {
        tracing::warn!(import = %imp.id, error = %e, "scheduled import failed");
        d.errors = errors;
        return Err(("failed", error_code(&e), Value::Null, Some(Box::new(d))));
    }
    d.errors = errors;
    if let Some(p) = &m.per_object {
        let pages = imp.max_pages.map_or(DEFAULT_PAGES, |n| n as u32);
        if let Some(last) = per_object_values(db, imp, p, pages)
            .await
            .ok()
            .and_then(|v| v.last().cloned())
        {
            let _ = db
                .execute_raw(stmt(
                    "UPDATE object_imports SET per_object_cursor = $2 WHERE id = $1",
                    vec![imp.id.into(), last.into()],
                ))
                .await;
        }
    }
    Ok(d)
}

/// In-app notice to the org of a refused or failed scheduled run
/// (D303: never silent).
async fn notify_refused(db: &DatabaseConnection, imp: &ImportRow, code: &str, args: Value) {
    use crate::models::_entities::notify_channels;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    tracing::info!(import = %imp.id, org = imp.org_id, code, "scheduled import not applied");
    let msg = pnex_notify::Message {
        subject: Some(format!("Import {} not applied", imp.name)),
        body: format!(
            "The scheduled run of the import {} was not applied ({code}).",
            imp.name
        ),
        meta: serde_json::json!({
            "kind": "import-refused",
            "import": imp.id.to_string(),
            "code": code,
            "args": args,
        }),
    };
    let channel = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(imp.org_id))
        .filter(notify_channels::Column::Kind.eq("websocket"))
        .order_by_asc(notify_channels::Column::Id)
        .one(db)
        .await
        .ok()
        .flatten();
    if let Some(ch) = channel {
        crate::services::notify::deliver_in_app(
            pnex_core::NotifyDeliveryEntry {
                org_id: imp.org_id,
                channel_id: ch.id,
                source: "import".into(),
                ..Default::default()
            },
            &msg,
        );
    }
}

const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// Due scheduled imports, run one after the other on one pod.
pub fn spawn_scheduler(ctx: &loco_rs::app::AppContext) {
    let db = ctx.db.clone();
    let store = crate::services::artifact_store::S3Config::from_config(&ctx.config)
        .store()
        .ok();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            // A stopping pod lets the lease go: another pod sweeps.
            if !stop::stopping()
                && crate::services::singleton::my_turn(&db, "object-imports", SWEEP_EVERY).await
            {
                if let Err(e) = sweep(&db, store.as_ref()).await {
                    tracing::warn!("import schedule sweep failed: {e}");
                }
            }
            tokio::time::sleep(SWEEP_EVERY).await;
        }
    });
}

/// A run without a heartbeat for this long lost its server.
const RUN_STALE: &str = "10 minutes";

async fn sweep(
    db: &DatabaseConnection,
    store: Option<&crate::services::artifact_store::S3Store>,
) -> Result<()> {
    let store = store.cloned();
    // Runs lost with their pod: no heartbeat any more. An application
    // still writing holds the org's import lock (it dies with its
    // connection): such a run is not lost.
    db.execute_raw(stmt(
        &format!(
            "UPDATE object_import_runs SET state = 'failed', state_code = $2, finished_at = now() \
             WHERE state = 'running' AND heartbeat_at < now() - interval '{RUN_STALE}' \
             AND pg_try_advisory_xact_lock($1, (org_id % 2147483647)::int)"
        ),
        vec![
            crate::services::db_lock::ns::ONTOLOGY_IMPORT.into(),
            err_codes::IMPORT_RUN_INTERRUPTED.into(),
        ],
    ))
    .await?;
    // A stopping pod starts nothing: the next pod takes the due runs.
    if stop::stopping() {
        return Ok(());
    }
    let due = ImportRow::find_by_statement(stmt(
        &format!(
            "SELECT {IMPORT_COLUMNS} FROM object_imports \
             WHERE next_run_at IS NOT NULL AND next_run_at <= now() ORDER BY next_run_at LIMIT 20"
        ),
        vec![],
    ))
    .all(db)
    .await?;
    for imp in due {
        // Moved first: a run that crashes is not retried in a loop.
        db.execute_raw(stmt(
            "UPDATE object_imports SET next_run_at = $2 WHERE id = $1",
            vec![imp.id.into(), next_run(imp.recurrence.as_ref()).into()],
        ))
        .await?;
        match start(db, store.clone(), imp.org_id, imp.id, None, "schedule").await {
            Ok(r) => tracing::info!(import = %imp.id, run = %r.id, "scheduled import run started"),
            Err(e) => {
                tracing::warn!(import = %imp.id, error = %e, "scheduled import run not started")
            }
        }
    }
    Ok(())
}

/// The part of an import its pack owns (D424): mapping, source and URL.
/// Mode, schedule and pages are the org's running choices, never replaced.
fn pack_content(m: &Mapping, source: ImportSource, url: Option<&str>) -> Value {
    serde_json::json!({
        "mapping": serde_json::to_value(m).unwrap_or(Value::Null),
        "source": source.wire(),
        "source_url": url,
    })
}

/// What differs between two contents: `source`, `source_url`, or
/// `mapping.<key>` for each top-level key of the mapping.
fn differs(a: &Value, b: &Value) -> Vec<String> {
    let mut out: Vec<String> = ["source", "source_url"]
        .into_iter()
        .filter(|k| a.get(k) != b.get(k))
        .map(str::to_string)
        .collect();
    let (ma, mb) = (&a["mapping"], &b["mapping"]);
    let mut keys: Vec<&String> = ma
        .as_object()
        .into_iter()
        .chain(mb.as_object())
        .flat_map(|o| o.keys())
        .collect();
    keys.sort();
    keys.dedup();
    out.extend(
        keys.into_iter()
            .filter(|k| ma.get(k.as_str()) != mb.get(k.as_str()))
            .map(|k| format!("mapping.{k}")),
    );
    out
}

/// The pack's imports in the org, with the content the pack ships now.
async fn pack_rows(
    db: &DatabaseConnection,
    org_id: i64,
    pack: &pnex_core::ontology::api::Pack,
) -> Result<Vec<(usize, Option<ImportRow>, Value)>> {
    let mut out = Vec::with_capacity(pack.imports.len());
    for (i, p) in pack.imports.iter().enumerate() {
        let row = ImportRow::find_by_statement(stmt(
            &format!(
                "SELECT {IMPORT_COLUMNS} FROM object_imports \
                 WHERE org_id = $1 AND pack_key = $2 AND name = $3"
            ),
            vec![
                org_id.into(),
                pack.key.clone().into(),
                p.name.clone().into(),
            ],
        ))
        .one(db)
        .await?;
        let shipped = pack_content(&p.mapping, p.source, p.source_url.as_deref());
        out.push((i, row, shipped));
    }
    Ok(out)
}

/// The content an import holds now; `None` when its mapping no longer
/// parses (then it is the org's, kept).
fn current_content(row: &ImportRow) -> Option<Value> {
    Some(pack_content(
        &row.mapping().ok()?,
        row.source(),
        row.source_url.as_deref(),
    ))
}

/// The pack's imports the org edited since the pack wrote them (D424),
/// with what differs from the pack's version: an upgrade keeps them.
pub async fn kept(
    db: &DatabaseConnection,
    org_id: i64,
    pack: &pnex_core::ontology::api::Pack,
) -> Result<Vec<(String, Vec<String>)>> {
    Ok(pack_rows(db, org_id, pack)
        .await?
        .into_iter()
        .filter_map(|(i, row, shipped)| {
            let row = row?;
            let current = current_content(&row).unwrap_or(Value::Null);
            let written = row.pack_written.as_ref().map(|w| &w["written"]);
            // Created before `pack_written` (beta, no history): the pack's.
            (current != shipped && written.is_some() && written != Some(&current))
                .then(|| (pack.imports[i].name.clone(), differs(&current, &shipped)))
        })
        .collect())
}

/// Imports a pack ships (D301, D424): created at install; at an upgrade,
/// one the org left as is (or named in `take`) gets the pack's mapping,
/// source and URL, its mode, schedule and pages kept; one it edited stays.
pub async fn install_from_pack(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    pack: &pnex_core::ontology::api::Pack,
    take: &[String],
) -> Result<()> {
    let at = |i: usize| {
        move |e| match e {
            OntologyError::Invalid { field, token } => {
                invalid(format!("imports.{i}.{field}"), token)
            }
            e => e,
        }
    };
    for (i, row, shipped) in pack_rows(db, org_id, pack).await? {
        let p = &pack.imports[i];
        let mapping = serde_saphyr::to_string(&p.mapping)
            .map_err(|_| invalid(format!("imports.{i}.mapping"), FIELD_INVALID))?;
        let (id, replaced) = match row {
            None => {
                let input = ObjectImportInput {
                    name: p.name.clone(),
                    mapping,
                    source: p.source,
                    source_url: Some(p.source_url.clone()),
                    mode: p.mode,
                    recurrence: Some(p.recurrence.clone()),
                    max_pages: Some(p.max_pages),
                    expected_version: None,
                };
                let v = create(db, org_id, user_id, &input, Some(&pack.key))
                    .await
                    .map_err(at(i))?;
                (
                    v.id.parse::<Uuid>().map_err(|_| OntologyError::NotFound)?,
                    Value::Null,
                )
            }
            Some(row) => {
                let current = current_content(&row);
                let written = row.pack_written.as_ref().map(|w| &w["written"]);
                if current.as_ref() == Some(&shipped) {
                    if written != Some(&shipped) {
                        set_written(db, row.id, &shipped, &Value::Null).await?;
                    }
                    continue;
                }
                let pristine =
                    current.is_some() && (written.is_none() || written == current.as_ref());
                if !pristine && !take.contains(&p.name) {
                    continue;
                }
                let input = ObjectImportInput {
                    name: row.name.clone(),
                    mapping,
                    source: p.source,
                    source_url: Some(p.source_url.clone()),
                    mode: row.mode(),
                    recurrence: Some(row.recurrence.clone()),
                    max_pages: Some(row.max_pages.map(|n| n as u32)),
                    expected_version: Some(row.version),
                };
                update(db, org_id, row.id, &input).await.map_err(at(i))?;
                // The org's version, kept beside the pack's for a way back.
                let replaced = serde_json::json!({
                    "mapping_yaml": row.mapping_yaml,
                    "source": row.source,
                    "source_url": row.source_url,
                });
                (row.id, replaced)
            }
        };
        set_written(db, id, &shipped, &replaced).await?;
    }
    Ok(())
}

async fn set_written(
    db: &DatabaseConnection,
    id: Uuid,
    written: &Value,
    replaced: &Value,
) -> Result<()> {
    db.execute_raw(stmt(
        "UPDATE object_imports SET pack_written = $2 WHERE id = $1",
        vec![
            id.into(),
            serde_json::json!({ "written": written, "replaced": replaced }).into(),
        ],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::import::ImportCounts;

    /// The sweep's SQL key matches the lock an application holds: a run
    /// still writing is not swept as lost.
    #[tokio::test]
    async fn sweep_sees_the_lock_of_an_application_in_progress() {
        use crate::services::db_lock::{migrated_test_db, ns, TenantLock};
        let db = migrated_test_db().await;
        let org = 2_147_483_647 + 42; // folded like key32
        let try_lock = || async {
            db.query_one_raw(stmt(
                "SELECT pg_try_advisory_xact_lock($1, ($2 % 2147483647)::int) AS ok",
                vec![ns::ONTOLOGY_IMPORT.into(), org.into()],
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<bool>("", "ok")
            .unwrap()
        };
        let lock = TenantLock::acquire(&db, ns::ONTOLOGY_IMPORT, org, Duration::from_secs(5))
            .await
            .unwrap();
        assert!(!try_lock().await, "held by the application");
        lock.release().await;
        assert!(try_lock().await, "free once it ends");
    }

    #[test]
    fn counts_round_trip_through_json() {
        let c = ImportCounts {
            created: 2,
            errors: 1,
            ..Default::default()
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(serde_json::from_value::<ImportCounts>(v).unwrap(), c);
    }

    #[test]
    fn next_run_is_ahead_at_the_run_hour() {
        let r = serde_json::json!({"start": "2020-01-01", "every": 1, "unit": "day"});
        let t = next_run(Some(&r)).unwrap();
        assert!(t > chrono::Utc::now());
        assert_eq!(t.format("%H:%M").to_string(), "03:00");
        assert!(next_run(None).is_none());
    }
}
