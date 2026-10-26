//! Typed temporal links (D179) over `resource_edges`: open, close (never
//! delete), read "as of" an instant. Cardinality and attributes come from
//! the link type; `measures` additionally binds a device metric to a
//! `series` property of the target (D181).

use std::collections::HashMap;

use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};
use pnex_core::ontology::api::{LinkInput, LinkView, ObjectRef};
use pnex_core::ontology::schema::check_values;
use pnex_core::ontology::{
    PropertyKind, DEPICTS_POSE, MEASURES_METRIC, MEASURES_PROPERTY, REL_DEPICTS, REL_MEASURES,
};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::changes::{self, Change};
use super::objects::find;
use super::{parse_instant, role_allows, schema, Actor, OntologyError, OrgSchema, Result};
use crate::models::_entities::{objects, resource_edges};

pub fn object_ref(o: &objects::Model) -> ObjectRef {
    ObjectRef {
        id: o.id.to_string(),
        type_key: o.type_key.clone(),
        title: o.title.clone(),
        native_id: o.native_id.clone(),
    }
}

/// Identities of a batch of ids, one query.
pub async fn refs<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    ids: impl IntoIterator<Item = Uuid>,
) -> std::result::Result<HashMap<Uuid, objects::Model>, sea_orm::DbErr> {
    let ids: Vec<Uuid> = ids.into_iter().collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::Id.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|o| (o.id, o))
        .collect())
}

fn end_ref(
    id: Option<Uuid>,
    kind: &str,
    native: &str,
    by_id: &HashMap<Uuid, objects::Model>,
) -> ObjectRef {
    match id.and_then(|id| by_id.get(&id)) {
        Some(o) => object_ref(o),
        None => ObjectRef {
            id: id.map(|i| i.to_string()).unwrap_or_default(),
            type_key: kind.into(),
            title: String::new(),
            native_id: native.into(),
        },
    }
}

pub async fn views<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    edges: &[resource_edges::Model],
) -> std::result::Result<Vec<LinkView>, sea_orm::DbErr> {
    let ids: Vec<Uuid> = edges
        .iter()
        .flat_map(|e| [e.source_object_id, e.target_object_id])
        .flatten()
        .collect();
    let by_id = refs(db, org_id, ids).await?;
    Ok(views_from(edges, &by_id))
}

/// `views` with the ends already loaded.
fn views_from(
    edges: &[resource_edges::Model],
    by_id: &HashMap<Uuid, objects::Model>,
) -> Vec<LinkView> {
    edges
        .iter()
        .map(|e| LinkView {
            id: e.id,
            link_type: e.relation.clone(),
            source: end_ref(e.source_object_id, &e.source_kind, &e.source_id, &by_id),
            target: end_ref(e.target_object_id, &e.target_kind, &e.target_id, &by_id),
            attributes: e.attributes.as_object().cloned().unwrap_or_default(),
            valid_from: e.valid_from.to_rfc3339(),
            valid_to: e.valid_to.map(|t| t.to_rfc3339()),
            source_ref: e.source_ref.clone(),
        })
        .collect()
}

/// Validity at `t`: started at or before `t`, not closed at `t`.
pub fn valid_at(t: DateTimeWithTimeZone) -> Condition {
    Condition::all()
        .add(resource_edges::Column::ValidFrom.lte(t))
        .add(
            Condition::any()
                .add(resource_edges::Column::ValidTo.is_null())
                .add(resource_edges::Column::ValidTo.gt(t)),
        )
}

/// Links of an object (either end). `as_of` = links valid then; `history`
/// = every link ever, newest first (closed ones included).
pub async fn for_object(
    db: &DatabaseConnection,
    org_id: i64,
    id: Uuid,
    as_of: Option<DateTimeWithTimeZone>,
    history: bool,
) -> Result<Vec<LinkView>> {
    find(db, org_id, id).await?;
    let mut q = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(
            Condition::any()
                .add(resource_edges::Column::SourceObjectId.eq(id))
                .add(resource_edges::Column::TargetObjectId.eq(id)),
        )
        .order_by_desc(resource_edges::Column::ValidFrom);
    if !history {
        q = q.filter(valid_at(as_of.unwrap_or_else(|| chrono::Utc::now().into())));
    }
    let rows = q.all(db).await?;
    Ok(views(db, org_id, &rows).await?)
}

