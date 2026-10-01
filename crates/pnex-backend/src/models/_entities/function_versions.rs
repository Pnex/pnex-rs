//! `SeaORM` Entity — version append-only d'une fonction (code + interface
//! extraite des directives). École `_entities/flow_versions.rs` + org_id
//! dénormalisé (école media_versions).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "function_versions")]
pub struct Model {
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique_key = "uniq_function_versions_function_number")]
    pub function_id: i64,
    /// Dénormalisé pour le scoping direct (école media_versions).
    pub org_id: i64,
    #[sea_orm(unique_key = "uniq_function_versions_function_number")]
    pub version_number: i64,
    #[sea_orm(column_type = "Text")]
    pub code: String,
    /// Interface déclarée (`pnex_core::FunctionInput[]` sérialisée).
    #[sea_orm(column_type = "JsonBinary")]
    pub inputs: Json,
    /// Interface déclarée (`pnex_core::FunctionOutput[]` sérialisée).
    #[sea_orm(column_type = "JsonBinary")]
    pub outputs: Json,
    #[sea_orm(column_type = "Text", nullable)]
    pub note: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::functions::Entity",
        from = "Column::FunctionId",
        to = "super::functions::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Functions,
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Organizations,
}

impl Related<super::functions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Functions.def()
    }
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}
