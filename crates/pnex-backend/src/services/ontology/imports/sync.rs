//! Sync of a dataset into the ontology (data-pipelines.md D577, §9.7 ter): each
//! run plans the transactions of the current view and those last applied, by
//! entity, and writes what changed between them. An edit made in the ontology
//! therefore survives until the source changes that value (Q-P6).
//!
//! Without a valid reference (mapping changed, version purged) the whole plan is
//! compared to the current state, which can overwrite edits: such a run stops at
//! a preview, unless it is the first one.

use std::collections::{BTreeSet, HashSet};

use futures_util::TryStreamExt;
use pnex_core::datasets::IDENTITY_TITLE;
use pnex_core::err_codes;
use pnex_core::markets::PlannedObservation;
use pnex_core::ontology::import::{
    Convert, FieldSpec, ImportDiff, ImportMode, ImportSource, Mapping, Schema, SourceFormat,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::Value;
use uuid::Uuid;

use super::diff::{self, Closing, Planned};
use super::{
    author_role, finish_run, invalid, notify_refused, previous_rows, schema, stmt, write_locked,
    Actor, ImportRow, OntologyError, Result, RunRow,
};
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, parquet, DatasetError, ViewFile};

/// Objects planned in one transaction before refusing.
// shortcut: bounds two whole plans held in the server, plan in the worker if a real
// source needs more (see §9.6 for what was measured).
const PLANNED_MAX: usize = 5_000_000;

pub(super) fn dataset_id(imp: &ImportRow) -> Option<Uuid> {
    imp.source_url.as_deref().and_then(|u| u.parse().ok())
}

/// An import whose steps leave a CSV goes through a table: no limit on rows held
/// in memory, a version per run, and a table pipelines can read. Other formats
/// have nested records a flat table cannot hold.
pub(super) fn steps_fill_a_table(m: &Mapping) -> bool {
    !m.steps.is_empty() && m.format == Some(SourceFormat::Csv)
}

/// A table is readable by any viewer of the org, a sensitive value only by an admin
/// (data-pipelines.md §9.4): such a mapping does not go through a table.
pub(super) fn maps_sensitive(s: &super::super::OrgSchema, m: &Mapping) -> bool {
    use pnex_core::ontology::schema::Classification::Sensitive;
    let objects = m.objects.iter().any(|o| {
        s.object_type(&o.type_key).is_some_and(|def| {
            (o.title.is_some() && def.title_classification == Sensitive)
                || def
                    .properties
                    .iter()
                    .any(|p| p.classification == Sensitive && o.properties.contains_key(&p.key))
        })
    });
    let links = m.links.iter().any(|l| {
        s.links.iter().any(|(t, _, _)| {
            t.key == l.link_type
                && t.attributes
                    .iter()
                    .any(|a| a.classification == Sensitive && l.attributes.contains_key(&a.key))
        })
    });
    objects || links
}

/// The table an import's steps fill, named after its mapping and owned by the
/// import: a table of that name held by anything else is refused, never shared.
pub(super) async fn table_of<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    import_id: Uuid,
    m: &Mapping,
) -> Result<Uuid> {
    let row = db
        .query_one_raw(stmt(
            "INSERT INTO datasets (org_id, name, import_id) VALUES ($1, $2, $3) \
             ON CONFLICT (org_id, name) DO UPDATE SET name = EXCLUDED.name \
             WHERE datasets.import_id = EXCLUDED.import_id RETURNING id",
            vec![org_id.into(), m.key.clone().into(), import_id.into()],
        ))
        .await?
        .ok_or(OntologyError::Import {
            code: err_codes::DATASET_NAME_TAKEN,
            args: None,
        })?;
    let id = row.try_get("", "id")?;
    declare_identity(db, org_id, id, m).await?;
    Ok(id)
}

