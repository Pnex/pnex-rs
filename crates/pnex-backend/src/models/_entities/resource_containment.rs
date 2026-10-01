//! `SeaORM` Entity — couche d'organisation transverse (D42).
//!
//! Arbre d'appartenance : un seul parent par enfant (index unique
//! `uniq_resource_containment_child`). Pas de FK polymorphe — existence +
//! tenancy validées par le service ; `purge_for` remplace la cascade.

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
    /// Ordre stable au sein du parent (drag & drop) — léxicographique.
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