async fn open_exists<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    relation: &str,
    cond: Condition,
) -> std::result::Result<bool, sea_orm::DbErr> {
    Ok(resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(relation))
        .filter(resource_edges::Column::ValidTo.is_null())
        .filter(cond)
        .one(db)
        .await?
        .is_some())
}

fn parse_id(field: &str, raw: &str) -> Result<Uuid> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| super::invalid(field, pnex_core::err_codes::FIELD_INVALID_UUID))
}

/// Opens a link. Both ends must be live objects of the org, admitted by
/// the link type; the caller's role must allow writing both ends (D188).
/// Works inside a transaction (the journal entry is queued at once).
pub async fn create<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    input: &LinkInput,
) -> Result<LinkView> {
    create_with(db, &schema(db, org_id).await?, org_id, actor, input).await
}

/// `create` on a schema already loaded (a bulk import loads it once).
pub async fn create_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    input: &LinkInput,
) -> Result<LinkView> {
    let Some(lt) = s.link_type(&input.link_type).cloned() else {
        return Err(super::invalid("link_type", FIELD_INVALID));
    };
    let src = find(db, org_id, parse_id("source_id", &input.source_id)?).await?;
    let tgt = find(db, org_id, parse_id("target_id", &input.target_id)?).await?;
    if src.valid_to.is_some() || tgt.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    if src.erased_at.is_some() || tgt.erased_at.is_some() {
        return Err(OntologyError::Erased);
    }
    if src.id == tgt.id || !lt.allows(&lt.key, &src.type_key, &tgt.type_key) {
        return Err(OntologyError::LinkNotAllowed);
    }
    for t in [&src.type_key, &tgt.type_key] {
        if let Some(def) = s.object_type(t) {
            if !role_allows(&actor.role, def.write_role) {
                return Err(OntologyError::WriteForbidden);
            }
        }
    }
    super::opposition::check_links(db, s, org_id, &[(&src, &tgt)]).await?;
    let attrs = check_values(&lt.attributes, &input.attributes)
        .map_err(|(f, t)| super::invalid(format!("attributes.{f}"), t))?;
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let from = parse_instant("valid_from", input.valid_from.as_deref())?.unwrap_or(now);
    if from > now {
        return Err(super::invalid("valid_from", FIELD_INVALID));
    }
    let to = parse_instant("valid_to", input.valid_to.as_deref())?;
    if to.is_some_and(|to| to < from || to > now) {
        return Err(super::invalid("valid_to", FIELD_INVALID));
    }
    // Cardinalities bind open links only: a closed link is history.
    let open = to.is_none();
    if lt.key == REL_DEPICTS
        && attrs
            .get(DEPICTS_POSE)
            .and_then(Value::as_str)
            .is_some_and(|p| !pnex_core::ontology::pose_valid(p))
    {
        return Err(super::invalid(
            format!("attributes.{DEPICTS_POSE}"),
            FIELD_INVALID,
        ));
    }
    if open && lt.key == REL_MEASURES {
        // The bound property must be a `series` of the target type, fed by
        // one device at a time.
        let prop = attrs
            .get(MEASURES_PROPERTY)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_series = s.object_type(&tgt.type_key).is_some_and(|d| {
            d.properties
                .iter()
                .any(|p| p.key == prop && matches!(p.kind, PropertyKind::Series { .. }))
        });
        if !is_series {
            return Err(super::invalid(
                format!("attributes.{MEASURES_PROPERTY}"),
                FIELD_INVALID,
            ));
        }
        if attrs
            .get(MEASURES_METRIC)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(super::invalid(
                format!("attributes.{MEASURES_METRIC}"),
                FIELD_REQUIRED,
            ));
        }
        let bound = Condition::all()
            .add(resource_edges::Column::TargetObjectId.eq(tgt.id))
            .add(Expr::cust_with_values(
                "attributes ->> 'property' = $1",
                [prop],
            ));
        if open_exists(db, org_id, &lt.key, bound).await? {
            return Err(OntologyError::LinkCardinality);
        }
    }
    if open
        && lt.one_target
        && open_exists(
            db,
            org_id,
            &lt.key,
            resource_edges::Column::SourceObjectId
                .eq(src.id)
                .into_condition(),
        )
        .await?
    {
        return Err(OntologyError::LinkCardinality);
    }
    if open
        && lt.one_source
        && open_exists(
            db,
            org_id,
            &lt.key,
            resource_edges::Column::TargetObjectId
                .eq(tgt.id)
                .into_condition(),
        )
        .await?
    {
        return Err(OntologyError::LinkCardinality);
    }
    let row = resource_edges::ActiveModel {
        org_id: Set(org_id),
        relation: Set(lt.key.clone()),
        source_kind: Set(src.type_key.clone()),
        source_id: Set(src.native_id.clone()),
        target_kind: Set(tgt.type_key.clone()),
        target_id: Set(tgt.native_id.clone()),
        attributes: Set(Value::Object(attrs)),
        valid_from: Set(from),
        valid_to: Set(to),
        source_ref: Set(Some(actor.source_ref.clone())),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|e| match e.sql_err() {
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) => OntologyError::KeyTaken,
        _ => e.into(),
    })?;
    let v = views(db, org_id, &[row]).await?.remove(0);
    let events: &[&str] = if open {
        &["link.open"]
    } else {
        &["link.open", "link.close"]
    };
    for kind in events {
        for c in Change::link(org_id, kind, &actor.source_ref, &v) {
            changes::submit(c);
        }
    }
    Ok(v)
}

