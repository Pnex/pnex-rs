//! Org object types (D176): create, append-only versions with optimistic
//! concurrency (`expected_version` → 409), delete when unused.

use pnex_core::ontology::api::{ObjectTypeInput, ObjectTypeVersionView, ObjectTypeView};
use pnex_core::ontology::ObjectTypeDef;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, Set, TransactionTrait,
};
use uuid::Uuid;

use super::{schema, OntologyError, OrgSchema, Result};
use crate::models::_entities::{object_type_versions, object_types, objects};

/// Live object count per type key, one grouped query.
async fn counts<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
) -> std::result::Result<std::collections::HashMap<String, i64>, sea_orm::DbErr> {
    let rows = db
        .query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT type_key, count(*)::bigint AS n FROM objects \
             WHERE org_id = $1 AND valid_to IS NULL GROUP BY type_key",
            [org_id.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some((
                r.try_get::<String>("", "type_key").ok()?,
                r.try_get::<i64>("", "n").ok()?,
            ))
        })
        .collect())
}

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<ObjectTypeView>> {
    let s = schema(db, org_id).await?;
    let n = counts(db, org_id).await?;
    Ok(s.types
        .into_iter()
        .map(|(def, version, pack_key)| ObjectTypeView {
            object_count: n.get(&def.key).copied().unwrap_or(0),
            def,
            version,
            pack_key,
        })
        .collect())
}

pub async fn get(db: &DatabaseConnection, org_id: i64, key: &str) -> Result<ObjectTypeView> {
    list(db, org_id)
        .await?
        .into_iter()
        .find(|t| t.def.key == key)
        .ok_or(OntologyError::NotFound)
}

async fn find_row<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    key: &str,
) -> Result<object_types::Model> {
    object_types::Entity::find()
        .filter(object_types::Column::OrgId.eq(org_id))
        .filter(object_types::Column::Key.eq(key))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

/// Validates a definition against the org schema (the type itself counts
/// as known, so a type may contain or reference itself).
fn check(s: &OrgSchema, def: &ObjectTypeDef) -> Result<()> {
    let known = |k: &str| k == def.key || s.knows(k);
    def.check(&known).map_err(Into::into)
}

/// Creates an org type at version 1. `pack_key` marks a pack install.
pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    user_id: Option<i64>,
    def: &ObjectTypeDef,
    pack_key: Option<&str>,
) -> Result<ObjectTypeView> {
    let s = schema(db, org_id).await?;
    if s.knows(&def.key) {
        return Err(OntologyError::KeyTaken);
    }
    check(&s, def)?;
    let txn = db.begin().await?;
    let row = object_types::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        key: Set(def.key.clone()),
        current_version: Set(1),
        pack_key: Set(pack_key.map(str::to_string)),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|e| match e.sql_err() {
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) => OntologyError::KeyTaken,
        _ => e.into(),
    })?;
    insert_version(&txn, org_id, row.id, 1, def, user_id).await?;
    txn.commit().await?;
    Ok(ObjectTypeView {
        def: def.clone(),
        version: 1,
        pack_key: row.pack_key,
        object_count: 0,
    })
}

async fn insert_version<C: ConnectionTrait>(
    db: &C,
    org_id: i64,
    type_id: Uuid,
    version: i32,
    def: &ObjectTypeDef,
    user_id: Option<i64>,
) -> Result<()> {
    object_type_versions::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        object_type_id: Set(type_id),
        version: Set(version),
        definition: Set(serde_json::to_value(def).unwrap_or_default()),
        created_by: Set(user_id),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(())
}

