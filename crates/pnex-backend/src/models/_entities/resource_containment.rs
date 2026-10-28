//! `SeaORM` Entity — cross-cutting organization layer (D42).
//!
//! Membership tree: a single parent per child (unique index
//! `uniq_resource_containment_child`). No polymorphic FK — existence +
//! tenancy validated by the service; `purge_for` replaces the cascade.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "resource_containments")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub child_kind: String,
    pub child_id: String,
    pub parent_kind: String,
    pub parent_id: String,
    /// Stable order within the parent (drag & drop) — lexicographic.
    pub sort_key: Option<String>,
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