/// Replaces the attributes of a link, open or closed (a source added, a note
/// corrected). The ends, the type and the validity stay: those are a new link.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    id: i64,
    attributes: &Map<String, Value>,
) -> Result<LinkView> {
    update_with(
        db,
        &schema(db, org_id).await?,
        org_id,
        actor,
        id,
        attributes,
    )
    .await
}

/// `update` on a schema already loaded (a bulk import loads it once).
pub async fn update_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    id: i64,
    attributes: &Map<String, Value>,
) -> Result<LinkView> {
    let row = resource_edges::Entity::find_by_id(id)
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)?;
    for t in [&row.source_kind, &row.target_kind] {
        if let Some(def) = s.object_type(t) {
            if !role_allows(&actor.role, def.write_role) {
                return Err(OntologyError::WriteForbidden);
            }
        }
    }
    let Some(lt) = s.link_type(&row.relation) else {
        return Err(OntologyError::NotFound);
    };
    // A binding's attributes carry its uniqueness (measures) or a checked
    // pose (depicts): rebind instead.
    if lt.key == REL_MEASURES || lt.key == REL_DEPICTS {
        return Err(super::invalid("attributes", FIELD_INVALID));
    }
    let attrs = check_values(&lt.attributes, attributes)
        .map_err(|(f, t)| super::invalid(format!("attributes.{f}"), t))?;
    let before =
        serde_json::to_value(views(db, org_id, &[row]).await?.remove(0)).unwrap_or_default();
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    resource_edges::Entity::update_many()
        .col_expr(
            resource_edges::Column::Attributes,
            Expr::value(Value::Object(attrs)),
        )
        .col_expr(resource_edges::Column::UpdatedAt, Expr::value(now))
        .filter(resource_edges::Column::Id.eq(id))
        .exec(db)
        .await?;
    let row = resource_edges::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)?;
    let v = views(db, org_id, &[row]).await?.remove(0);
    for mut c in Change::link(org_id, "link.update", &actor.source_ref, &v) {
        c.before = before.clone().into();
        changes::submit(c);
    }
    Ok(v)
}