/// A dataset of the org, and a mapping with nothing that belongs to another source.
pub(super) async fn check<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    source_url: &str,
    m: &Mapping,
) -> Result<()> {
    let id: Uuid = source_url
        .parse()
        .map_err(|_| invalid("source_url", err_codes::FIELD_INVALID_UUID))?;
    let found = db
        .query_one_raw(stmt(
            "SELECT 1 AS x FROM datasets WHERE org_id = $1 AND id = $2",
            vec![org_id.into(), id.into()],
        ))
        .await?;
    if found.is_none() {
        return Err(invalid("source_url", err_codes::FIELD_INVALID));
    }
    if maps_sensitive(&schema(db, org_id).await?, m) {
        return Err(invalid("mapping", err_codes::FIELD_INVALID));
    }
    // A filter or a lookup is a transform of the pipeline before this sync.
    for (set, field) in [
        (!m.steps.is_empty(), "steps"),
        (m.per_object.is_some(), "per_object"),
        (m.press_feed.is_some(), "press_feed"),
        (m.events_feed.is_some(), "events_feed"),
        (m.conflict_feed.is_some(), "conflict_feed"),
        (m.per_setting_code.is_some(), "per_setting_code"),
        (m.rows_in_setting.is_some(), "rows_in_setting"),
        (m.contact_setting.is_some(), "contact_setting"),
        (m.keyed_source.is_some(), "keyed_source"),
        (m.shape.is_some(), "shape"),
    ] {
        if set {
            return Err(invalid(
                format!("mapping.{field}"),
                err_codes::FIELD_INVALID,
            ));
        }
    }
    Ok(())
}

/// The column read as is, up to what erasure ignores anyway: another conversion
/// leaves a stored value the column does not hold, so it cannot identify a row.
fn identity_column(f: &FieldSpec) -> Option<&str> {
    let plain = f.convert.iter().all(|c| {
        matches!(
            c,
            Convert::Trim | Convert::Upper | Convert::Lower | Convert::Email
        )
    });
    f.from.as_deref().filter(|_| plain && f.concat.is_empty())
}

/// Erasing an object must reach its rows: the mapping says which columns hold its
/// personal values, as `datasets_identity_set` would. Adds, never removes.
pub(super) async fn declare_identity<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    id: Uuid,
    m: &Mapping,
) -> Result<()> {
    let s = schema(db, org_id).await?;
    for o in m.objects.iter().filter(|o| o.each.is_none()) {
        let Some(def) = s.object_type(&o.type_key) else {
            continue;
        };
        let title = o
            .title
            .as_ref()
            .filter(|_| def.title_classification.is_personal())
            .and_then(identity_column)
            .map(|c| (c, IDENTITY_TITLE));
        let properties = o.properties.iter().filter_map(|(k, f)| {
            def.properties
                .iter()
                .any(|p| p.key == *k && p.classification.is_personal())
                .then(|| identity_column(f).map(|c| (c, k.as_str())))
                .flatten()
        });
        for (column, property) in title.into_iter().chain(properties) {
            db.execute_raw(stmt(
                "INSERT INTO dataset_identity_columns (dataset_id, column_name, type_key, property) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
                vec![
                    id.into(),
                    column.into(),
                    o.type_key.clone().into(),
                    property.into(),
                ],
            ))
            .await?;
        }
    }
    Ok(())
}

async fn plan(
    store: &S3Store,
    files: &[ViewFile],
    m: &Mapping,
    s: &Schema<'_>,
) -> Result<(Planned, Vec<PlannedObservation>)> {
    let unreadable = |e: ::parquet::errors::ParquetError| {
        tracing::warn!("dataset sync read: {e}");
        OntologyError::Import {
            code: err_codes::DATASET_STORAGE_UNAVAILABLE,
            args: None,
        }
    };
    let mut planned = Planned::default();
    let mut observed = Vec::new();
    // Line 1 of a CSV holds the headers: line numbers read as in the source.
    let mut row_no = 1;
    for file in files {
        let mut batches = parquet::batches(store, &file.key, file.bytes)
            .await
            .map_err(unreadable)?;
        while let Some(batch) = batches.try_next().await.map_err(unreadable)? {
            for row in parquet::records(&batch).map_err(unreadable)? {
                row_no += 1;
                planned.push(m, s, row_no, &row, PLANNED_MAX)?;
                if m.observations.is_some() {
                    observed.extend(pnex_core::markets::observation(&row));
                }
            }
            tokio::task::yield_now().await;
        }
    }
    Ok((planned, pnex_core::markets::first_reports(observed)))
}

