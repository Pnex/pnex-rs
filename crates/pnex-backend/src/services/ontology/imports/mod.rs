//! Objects written from rows (pipelines.md D1050): a pipeline's `write
//! objects` step diffs its rows against the base it kept, writes the
//! difference in one transaction, and keeps the new base. Imports are gone:
//! a pack copies each import it ships into an org pipeline, and the imports
//! an org still holds become org pipelines at boot (`migrate`). Every fact
//! written carries the provenance of its pipeline (D184).

pub mod diff;
mod migrate;
pub(crate) use migrate::imports_left;
pub use migrate::run as migrate_to_pipelines;
mod sync;

use std::collections::HashSet;
use std::time::Duration;

use pnex_core::err_codes::{self, FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use pnex_core::ontology::import::{
    self as imp, ImportDiff, ImportMode, ImportSource, Mapping, MAPPING_MAX_BYTES,
};
use pnex_core::ontology::recurrence::Recurrence;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
    TransactionTrait,
};
use serde_json::Value;
use uuid::Uuid;

use super::{invalid, schema, Actor, OntologyError, Result};
use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::iam::level_of;

/// Scheduled runs start at this hour (UTC) of their day.
const RUN_HOUR: u32 = 3;

fn coded(code: &'static str) -> OntologyError {
    OntologyError::Import { code, args: None }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// An import an org still holds, until its conversion at boot (`migrate`).
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
    version: i32,
    created_by: Option<i64>,
    /// What its pack last wrote (D424): `{written, replaced}`.
    pack_written: Option<Value>,
}

const IMPORT_COLUMNS: &str = "id, org_id, name, mapping_yaml, pack_key, source, source_url, mode, \
     recurrence, next_run_at, version, created_by, pack_written";