/// Closes an open link at `valid_to` (default now): it stays as history.
pub async fn close<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    actor: &Actor,
    id: i64,
    valid_to: Option<&str>,
) -> Result<LinkView> {
    let s = schema(db, org_id).await?;
    let row = resource_edges::Entity::find_by_id(id)
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::ValidTo.is_null())
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)?;
    for t in [&row.source_kind, &row.target_kind] {
        if let Some(def) = s.object_type(t) {
            if !role_allows(&actor.role, def.write_role) {
                return Err(OntologyError::WriteForbidden);
            }
        }
    }
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let to = parse_instant("valid_to", valid_to)?.unwrap_or(now);
    if to < row.valid_from || to > now {
        return Err(super::invalid("valid_to", FIELD_INVALID));
    }
    resource_edges::Entity::update_many()
        .col_expr(resource_edges::Column::ValidTo, Expr::value(to))
        .col_expr(resource_edges::Column::UpdatedAt, Expr::value(now))
        .filter(resource_edges::Column::Id.eq(id))
        .exec(db)
        .await?;
    let row = resource_edges::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)?;
    let v = views(db, org_id, &[row]).await?.remove(0);
    for c in Change::link(org_id, "link.close", &actor.source_ref, &v) {
        changes::submit(c);
    }
    Ok(v)
}

use sea_orm::sea_query::IntoCondition;

// ─────────────────────────── bulk (data-pipelines.md D347) ───────────────────────────

const BULK_CHUNK: usize = super::objects::BULK_CHUNK;