/// Appends a version on top of `expected_version` (409 otherwise). Existing
/// objects keep their properties; they are re-validated at their next write.
pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    user_id: Option<i64>,
    input: &ObjectTypeInput,
) -> Result<ObjectTypeView> {
    if input.def.key != key {
        return Err(super::invalid("key", pnex_core::err_codes::FIELD_INVALID));
    }
    let s = schema(db, org_id).await?;
    if let Some(sys) = s.object_type(key).filter(|t| t.system) {
        return extend_system(db, org_id, sys.clone(), &s, user_id, input).await;
    }
    check(&s, &input.def)?;
    let Some(expected) = input.expected_version else {
        return Err(super::invalid(
            "expected_version",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    };
    let txn = db.begin().await?;
    let row = find_row(&txn, org_id, key).await?;
    let next = expected + 1;
    let bumped = object_types::Entity::update_many()
        .col_expr(object_types::Column::CurrentVersion, Expr::value(next))
        .col_expr(
            object_types::Column::UpdatedAt,
            Expr::value(sea_orm::prelude::DateTimeWithTimeZone::from(
                chrono::Utc::now(),
            )),
        )
        .filter(object_types::Column::Id.eq(row.id))
        .filter(object_types::Column::CurrentVersion.eq(expected))
        .exec(&txn)
        .await?;
    if bumped.rows_affected != 1 {
        return Err(OntologyError::VersionConflict {
            current: row.current_version,
        });
    }
    insert_version(&txn, org_id, row.id, next, &input.def, user_id).await?;
    txn.commit().await?;
    get(db, org_id, key).await
}

/// A system type gains org properties (D176): only `properties` is taken
/// from the input, the rest of the definition stays the system one. The
/// extension is stored as an org type row under the system key.
async fn extend_system(
    db: &DatabaseConnection,
    org_id: i64,
    mut def: ObjectTypeDef,
    s: &OrgSchema,
    user_id: Option<i64>,
    input: &ObjectTypeInput,
) -> Result<ObjectTypeView> {
    pnex_core::ontology::schema::check_defs(&input.def.properties, "properties", &|k| s.knows(k))?;
    def.properties = input.def.properties.clone();
    let expected = input.expected_version.unwrap_or(0);
    let txn = db.begin().await?;
    match find_row(&txn, org_id, &def.key).await {
        Err(OntologyError::NotFound) if expected == 0 => {
            let row = object_types::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(org_id),
                key: Set(def.key.clone()),
                current_version: Set(1),
                ..Default::default()
            }
            .insert(&txn)
            .await?;
            insert_version(&txn, org_id, row.id, 1, &def, user_id).await?;
        }
        Err(OntologyError::NotFound) => return Err(OntologyError::VersionConflict { current: 0 }),
        Err(e) => return Err(e),
        Ok(row) => {
            let bumped = object_types::Entity::update_many()
                .col_expr(
                    object_types::Column::CurrentVersion,
                    Expr::value(expected + 1),
                )
                .filter(object_types::Column::Id.eq(row.id))
                .filter(object_types::Column::CurrentVersion.eq(expected))
                .exec(&txn)
                .await?;
            if bumped.rows_affected != 1 {
                return Err(OntologyError::VersionConflict {
                    current: row.current_version,
                });
            }
            insert_version(&txn, org_id, row.id, expected + 1, &def, user_id).await?;
        }
    }
    txn.commit().await?;
    get(db, org_id, &def.key).await
}

pub async fn versions(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
) -> Result<Vec<ObjectTypeVersionView>> {
    let row = find_row(db, org_id, key).await?;
    Ok(object_type_versions::Entity::find()
        .filter(object_type_versions::Column::ObjectTypeId.eq(row.id))
        .order_by_desc(object_type_versions::Column::Version)
        .all(db)
        .await?
        .into_iter()
        .filter_map(|v| {
            Some(ObjectTypeVersionView {
                version: v.version,
                def: serde_json::from_value(v.definition).ok()?,
                created_at: v.created_at.to_rfc3339(),
            })
        })
        .collect())
}

/// Is `key` named by another org type (ref, containment) or a link type?
fn referenced(s: &OrgSchema, key: &str) -> bool {
    use pnex_core::ontology::{PropertyKind, TypeSet};
    let names =
        |set: &Option<TypeSet>| matches!(set, Some(TypeSet::Only(k)) if k.iter().any(|k| k == key));
    s.org_types().iter().filter(|t| t.key != key).any(|t| {
        names(&t.may_contain)
            || names(&t.may_be_contained_in)
            || t.properties
                .iter()
                .any(|p| matches!(&p.kind, PropertyKind::Ref { to_type } if to_type == key))
    }) || s
        .org_links()
        .iter()
        .any(|l| names(&Some(l.from_types.clone())) || names(&Some(l.to_types.clone())))
}

/// Deletes an org type without live objects (archived ones go with it).
pub async fn delete(db: &DatabaseConnection, org_id: i64, key: &str) -> Result<()> {
    if pnex_core::ontology::is_system_key(key) {
        return Err(OntologyError::SystemReadOnly);
    }
    let row = find_row(db, org_id, key).await?;
    let live = objects::Entity::find()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(key))
        .filter(objects::Column::ValidTo.is_null())
        .one(db)
        .await?;
    if live.is_some() || referenced(&schema(db, org_id).await?, key) {
        return Err(OntologyError::TypeInUse);
    }
    let txn = db.begin().await?;
    // Archived objects of the type and their links leave with it.
    txn.execute_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "DELETE FROM resource_edges WHERE org_id = $1 AND (source_kind = $2 OR target_kind = $2)",
        [org_id.into(), key.into()],
    ))
    .await?;
    for t in ["resource_labels", "resource_containments"] {
        let col = if t == "resource_labels" {
            "resource_kind"
        } else {
            "child_kind"
        };
        txn.execute_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            &format!("DELETE FROM {t} WHERE org_id = $1 AND {col} = $2"),
            [org_id.into(), key.into()],
        ))
        .await?;
    }
    objects::Entity::delete_many()
        .filter(objects::Column::OrgId.eq(org_id))
        .filter(objects::Column::TypeKey.eq(key))
        .exec(&txn)
        .await?;
    object_types::Entity::delete_by_id(row.id)
        .exec(&txn)
        .await?;
    txn.commit().await?;
    Ok(())
}