impl ImportRow {
    fn source(&self) -> ImportSource {
        match self.source.as_str() {
            "url" => ImportSource::Url,
            "api" => ImportSource::Api,
            "dataset" => ImportSource::Dataset,
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

/// Next scheduled start: the first occurrence day whose run hour is ahead.
pub(crate) fn next_run(recurrence: Option<&Value>) -> Option<DateTimeWithTimeZone> {
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

/// Where a pipeline's `write: objects` writes from (pipelines.md D1050).
pub struct RowsWriter<'a> {
    pub org_id: i64,
    /// `<pack>/<key>` or an org pipeline's id: with `step`, names the base kept
    /// between runs.
    pub pipeline: &'a str,
    pub step: &'a str,
    /// Provenance of every write; the objects it holds are this step's own.
    pub source_ref: &'a str,
    /// Author of the version run, whose role is read again at each run (D303).
    /// `None`: a pack's pipeline, written as a member.
    pub author: Option<i64>,
    /// Tables of the org the pipeline reads: it writes no higher than the
    /// member who last loaded each one (SEC-93).
    pub reads: &'a [&'a str],
    /// Closes what this step wrote and the rows no longer hold.
    pub snapshot: bool,
    /// A preview (D944): counts what it would write, writes and keeps nothing.
    pub dry: bool,
}

/// The base a step kept and the mapping it was planned with.
struct Kept {
    mapping: Value,
    base: diff::Base,
    skipped: HashSet<String>,
}

async fn kept_base(db: &DatabaseConnection, w: &RowsWriter<'_>) -> Result<Option<Kept>> {
    use std::io::Read;
    let row = db
        .query_one_raw(stmt(
            "SELECT mapping, base, skipped_keys FROM pipeline_object_bases \
             WHERE org_id = $1 AND pipeline = $2 AND step = $3",
            vec![w.org_id.into(), w.pipeline.into(), w.step.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let packed: Vec<u8> = row.try_get("", "base")?;
    let mut json = Vec::new();
    // An unreadable base is no base: the run compares to the ontology.
    if flate2::read::GzDecoder::new(&packed[..])
        .read_to_end(&mut json)
        .is_err()
    {
        return Ok(None);
    }
    let Ok(base) = serde_json::from_slice(&json) else {
        return Ok(None);
    };
    Ok(Some(Kept {
        mapping: row.try_get("", "mapping")?,
        base,
        skipped: row
            .try_get::<Vec<String>>("", "skipped_keys")?
            .into_iter()
            .collect(),
    }))
}

async fn keep_base(
    db: &DatabaseConnection,
    w: &RowsWriter<'_>,
    mapping: &Value,
    base: &diff::Base,
    skipped: Vec<String>,
) -> Result<()> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let json = serde_json::to_vec(base).map_err(|e| DbErr::Custom(e.to_string()))?;
    let packed = gz
        .write_all(&json)
        .and_then(|_| gz.finish())
        .map_err(|e| DbErr::Custom(e.to_string()))?;
    db.execute_raw(stmt(
        "INSERT INTO pipeline_object_bases (org_id, pipeline, step, mapping, base, skipped_keys) \
         VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (org_id, pipeline, step) DO UPDATE SET \
         mapping = EXCLUDED.mapping, base = EXCLUDED.base, \
         skipped_keys = EXCLUDED.skipped_keys, updated_at = now()",
        vec![
            w.org_id.into(),
            w.pipeline.into(),
            w.step.into(),
            mapping.clone().into(),
            packed.into(),
            skipped.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Writes a pipeline's rows into objects (pipelines.md D1050), as a dataset sync
/// does (D577): against the base the step kept, an edit made in the ontology
/// survives until the source changes that value. Without a base (first run,
/// mapping changed) the rows are compared to the ontology. With `snapshot`, what
/// the step wrote and the rows no longer hold is closed, unless that is more than
/// `max_closures_pct` % of what it holds. Returns what was written.
/// Row issues a run keeps in its journal, at most (D1050).
const ISSUES_KEPT: usize = 20;

/// What [`write_rows`] wrote, and the first warnings of its rows.
pub struct Written {
    pub counts: imp::ImportCounts,
    pub warnings: Vec<imp::RowIssue>,
    /// A preview's rows refused; a run is refused instead.
    pub errors: Vec<imp::RowIssue>,
}

impl Written {
    /// Objects, links, closures and dated values written.
    pub fn total(&self) -> usize {
        let c = &self.counts;
        c.created + c.updated + c.links_opened + c.links_updated + c.closed + c.observations
    }
}

pub async fn write_rows(
    db: &DatabaseConnection,
    w: &RowsWriter<'_>,
    m: &Mapping,
    rows: &[Value],
) -> Result<Written> {
    use crate::services::db_lock::{ns, TenantLock};
    let mut role = match w.author {
        None => OrgMemberRole::Member,
        Some(user) => member_role(db, w.org_id, user)
            .await
            .ok_or(coded(err_codes::ONTOLOGY_WRITE_FORBIDDEN))?,
    };
    for table in w.reads {
        let loader = loader_role(db, w.org_id, table).await?;
        if let Some(loader) = loader.filter(|l| level_of(l) < level_of(&role)) {
            role = loader;
        }
    }
    let s = schema(db, w.org_id).await?;
    let types: Vec<_> = s.types.iter().map(|(t, _, _)| t.clone()).collect();
    let links: Vec<_> = s.links.iter().map(|(l, _, _)| l.clone()).collect();
    let shape = imp::Schema {
        types: &types,
        links: &links,
    };
    let rows: Vec<Value> = rows
        .iter()
        .flat_map(|r| sync::expanded(r.clone(), &m.json_column))
        .collect();
    let rows = &rows[..];
    let mut planned = diff::Planned::default();
    for (i, row) in rows.iter().enumerate() {
        planned.push(m, &shape, i + 1, row, imp::PLANNED_MAX)?;
    }
    let mapping = serde_json::to_value(m).map_err(|e| DbErr::Custom(e.to_string()))?;
    let current = planned.base();
    let kept = kept_base(db, w).await?.filter(|k| k.mapping == mapping);
    let (closing, old) = match kept {
        Some(k) => {
            let gone = planned.delta(m, &diff::Planned::from_base(&k.base), &k.skipped);
            let closing = match w.snapshot {
                true => diff::Closing::Gone(gone),
                false => diff::Closing::None,
            };
            (closing, Some(k.base))
        }
        None if w.snapshot => (diff::Closing::NotSeen, None),
        None => (diff::Closing::None, None),
    };
    let like = diff::source_exact(w.source_ref);
    let (mut d, left_out) = diff::against(db, w.org_id, &like, &s, m, planned, closing).await?;
    if m.observations.is_some() {
        d.observations = pnex_core::markets::observations(rows);
    }
    if w.dry {
        return Ok(Written {
            counts: d.counts(),
            warnings: d.warnings.into_iter().take(ISSUES_KEPT).collect(),
            errors: d.errors.into_iter().take(ISSUES_KEPT).collect(),
        });
    }
    if !d.errors.is_empty() {
        tracing::info!(
            org_id = w.org_id,
            source_ref = w.source_ref,
            errors = d.errors.len(),
            "pipeline rows refused"
        );
        // The first rows refused and why, for the run's message.
        let first: Vec<_> = d.errors.iter().take(ISSUES_KEPT).collect();
        return Err(OntologyError::Import {
            code: err_codes::IMPORT_HAS_ERRORS,
            args: Some(serde_json::json!({ "errors": first, "count": d.errors.len() })),
        });
    }
    if w.snapshot {
        let holding = held_by(db, w.org_id, &like).await?;
        let max_pct = imp::MAX_CLOSURES_PCT;
        if holding > 0 && d.closed.len() * 100 > holding * max_pct as usize {
            return Err(OntologyError::Import {
                code: err_codes::INGEST_SUSPICIOUS_CLOSURES,
                args: Some(serde_json::json!({
                    "closed": d.closed.len().to_string(),
                    "held": holding.to_string(),
                    "max_pct": max_pct.to_string(),
                })),
            });
        }
    }
    let actor = Actor {
        user_id: w.author,
        role,
        source_ref: w.source_ref.to_string(),
    };
    let lock =
        TenantLock::acquire(db, ns::ONTOLOGY_IMPORT, w.org_id, Duration::from_secs(600)).await?;
    let mut out = write(db, w.org_id, &actor, &d).await.map(|left| (left, 0));
    // After the objects: a series created by this run must exist.
    if let (Ok((left, _)), Some(target)) = (&out, &m.observations) {
        let left = *left;
        out = crate::services::markets::store(db, w.org_id, target, &d.observations, w.source_ref)
            .await
            .map(|n| (left, n))
            .map_err(OntologyError::from);
    }
    lock.release().await;
    let (left, observed) = out?;
    // D431: the watched objects it updated count a check run.
    super::cases::watch::record_import(db, w.org_id, &actor, &m.name, &d.updated).await;
    let next = match (w.snapshot, old) {
        (false, Some(old)) => old.merged(current),
        _ => current,
    };
    keep_base(db, w, &mapping, &next, left_out).await?;
    let mut counts = d.counts();
    counts.created -= left;
    counts.observations = observed;
    Ok(Written {
        counts,
        warnings: d.warnings.into_iter().take(ISSUES_KEPT).collect(),
        errors: Vec::new(),
    })
}

/// Writes a difference in one transaction: all or nothing. Returns how many
/// creations were left out at write time.
async fn write(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    diff: &ImportDiff,
) -> Result<usize> {
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
        // A file must not bring back an address that objected or was erased
        // (crm.md D610). The preview left out those known then; this catches
        // one recorded since; the run's counts leave it out too.
        let to_create: Vec<_> = diff
            .created
            .iter()
            .map(|c| (c.type_key.clone(), c.properties.clone()))
            .collect();
        let left_out = super::opposition::opposed_creations(&txn, &s, org_id, &to_create).await?;
        drop(to_create);
        for (i, c) in diff.created.iter().enumerate() {
            if left_out.contains(&i) {
                continue;
            }
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
        // The links of a row left out go with it.
        let kept = |l: &&imp::DiffLink| {
            ![&l.from, &l.to]
                .iter()
                .any(|e| e.created.is_some_and(|i| left_out.contains(&i)))
        };
        // Proposed links wait for a human (proposals.md D375).
        for l in diff.links_opened.iter().filter(kept).filter(|l| l.propose) {
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
            .filter(kept)
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
        // Links first: archiving an object closes its links, and closing one
        // already closed is refused.
        for c in diff.closed.iter().filter(|c| c.kind != "object") {
            let id: i64 = c.id.parse().map_err(|_| OntologyError::NotFound)?;
            links::close(&txn, org_id, actor, id, None).await?;
        }
        for c in diff.closed.iter().filter(|c| c.kind == "object") {
            let id = Uuid::parse_str(&c.id).map_err(|_| OntologyError::NotFound)?;
            objects::archive(&txn, org_id, actor, id).await?;
        }
        txn.commit().await?;
        crate::services::ontology::changes::flush();
        Ok(left_out.len())
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

/// The role of who last loaded `table`: `None` when it has no version yet,
/// Member when a pack, the system or a member gone loaded it (SEC-93).
async fn loader_role(
    db: &DatabaseConnection,
    org_id: i64,
    table: &str,
) -> Result<Option<OrgMemberRole>> {
    let Some(row) = db
        .query_one_raw(stmt(
            "SELECT t.created_by FROM dataset_transactions t \
             JOIN datasets d ON d.id = t.dataset_id \
             WHERE d.org_id = $1 AND d.name = $2 AND t.state = 'committed' \
             ORDER BY t.seq DESC LIMIT 1",
            vec![org_id.into(), table.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let role = match row.try_get::<Option<i64>>("", "created_by")? {
        Some(user) => member_role(db, org_id, user).await,
        None => None,
    };
    Ok(Some(role.unwrap_or(OrgMemberRole::Member)))
}

/// The role of `user` in the org, when it may write objects.
async fn member_role<C: ConnectionTrait>(db: &C, org_id: i64, user: i64) -> Option<OrgMemberRole> {
    use crate::models::_entities::organization_members as om;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    om::Entity::find()
        .filter(om::Column::OrgId.eq(org_id))
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

/// Live objects and links whose provenance matches `like`.
async fn held_by<C: ConnectionTrait>(db: &C, org_id: i64, like: &str) -> Result<usize> {
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

/// The part of an import its pack owns (D424): mapping, source and URL.
/// Mode, schedule and pages are the org's running choices, never replaced.
fn pack_content(m: &Mapping, source: ImportSource, url: Option<&str>) -> Value {
    serde_json::json!({
        "mapping": serde_json::to_value(m).unwrap_or(Value::Null),
        "source": source.wire(),
        "source_url": url,
    })
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

/// The copies of a pack's imports the org edited since the pack wrote them
/// (D424, D1050), with the top-level fields of the document it changed: an
/// upgrade keeps them.
pub async fn kept(
    db: &DatabaseConnection,
    org_id: i64,
    pack: &pnex_core::ontology::api::Pack,
) -> Result<Vec<(String, Vec<String>)>> {
    let mut out = Vec::new();
    let copies = db
        .query_all_raw(stmt(
            "SELECT name, document, pack_written ->> 'written' AS written FROM pipelines \
             WHERE org_id = $1 AND pack = $2 AND document <> (pack_written ->> 'written') \
             ORDER BY name",
            vec![org_id.into(), pack.key.clone().into()],
        ))
        .await?;
    for r in copies {
        let json = |c: &str| -> Value {
            r.try_get::<String>("", c)
                .ok()
                .and_then(|t| serde_saphyr::from_str(&t).ok())
                .unwrap_or(Value::Null)
        };
        let (a, b) = (json("document"), json("written"));
        let mut keys: Vec<String> = a
            .as_object()
            .into_iter()
            .chain(b.as_object())
            .flat_map(|o| o.keys().cloned())
            .filter(|k| a.get(k) != b.get(k))
            .collect();
        keys.sort();
        keys.dedup();
        out.push((r.try_get("", "name")?, keys));
    }
    Ok(out)
}

/// The org pipeline a pack import becomes (pipelines.md D1050): the steps
/// of its pack pipeline, the table it writes kept (other pipelines read
/// it), then the import's mapping written as objects from the same rows. A
/// file import reads the Table named after it, where a member sends the
/// file. It runs after the copies of the imports its mapping names in
/// `after`. Left out: the pack pipeline's `tests:` (they read the pack's
/// fixture files), `schedule` and `after` (an org pipeline runs on its
/// recurrence). `None`: the pack pipeline is gone or writes no table.
pub fn pack_copy(
    pack: &pnex_core::ontology::api::Pack,
    p: &imp::PackImport,
) -> Option<pnex_core::pipeline::doc::Document> {
    let after = p
        .mapping
        .after
        .iter()
        .filter_map(|k| pack.imports.iter().find(|i| i.mapping.key == *k))
        .map(|i| i.name.clone())
        .collect();
    copy_of(p, &p.name, after)
}

/// The org pipeline of the import `p`: [`pack_copy`]'s document, a file
/// import reading `table`, run after the pipelines named in `after`.
fn copy_of(
    p: &imp::PackImport,
    table: &str,
    after: Vec<String>,
) -> Option<pnex_core::pipeline::doc::Document> {
    use pnex_core::pipeline::doc::{Document, ObjectsTarget, Sql, Step, Write};
    let m = &p.mapping;
    let objects = |from: String| {
        Step::Write(Write {
            from,
            table: None,
            objects: Some(ObjectsTarget {
                objects: m.objects.clone(),
                links: m.links.clone(),
                snapshot: p.mode == ImportMode::Snapshot,
                observations: m.observations.clone(),
                json_column: m.json_column.clone(),
                share: None,
            }),
            time_ranges: None,
            proposals: None,
            media: None,
            ask: None,
        })
    };
    let mut doc = match &m.pipeline {
        Some(id) => {
            let sp = crate::services::pipeline_engine::store::shipped()
                .iter()
                .find(|s| s.id() == *id)?;
            let mut doc = sp.doc.clone();
            let at = doc
                .steps
                .iter()
                .position(|s| matches!(s, Step::Write(w) if w.table.is_some()))?;
            let Step::Write(w) = &doc.steps[at] else {
                return None;
            };
            let from = w.from.clone();
            doc.steps.insert(at + 1, objects(from));
            // In an org's document a bare name is the org's function.
            for s in &mut doc.steps {
                if let Step::Call(c) = s {
                    if !c.function.contains('/') {
                        c.function = format!("{}/{}", sp.pack, c.function);
                    }
                }
            }
            doc.tests.clear();
            doc.schedule = None;
            doc.after.clear();
            doc
        }
        None => Document {
            key: m.key.clone(),
            name: String::new(),
            hosts: Vec::new(),
            object_hosts: Vec::new(),
            steps: vec![
                Step::Sql(Sql {
                    as_: "rows".into(),
                    tables: [("source".to_string(), table.to_string())].into(),
                    columns: Default::default(),
                    sql: "SELECT * FROM source".into(),
                }),
                objects("rows".into()),
            ],
            schedule: None,
            large: false,
            after: Vec::new(),
            requires: Vec::new(),
            tests: Vec::new(),
        },
    };
    doc.name = p.name.clone();
    doc.after = after;
    Some(doc)
}

/// Copies a pack's imports into org pipelines (pipelines.md D1050, D424):
/// created at install, with the import's recurrence; at an upgrade, a copy
/// the org left as is (or named in `take`) gets the pack's new document,
/// one it edited stays. A pipeline of the same name the pack never wrote is
/// the org's own, left alone. The pack writes with no author: a pack's role.
pub async fn install_copies(
    db: &DatabaseConnection,
    org_id: i64,
    pack: &pnex_core::ontology::api::Pack,
    take: &[String],
) -> Result<()> {
    let imports_left = imports_left(db).await?;
    for p in &pack.imports {
        // An org installed before D1050 holds it as an import until its migration.
        let held = imports_left
            && db
                .query_one_raw(stmt(
                    "SELECT 1 AS one FROM object_imports WHERE org_id = $1 AND pack_key = $2 AND name = $3",
                    vec![org_id.into(), pack.key.clone().into(), p.name.clone().into()],
                ))
                .await?
                .is_some();
        let Some(doc) = pack_copy(pack, p).filter(|_| !held) else {
            continue;
        };
        let document =
            serde_json::to_string_pretty(&doc).map_err(|e| DbErr::Custom(e.to_string()))?;
        let written = serde_json::json!({ "written": document });
        let row = db
            .query_one_raw(stmt(
                "SELECT id, version, document, pack, pack_written ->> 'written' AS written \
                 FROM pipelines WHERE org_id = $1 AND name = $2",
                vec![org_id.into(), p.name.clone().into()],
            ))
            .await?;
        let tx = db.begin().await?;
        match row {
            None => {
                let recurrence = p.recurrence.clone();
                let id: Uuid = tx
                    .query_one_raw(stmt(
                        "INSERT INTO pipelines (org_id, name, document, pack, pack_written, \
                           recurrence, next_run_at) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
                        vec![
                            org_id.into(),
                            p.name.clone().into(),
                            document.clone().into(),
                            pack.key.clone().into(),
                            written.into(),
                            recurrence.clone().into(),
                            next_run(recurrence.as_ref()).into(),
                        ],
                    ))
                    .await?
                    .ok_or(OntologyError::NotFound)?
                    .try_get("", "id")?;
                tx.execute_raw(stmt(
                    "INSERT INTO pipeline_versions (pipeline_id, version, document) VALUES ($1, 1, $2)",
                    vec![id.into(), document.into()],
                ))
                .await?;
            }
            Some(r) => {
                let owner: Option<String> = r.try_get("", "pack")?;
                let current: String = r.try_get("", "document")?;
                let last: Option<String> = r.try_get("", "written")?;
                if owner.as_deref() != Some(pack.key.as_str()) || current == document {
                    continue;
                }
                if last.as_deref() != Some(current.as_str()) && !take.contains(&p.name) {
                    continue;
                }
                let id: Uuid = r.try_get("", "id")?;
                let version: i32 = r.try_get("", "version")?;
                tx.execute_raw(stmt(
                    "UPDATE pipelines SET document = $2, version = version + 1, \
                       updated_at = now(), pack_written = $3 WHERE id = $1",
                    vec![id.into(), document.clone().into(), written.into()],
                ))
                .await?;
                tx.execute_raw(stmt(
                    "INSERT INTO pipeline_versions (pipeline_id, version, document) VALUES ($1, $2, $3)",
                    vec![id.into(), (version + 1).into(), document.into()],
                ))
                .await?;
            }
        }
        tx.commit().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::import::ImportCounts;

    /// D1050: each import a pack ships becomes an org pipeline the server
    /// accepts, which writes objects.
    #[test]
    fn every_pack_import_makes_a_valid_org_pipeline() {
        use pnex_core::pipeline::doc::Step;
        let mut n = 0;
        for pack in crate::services::ontology::packs::shipped() {
            for p in &pack.imports {
                let doc = pack_copy(&pack, p).unwrap_or_else(|| panic!("{}: {}", pack.key, p.name));
                assert_eq!(
                    doc.after.len(),
                    p.mapping.after.len(),
                    "{}: {}",
                    pack.key,
                    p.name
                );
                let yaml = serde_json::to_string_pretty(&doc).unwrap();
                crate::services::pipelines::checked(&yaml)
                    .unwrap_or_else(|e| panic!("{}: {}: {e:?}", pack.key, p.name));
                assert!(doc
                    .steps
                    .iter()
                    .any(|s| matches!(s, Step::Write(w) if w.objects.is_some())));
                assert!(doc
                    .steps
                    .iter()
                    .all(|s| !matches!(s, Step::Call(c) if !c.function.contains('/'))));
                n += 1;
            }
        }
        assert!(n > 50, "{n} pack imports");
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