fn bulk_stmt(sql: &str, values: Vec<sea_orm::Value>) -> sea_orm::Statement {
    sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// Open links of `relation` among `ids` on one side (`column` =
/// `source_object_id` or `target_object_id`).
async fn open_on<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    relation: &str,
    column: &str,
    ids: Vec<Uuid>,
) -> Result<bool> {
    for chunk in ids.chunks(BULK_CHUNK) {
        let hit = db
            .query_one_raw(bulk_stmt(
                &format!(
                    "SELECT 1 AS hit FROM resource_edges WHERE org_id = $1 AND relation = $2 \
                       AND valid_to IS NULL AND {column} = ANY($3) LIMIT 1"
                ),
                vec![org_id.into(), relation.into(), chunk.to_vec().into()],
            ))
            .await?;
        if hit.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A row of a bulk link insert; a `Value` per link costs several times it.
#[derive(serde::Serialize)]
struct NewEdge<'a> {
    relation: &'a str,
    source_kind: &'a str,
    source_id: &'a str,
    target_kind: &'a str,
    target_id: &'a str,
    source_object_id: Uuid,
    target_object_id: Uuid,
    attributes: Map<String, Value>,
    valid_from: String,
}

/// `create_with` for many open links (imports): the same checks, ends
/// loaded and cardinalities checked per batch. `measures`, `depicts` and
/// links given a `valid_to` keep the one-by-one path.
pub async fn create_many_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    inputs: &[LinkInput],
) -> Result<()> {
    let mut parsed = Vec::with_capacity(inputs.len());
    for input in inputs {
        parsed.push((
            parse_id("source_id", &input.source_id)?,
            parse_id("target_id", &input.target_id)?,
        ));
    }
    let mut ends: HashMap<Uuid, objects::Model> = HashMap::new();
    let mut all: Vec<Uuid> = parsed.iter().flat_map(|(a, b)| [*a, *b]).collect();
    all.sort_unstable();
    all.dedup();
    for chunk in all.chunks(BULK_CHUNK) {
        // Only the ends' identity is read: their properties are not kept.
        ends.extend(
            refs(db, org_id, chunk.iter().copied())
                .await?
                .into_iter()
                .map(|(id, mut o)| {
                    o.properties = Value::Null;
                    (id, o)
                }),
        );
    }
    // Cardinalities first, on the ids alone: checked once the links of the
    // run are inserted, they would find themselves.
    let mut one: HashMap<(&str, &str), Vec<Uuid>> = HashMap::new();
    for (input, (sid, tid)) in inputs.iter().zip(&parsed) {
        let Some(lt) = s.link_type(&input.link_type) else {
            return Err(super::invalid("link_type", FIELD_INVALID));
        };
        if lt.key == REL_MEASURES || lt.key == REL_DEPICTS || input.valid_to.is_some() {
            continue;
        }
        if lt.one_target {
            one.entry((lt.key.as_str(), "source_object_id"))
                .or_default()
                .push(*sid);
        }
        if lt.one_source {
            one.entry((lt.key.as_str(), "target_object_id"))
                .or_default()
                .push(*tid);
        }
    }
    for ((relation, column), mut ids) in one {
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != n || open_on(db, org_id, relation, column, ids).await? {
            return Err(OntologyError::LinkCardinality);
        }
    }
    // A link that would recreate a person who objected is left out, the
    // rest of the run goes on (crm.md D447).
    let pairs: Vec<_> = parsed
        .iter()
        .filter_map(|(a, b)| Some((ends.get(a)?, ends.get(b)?)))
        .collect();
    let opposed = super::opposition::opposed_pairs(db, s, org_id, &pairs).await?;
    // Then batch by batch: checked, inserted, journaled (only a batch of
    // rows is held at a time; the caller's transaction keeps it all or
    // nothing).
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    for (batch, ids) in inputs.chunks(BULK_CHUNK).zip(parsed.chunks(BULK_CHUNK)) {
        let mut recs: Vec<NewEdge> = Vec::with_capacity(batch.len());
        for (input, (sid, tid)) in batch.iter().zip(ids) {
            let Some(lt) = s.link_type(&input.link_type) else {
                return Err(super::invalid("link_type", FIELD_INVALID));
            };
            if opposed.contains(&(*sid, *tid)) {
                continue;
            }
            if lt.key == REL_MEASURES || lt.key == REL_DEPICTS || input.valid_to.is_some() {
                create_with(db, s, org_id, actor, input).await?;
                continue;
            }
            let (Some(src), Some(tgt)) = (ends.get(sid), ends.get(tid)) else {
                return Err(OntologyError::NotFound);
            };
            if src.valid_to.is_some() || tgt.valid_to.is_some() {
                return Err(OntologyError::NotFound);
            }
            // An erased end is left out like an opposed one (D446).
            if src.erased_at.is_some() || tgt.erased_at.is_some() {
                continue;
            }
            if src.id == tgt.id || !lt.allows(&lt.key, &src.type_key, &tgt.type_key) {
                return Err(OntologyError::LinkNotAllowed);
            }
            for t in [&src.type_key, &tgt.type_key] {
                if let Some(def) = s.object_type(t) {
                    if !role_allows(&actor.role, def.write_role) {
                        return Err(OntologyError::WriteForbidden);
                    }
                }
            }
            let attrs = check_values(&lt.attributes, &input.attributes)
                .map_err(|(f, t)| super::invalid(format!("attributes.{f}"), t))?;
            let from = parse_instant("valid_from", input.valid_from.as_deref())?.unwrap_or(now);
            if from > now {
                return Err(super::invalid("valid_from", FIELD_INVALID));
            }
            recs.push(NewEdge {
                relation: &lt.key,
                source_kind: &src.type_key,
                source_id: &src.native_id,
                target_kind: &tgt.type_key,
                target_id: &tgt.native_id,
                source_object_id: src.id,
                target_object_id: tgt.id,
                attributes: attrs,
                valid_from: from.to_rfc3339(),
            });
        }
        if recs.is_empty() {
            continue;
        }
        let rows = resource_edges::Entity::find()
            .from_raw_sql(bulk_stmt(
                "INSERT INTO resource_edges (org_id, relation, source_kind, source_id, \
                   target_kind, target_id, source_object_id, target_object_id, attributes, \
                   valid_from, source_ref) \
                 SELECT $1, u.relation, u.source_kind, u.source_id, u.target_kind, u.target_id, \
                   u.source_object_id, u.target_object_id, u.attributes, u.valid_from, $2 \
                 FROM jsonb_to_recordset($3::jsonb) AS u(relation text, source_kind text, \
                   source_id text, target_kind text, target_id text, source_object_id uuid, \
                   target_object_id uuid, attributes jsonb, valid_from timestamptz) \
                 RETURNING *",
                vec![
                    org_id.into(),
                    actor.source_ref.clone().into(),
                    serde_json::to_value(&recs).unwrap_or_default().into(),
                ],
            ))
            .all(db)
            .await
            .map_err(|e| match e.sql_err() {
                Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) => OntologyError::KeyTaken,
                _ => e.into(),
            })?;
        for v in views_from(&rows, &ends) {
            for c in Change::link(org_id, "link.open", &actor.source_ref, &v) {
                changes::submit(c);
            }
        }
    }
    Ok(())
}

