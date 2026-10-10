//! Objects (D177): org-type objects live entirely in `objects`; system
//! objects have their identity row (written by triggers) and carry the
//! org's extra properties there (D176), their native content staying in
//! place. Every write is validated against the current type definition,
//! locked by the type's write role (D188) and journaled (D184).

use pnex_core::err_codes::{FIELD_INVALID, FIELD_MAX_LENGTH, FIELD_REQUIRED};
use pnex_core::ontology::api::{ObjectInput, ObjectView, TITLE_MAX};
use pnex_core::ontology::schema::check_values;
use pnex_core::ontology::{ObjectTypeDef, PropertyKind};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    Set,
};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::changes::{self, Change};
use super::{role_allows, schema, Actor, OntologyError, OrgSchema, Result};
use crate::models::_entities::objects;

pub fn view(o: &objects::Model, s: &OrgSchema) -> ObjectView {
    ObjectView {
        id: o.id.to_string(),
        type_key: o.type_key.clone(),
        title: o.title.clone(),
        native_id: o.native_id.clone(),
        system: s.object_type(&o.type_key).is_some_and(|t| t.system),
        properties: o.properties.as_object().cloned().unwrap_or_default(),
        type_version: o.type_version,
        version: o.version,
        source_ref: o.source_ref.clone(),
        valid_from: o.valid_from.to_rfc3339(),
        valid_to: o.valid_to.map(|t| t.to_rfc3339()),
        created_at: o.created_at.to_rfc3339(),
        updated_at: o.updated_at.to_rfc3339(),
    }
}

/// An object of the org by id (archived included).
pub async fn find<C: ConnectionTrait>(db: &C, org_id: i64, id: Uuid) -> Result<objects::Model> {
    objects::Entity::find_by_id(id)
        .filter(objects::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

/// The identity of a system or org object addressed the D42 way.
pub async fn find_by_native<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_key: &str,
    native_id: &str,
) -> Result<objects::Model> {
    objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(type_key))
        .filter(objects::Column::NativeId.eq(native_id))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

pub async fn get(db: &DatabaseConnection, org_id: i64, id: Uuid) -> Result<ObjectView> {
    let s = schema(db, org_id).await?;
    Ok(view(&find(db, org_id, id).await?, &s))
}

fn check_title(title: &str) -> Result<String> {
    let t = title.trim();
    if t.is_empty() {
        return Err(super::invalid("title", FIELD_REQUIRED));
    }
    if t.chars().count() > TITLE_MAX {
        return Err(super::invalid(
            "title",
            format!("{FIELD_MAX_LENGTH}:{TITLE_MAX}"),
        ));
    }
    Ok(t.to_string())
}

/// Validates the property document, then that every `ref` names a live
/// object of the expected type in the org (R1: never another org's).
async fn check_properties<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    def: &ObjectTypeDef,
    props: &Map<String, Value>,
) -> Result<Map<String, Value>> {
    let out = check_values(&def.properties, props)?;
    for p in &def.properties {
        let PropertyKind::Ref { to_type } = &p.kind else {
            continue;
        };
        let Some(id) = out.get(&p.key).and_then(Value::as_str) else {
            continue;
        };
        let ok = match Uuid::parse_str(id) {
            Ok(id) => find(db, org_id, id)
                .await
                .is_ok_and(|o| &o.type_key == to_type && o.valid_to.is_none()),
            Err(_) => false,
        };
        if !ok {
            return Err(super::invalid(p.key.clone(), FIELD_INVALID));
        }
    }
    Ok(out)
}

fn require_role(def: &ObjectTypeDef, actor: &Actor) -> Result<()> {
    if role_allows(&actor.role, def.write_role) {
        Ok(())
    } else {
        Err(OntologyError::WriteForbidden)
    }
}

