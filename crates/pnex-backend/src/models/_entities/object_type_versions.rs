//! `SeaORM` Entity (hand-written on the generated shape).
//! Ontology core (docs/architecture/ontology.md D176–D191).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Append-only version of an org object type (D176).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "object_type_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub object_type_id: Uuid,
    pub version: i32,
    #[sea_orm(column_type = "JsonBinary")]
    pub definition: Json,
    pub created_by: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "Cascade",
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
