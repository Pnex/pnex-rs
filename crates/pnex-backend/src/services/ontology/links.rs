//! Typed temporal links (D179) over `resource_edges`: open, close (never
//! delete), read "as of" an instant. Cardinality and attributes come from
//! the link type; `measures` additionally binds a device metric to a
//! `series` property of the target (D181).

use std::collections::HashMap;

use pnex_core::err_codes::{FIELD_INVALID, FIELD_REQUIRED};
use pnex_core::ontology::api::{LinkInput, LinkView, ObjectRef};
use pnex_core::ontology::schema::check_values;
use pnex_core::ontology::{PropertyKind, MEASURES_METRIC, MEASURES_PROPERTY, REL_MEASURES};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, Set,
};
use serde_json::Value;
use uuid::Uuid;

use super::changes::{self, Change};
use super::objects::find;
use super::{parse_instant, role_allows, schema, Actor, OntologyError, Result};
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
    Ok(edges
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
        .collect())
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

async fn open_exists(
    db: &DatabaseConnection,
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
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    input: &LinkInput,
) -> Result<LinkView> {
    let s = schema(db, org_id).await?;
    let Some(lt) = s.link_type(&input.link_type).cloned() else {
        return Err(super::invalid("link_type", FIELD_INVALID));
    };
    let src = find(db, org_id, parse_id("source_id", &input.source_id)?).await?;
    let tgt = find(db, org_id, parse_id("target_id", &input.target_id)?).await?;
    if src.valid_to.is_some() || tgt.valid_to.is_some() {
        return Err(OntologyError::NotFound);
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
    if lt.key == REL_MEASURES {
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
    if lt.one_target
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
    if lt.one_source
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
    let now: DateTimeWithTimeZone = chrono::Utc::now().into();
    let from = parse_instant("valid_from", input.valid_from.as_deref())?.unwrap_or(now);
    if from > now {
        return Err(super::invalid("valid_from", FIELD_INVALID));
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
    for c in Change::link(org_id, "link.open", &actor.source_ref, &v) {
        changes::submit(c);
    }
    Ok(v)
}

/// Closes an open link at `valid_to` (default now): it stays as history.
pub async fn close(
    db: &DatabaseConnection,
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
