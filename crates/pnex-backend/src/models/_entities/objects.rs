//! `SeaORM` Entity (hand-written on the generated shape).
//! Ontology core (docs/architecture/ontology.md D176–D191).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Identity of every object, system or org type (D177 amended, annex A2).
/// System rows are written by database triggers only.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "objects")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub type_key: String,
    pub type_version: Option<i32>,
    pub title: String,
    pub native_id: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub properties: Json,
    pub source_ref: Option<String>,
    pub valid_from: DateTimeWithTimeZone,
    pub valid_to: Option<DateTimeWithTimeZone>,
    pub version: i32,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
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
