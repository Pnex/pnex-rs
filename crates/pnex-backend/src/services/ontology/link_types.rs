//! Org link types (D179): create, update on `expected_version`, delete
//! when no link of the type exists (open or closed: history is kept).

use pnex_core::ontology::api::{LinkTypeInput, LinkTypeView};
use pnex_core::ontology::LinkTypeDef;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use super::{schema, OntologyError, Result};
use crate::models::_entities::{link_types, resource_edges};

pub async fn list(db: &DatabaseConnection, org_id: i64) -> Result<Vec<LinkTypeView>> {
    Ok(schema(db, org_id)
        .await?
        .links
        .into_iter()
        .map(|(def, version, pack_key)| LinkTypeView {
            def,
            version,
            pack_key,
        })
        .collect())
}

pub async fn create(
    db: &DatabaseConnection,
    org_id: i64,
    def: &LinkTypeDef,
    pack_key: Option<&str>,
) -> Result<LinkTypeView> {
    let s = schema(db, org_id).await?;
    if s.link_type(&def.key).is_some() {
        return Err(OntologyError::KeyTaken);
    }
    def.check(&|k| s.knows(k))?;
    link_types::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        key: Set(def.key.clone()),
        definition: Set(serde_json::to_value(def).unwrap_or_default()),
        version: Set(1),
        pack_key: Set(pack_key.map(str::to_string)),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_err(|e| match e.sql_err() {
        Some(sea_orm::SqlErr::UniqueConstraintViolation(_)) => OntologyError::KeyTaken,
        _ => e.into(),
    })?;
    Ok(LinkTypeView {
        def: def.clone(),
        version: 1,
        pack_key: pack_key.map(str::to_string),
    })
}

async fn find_row(db: &DatabaseConnection, org_id: i64, key: &str) -> Result<link_types::Model> {
    link_types::Entity::find()
        .filter(link_types::Column::OrgId.eq(org_id))
        .filter(link_types::Column::Key.eq(key))
        .one(db)
        .await?
        .ok_or(OntologyError::NotFound)
}

pub async fn update(
    db: &DatabaseConnection,
    org_id: i64,
    key: &str,
    input: &LinkTypeInput,
) -> Result<LinkTypeView> {
    if input.def.key != key {
        return Err(super::invalid("key", pnex_core::err_codes::FIELD_INVALID));
    }
    let s = schema(db, org_id).await?;
    if s.link_type(key).is_some_and(|l| l.system) {
        return Err(OntologyError::SystemReadOnly);
    }
    input.def.check(&|k| s.knows(k))?;
    let Some(expected) = input.expected_version else {
        return Err(super::invalid(
            "expected_version",
            pnex_core::err_codes::FIELD_REQUIRED,
        ));
    };
    let row = find_row(db, org_id, key).await?;
    let bumped = link_types::Entity::update_many()
        .col_expr(link_types::Column::Version, Expr::value(expected + 1))
        .col_expr(
            link_types::Column::Definition,
            Expr::value(serde_json::to_value(&input.def).unwrap_or_default()),
        )
        .col_expr(
            link_types::Column::UpdatedAt,
            Expr::value(sea_orm::prelude::DateTimeWithTimeZone::from(
                chrono::Utc::now(),
            )),
        )
        .filter(link_types::Column::Id.eq(row.id))
        .filter(link_types::Column::Version.eq(expected))
        .exec(db)
        .await?;
    if bumped.rows_affected != 1 {
        return Err(OntologyError::VersionConflict {
            current: row.version,
        });
    }
    Ok(LinkTypeView {
        def: input.def.clone(),
        version: expected + 1,
        pack_key: row.pack_key,
    })
}

pub async fn delete(db: &DatabaseConnection, org_id: i64, key: &str) -> Result<()> {
    if pnex_core::ontology::is_system_key(key) {
        return Err(OntologyError::SystemReadOnly);
    }
    let row = find_row(db, org_id, key).await?;
    let used = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org_id))
        .filter(resource_edges::Column::Relation.eq(key))
        .one(db)
        .await?;
    if used.is_some() {
        return Err(OntologyError::TypeInUse);
    }
    link_types::Entity::delete_by_id(row.id).exec(db).await?;
    Ok(())
}