/// A preview stands only on the transactions it read (D577).
pub(super) async fn preview_current(
    db: &DatabaseConnection,
    imp: &ImportRow,
    run_id: Uuid,
) -> Result<bool> {
    let Some(dataset) = dataset_id(imp) else {
        return Ok(false);
    };
    let read: Option<Vec<Uuid>> = db
        .query_one_raw(stmt(
            "SELECT dataset_transactions AS t FROM object_import_runs WHERE id = $1",
            vec![run_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get("", "t").ok());
    let Ok(files) = datasets::current_files(db, imp.org_id, dataset).await else {
        return Ok(false);
    };
    Ok(read.is_some_and(|r| transactions(&files) == r))
}

fn transactions(files: &[ViewFile]) -> Vec<Uuid> {
    files
        .iter()
        .map(|f| f.transaction)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Planning holds two whole plans (1 GB on the largest source measured): one at a
/// time, so that several orgs do not exhaust the API pod.
static PLANNING: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

/// One run. `apply` false stops at a preview.
pub(super) async fn run(
    db: &DatabaseConnection,
    store: &S3Store,
    imp: &ImportRow,
    id: Uuid,
    m: &Mapping,
    run: &RunRow,
    apply: bool,
) -> Result<()> {
    let _one_at_a_time = PLANNING.acquire().await;
    // The difference is computed outside the org lock: another import writing
    // the same objects meanwhile makes it stale (audit B1). It is computed
    // again once, on what that import left.
    let mut stale = false;
    attempt(db, store, imp, id, m, run, apply, Some(&mut stale)).await?;
    if stale {
        tracing::info!(import = %imp.id, "difference stale, computed again");
        attempt(db, store, imp, id, m, run, apply, None).await?;
    }
    Ok(())
}

/// One computation and write of the run; with `stale`, a write that finds
/// the objects changed since sets it and leaves the run open.
#[allow(clippy::too_many_arguments)]
async fn attempt(
    db: &DatabaseConnection,
    store: &S3Store,
    imp: &ImportRow,
    id: Uuid,
    m: &Mapping,
    run: &RunRow,
    apply: bool,
    stale: Option<&mut bool>,
) -> Result<()> {
    let end = |state: &'static str,
               code: Option<&'static str>,
               args: Value,
               d: Option<ImportDiff>| async move {
        let keep = matches!(state, "preview" | "refused");
        finish_run(db, run.id, state, code, &args, d.as_ref(), keep).await?;
        if state == "refused" {
            notify_refused(db, imp, code.unwrap_or_default(), args).await;
        }
        Ok::<_, OntologyError>(())
    };
    let files = match datasets::current_files(db, imp.org_id, id).await {
        Ok(f) => f,
        Err(DatasetError::NotFound) => {
            return end(
                "failed",
                Some(err_codes::DATASET_NOT_FOUND),
                Value::Null,
                None,
            )
            .await
        }
        Err(DatasetError::VersionPurged) => {
            // Purged by an erasure: its pipeline is computing it again.
            return end(
                "failed",
                Some(err_codes::DATASET_VERSION_PURGED),
                Value::Null,
                None,
            )
            .await;
        }
        Err(e) => return Err(OntologyError::Db(sea_orm::DbErr::Custom(format!("{e:?}")))),
    };
    let current = transactions(&files);

    let previous = db
        .query_one_raw(stmt(
            "SELECT import_version, dataset_transactions AS t, skipped_keys AS s \
             FROM object_import_runs WHERE import_id = $1 AND state = 'applied' \
             AND dataset_transactions IS NOT NULL ORDER BY number DESC LIMIT 1",
            vec![imp.id.into()],
        ))
        .await?;
    let has_previous = previous.is_some();
    let skipped: HashSet<String> = previous
        .as_ref()
        .and_then(|r| r.try_get::<Option<Vec<String>>>("", "s").ok().flatten())
        .unwrap_or_default()
        .into_iter()
        .collect();
    let reference: Option<Vec<Uuid>> = previous.and_then(|r| {
        let version: i32 = r.try_get("", "import_version").ok()?;
        let read: Option<Vec<Uuid>> = r.try_get("", "t").ok()?;
        read.filter(|_| version == imp.version)
    });

    // The transactions read go on the run whatever it becomes: an applied one is
    // the next reference, a preview is checked against them when applied.
    db.execute_raw(stmt(
        "UPDATE object_import_runs SET dataset_transactions = $2 WHERE id = $1",
        vec![run.id.into(), current.clone().into()],
    ))
    .await?;

    // What was left out may have what it needs by now: it has to be planned again.
    if reference.as_ref() == Some(&current) && skipped.is_empty() {
        let d = ImportDiff {
            rows: previous_rows(db, imp.id).await?,
            ..Default::default()
        };
        return end("applied", None, Value::Null, Some(d)).await;
    }

    if !files.is_empty() {
        let dataset = datasets::get(db, imp.org_id, id).await;
        let held: BTreeSet<String> = dataset
            .map(|d| d.columns.into_iter().map(|c| c.name).collect())
            .unwrap_or_default();
        let missing: Vec<String> = m
            .columns()
            .into_iter()
            .filter(|c| !held.contains(c))
            .collect();
        if !missing.is_empty() {
            let args = serde_json::json!({ "columns": missing.join(", ") });
            return end(
                "refused",
                Some(err_codes::IMPORT_SCHEMA_CHANGED),
                args,
                None,
            )
            .await;
        }
    } else if has_previous {
        let args = serde_json::json!({ "columns": "—" });
        return end(
            "refused",
            Some(err_codes::IMPORT_SCHEMA_CHANGED),
            args,
            None,
        )
        .await;
    }

    let s = schema(db, imp.org_id).await?;
    let types: Vec<_> = s.types.iter().map(|(t, _, _)| t.clone()).collect();
    let links: Vec<_> = s.links.iter().map(|(l, _, _)| l.clone()).collect();
    let shape = Schema {
        types: &types,
        links: &links,
    };
    let snapshot = imp.mode() == ImportMode::Snapshot;
    let (mut planned, observations) = plan(store, &files, m, &shape).await?;
    let base_files = match &reference {
        Some(r) => datasets::files_of_transactions(db, imp.org_id, id, r)
            .await
            .map_err(|e| OntologyError::Db(sea_orm::DbErr::Custom(format!("{e:?}"))))?,
        None => None,
    };
    let compared_to_reference = base_files.is_some();
    let closing = match base_files {
        Some(base) => {
            let (base, _) = plan(store, &base, m, &shape).await?;
            let gone = planned.delta(m, &base, &skipped);
            match snapshot {
                true => Closing::Gone(gone),
                false => Closing::None,
            }
        }
        None if snapshot => Closing::NotSeen,
        None => Closing::None,
    };
    let (mut d, left_out) = diff::against(db, imp.org_id, imp.id, &s, m, planned, closing).await?;
    d.observations = observations;
    db.execute_raw(stmt(
        "UPDATE object_import_runs SET skipped_keys = $2 WHERE id = $1",
        vec![run.id.into(), left_out.into()],
    ))
    .await?;

    if snapshot && !d.errors.is_empty() {
        return end(
            "refused",
            Some(err_codes::IMPORT_HAS_ERRORS),
            Value::Null,
            Some(d),
        )
        .await;
    }
    let holding = super::held(db, imp.org_id, imp.id).await?;
    let max_pct = pnex_core::ingest::MAX_CLOSURES_PCT;
    if holding > 0 && d.closed.len() * 100 > holding * max_pct as usize {
        let args = serde_json::json!({
            "closed": d.closed.len().to_string(),
            "held": holding.to_string(),
            "max_pct": max_pct.to_string(),
        });
        return end(
            "refused",
            Some(err_codes::INGEST_SUSPICIOUS_CLOSURES),
            args,
            Some(d),
        )
        .await;
    }
    // The only path that can overwrite an edit is shown first (D577), and a
    // scheduled run left waiting is never silent (D303).
    let must_show = has_previous && !compared_to_reference;
    if !apply || must_show {
        if apply {
            notify_refused(db, imp, err_codes::IMPORT_PREVIEW_REQUIRED, Value::Null).await;
        }
        return end("preview", None, Value::Null, Some(d)).await;
    }
    let Some(role) = author_role(db, imp).await else {
        return end(
            "refused",
            Some(err_codes::ONTOLOGY_WRITE_FORBIDDEN),
            Value::Null,
            None,
        )
        .await;
    };
    // Whoever can write a table feeds this run, which has no human reading it: it
    // gets their rights, not those of the author (R2). A table filled by the
    // import's own steps cannot be written by anyone else.
    let role = match imp.source() {
        ImportSource::Dataset => OrgMemberRole::Member,
        _ => role,
    };
    let actor = Actor {
        user_id: imp.created_by,
        role,
        source_ref: format!("import:{}@{}", imp.id, run.number),
    };
    match write_locked(db, imp, &actor, &mut d).await {
        Ok(()) => end("applied", None, Value::Null, Some(d)).await,
        Err(e) if stale.is_some() && super::is_stale(&e) => {
            if let Some(s) = stale {
                *s = true;
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!(import = %imp.id, error = %e, "dataset sync failed");
            end("failed", Some(super::error_code(&e)), Value::Null, Some(d)).await
        }
    }
}
