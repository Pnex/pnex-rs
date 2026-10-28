//! `SeaORM` Entity — cross-cutting organization layer (D42).
//!
//! Queryable classification: 1 JSONB doc per resource
//! (unique `(org_id, resource_kind, resource_id)`), PG GIN index.
//! `labels` nullable in DB (portable) — the service guarantees non-null
//! semantics (`COALESCE(labels, '{}')` on read, always `Some` on write).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "resource_labels")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub resource_kind: String,
    pub resource_id: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub labels: Option<Json>,
    pub updated_by: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Organizations,
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