/// `update_with` for many links (imports): attributes replaced in one
/// statement per batch.
pub async fn update_many_with<C: ConnectionTrait>(
    db: &C,
    s: &OrgSchema,
    org_id: i64,
    actor: &Actor,
    inputs: &[(i64, Map<String, Value>)],
) -> Result<()> {
    for chunk in inputs.chunks(BULK_CHUNK) {
        let ids: Vec<i64> = chunk.iter().map(|(id, _)| *id).collect();
        let rows: HashMap<i64, resource_edges::Model> = resource_edges::Entity::find()
            .filter(resource_edges::Column::OrgId.eq(org_id))
            .filter(resource_edges::Column::Id.is_in(ids))
            .all(db)
            .await?
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let mut recs = Vec::with_capacity(chunk.len());
        let mut before_rows = Vec::with_capacity(chunk.len());
        for (id, attributes) in chunk {
            let row = rows.get(id).ok_or(OntologyError::NotFound)?;
            for t in [&row.source_kind, &row.target_kind] {
                if let Some(def) = s.object_type(t) {
                    if !role_allows(&actor.role, def.write_role) {
                        return Err(OntologyError::WriteForbidden);
                    }
                }
            }
            let Some(lt) = s.link_type(&row.relation) else {
                return Err(OntologyError::NotFound);
            };
            if lt.key == REL_MEASURES || lt.key == REL_DEPICTS {
                return Err(super::invalid("attributes", FIELD_INVALID));
            }
            let attrs = check_values(&lt.attributes, attributes)
                .map_err(|(f, t)| super::invalid(format!("attributes.{f}"), t))?;
            recs.push(serde_json::json!({ "id": id, "attributes": attrs }));
            before_rows.push(row.clone());
        }
        let before: HashMap<i64, Value> = views(db, org_id, &before_rows)
            .await?
            .into_iter()
            .map(|v| (v.id, serde_json::to_value(&v).unwrap_or_default()))
            .collect();
        let updated = resource_edges::Entity::find()
            .from_raw_sql(bulk_stmt(
                "UPDATE resource_edges e SET attributes = u.attributes, updated_at = now() \
                 FROM jsonb_to_recordset($2::jsonb) AS u(id bigint, attributes jsonb) \
                 WHERE e.org_id = $1 AND e.id = u.id \
                 RETURNING e.*",
                vec![org_id.into(), Value::Array(recs).into()],
            ))
            .all(db)
            .await?;
        for v in views(db, org_id, &updated).await? {
            for mut c in Change::link(org_id, "link.update", &actor.source_ref, &v) {
                c.before = before.get(&v.id).cloned().unwrap_or_default().into();
                changes::submit(c);
            }
        }
    }
    Ok(())
}
