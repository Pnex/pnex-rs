//! `SeaORM` Entity — fonction du registre « Fonctions » (identité, sans code).
//! Style « hand-annotated codegen » (école `_entities/flows.rs`).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "functions")]
pub struct Model {
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub name: String,
    /// `"js" | "starlark"` — VARCHAR applicatif (pnex_core::FunctionLanguage).
    pub language: String,
    pub description: Option<String>,
    pub current_version_id: Option<i64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::function_versions::Entity",
        from = "Column::CurrentVersionId",
        to = "super::function_versions::Column::Id",
        on_update = "NoAction",
        on_delete = "SetNull"
    )]
    FunctionVersions,
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Organizations,
}

impl Related<super::function_versions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FunctionVersions.def()
    }
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}