/// Creates an org-type object. System objects are created by their own
/// pages (device wizard, media upload…): the identity follows.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    input: &ObjectInput,
) -> Result<ObjectView> {
    let s = schema(db, org_id).await?;
    let Some(def) = s.object_type(&input.type_key) else {
        return Err(super::invalid("type_key", FIELD_INVALID));
    };
    if def.system {
        return Err(OntologyError::SystemReadOnly);
    }
    require_role(def, actor)?;
    let title = check_title(&input.title)?;
    let props = check_properties(db, org_id, def, &input.properties).await?;
    let id = Uuid::new_v4();
    let row = objects::ActiveModel {
        id: Set(id),
        org_id: Set(org_id),
        type_key: Set(def.key.clone()),
        type_version: Set(s.type_version(&def.key)),
        title: Set(title),
        native_id: Set(id.to_string()),
        properties: Set(Value::Object(props)),
        source_ref: Set(Some(actor.source_ref.clone())),
        version: Set(1),
        ..Default::default()
    }
    .insert(db)
    .await?;
    let v = view(&row, &s);
    changes::submit(Change::object(
        org_id,
        "object.create",
        &actor.source_ref,
        Value::Null,
        &v,
    ));
    Ok(v)
}

/// Updates title and properties on `expected_version` (409 otherwise).
/// For a system object only the extra properties are written.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    actor: &Actor,
    id: Uuid,
    input: &ObjectInput,
) -> Result<ObjectView> {
    let s = schema(db, org_id).await?;
    let row = find(db, org_id, id).await?;
    if row.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    let Some(def) = s.object_type(&row.type_key) else {
        return Err(OntologyError::NotFound);
    };
    require_role(def, actor)?;
    let Some(expected) = input.expected_version else {
        return Err(super::invalid("expected_version", FIELD_REQUIRED));
    };
    let title = if def.system {
        row.title.clone()
    } else {
        check_title(&input.title)?
    };
    let props = check_properties(db, org_id, def, &input.properties).await?;
    let now = sea_orm::prelude::DateTimeWithTimeZone::from(chrono::Utc::now());
    let done = objects::Entity::update_many()
        .col_expr(objects::Column::Title, Expr::value(title))
        .col_expr(
            objects::Column::Properties,
            Expr::value(Value::Object(props)),
        )
        .col_expr(
            objects::Column::TypeVersion,
            Expr::value(s.type_version(&def.key)),
        )
        .col_expr(
            objects::Column::SourceRef,
            Expr::value(actor.source_ref.clone()),
        )
        .col_expr(objects::Column::Version, Expr::value(expected + 1))
        .col_expr(objects::Column::UpdatedAt, Expr::value(now))
        .filter(objects::Column::Id.eq(id))
        .filter(objects::Column::Version.eq(expected))
        .exec(db)
        .await?;
    if done.rows_affected != 1 {
        return Err(OntologyError::VersionConflict {
            current: row.version,
        });
    }
    let before = view(&row, &s);
    let after = view(&find(db, org_id, id).await?, &s);
    changes::submit(Change::object(
        org_id,
        "object.update",
        &actor.source_ref,
        serde_json::to_value(&before).unwrap_or_default(),
        &after,
    ));
    Ok(after)
}

/// Archives an org-type object: its validity closes, its open links close
/// with it (D179), nothing is deleted. System objects are deleted by their
/// own pages, which closes their identity.
pub async fn archive(db: &DatabaseConnection, org_id: i64, actor: &Actor, id: Uuid) -> Result<()> {
    let s = schema(db, org_id).await?;
    let row = find(db, org_id, id).await?;
    if row.valid_to.is_some() {
        return Err(OntologyError::NotFound);
    }
    let Some(def) = s.object_type(&row.type_key) else {
        return Err(OntologyError::NotFound);
    };
    if def.system {
        return Err(OntologyError::SystemReadOnly);
    }
    require_role(def, actor)?;
    let now = sea_orm::prelude::DateTimeWithTimeZone::from(chrono::Utc::now());
    objects::Entity::update_many()
        .col_expr(objects::Column::ValidTo, Expr::value(now))
        .col_expr(objects::Column::UpdatedAt, Expr::value(now))
        .col_expr(
            objects::Column::SourceRef,
            Expr::value(actor.source_ref.clone()),
        )
        .filter(objects::Column::Id.eq(id))
        .exec(db)
        .await?;
    // Labels and containment go like for any deleted resource (D42 purge,
    // links closed rather than deleted).
    crate::services::resources::purge_for(db, org_id, &row.type_key, &row.native_id).await?;
    let before = view(&row, &s);
    changes::submit(Change {
        org_id,
        action: "object.archive".into(),
        object_id: row.id.to_string(),
        type_key: row.type_key.clone(),
        source_ref: actor.source_ref.clone(),
        before: serde_json::to_value(&before).unwrap_or_default(),
        after: Value::Null,
    });
    Ok(())
}
