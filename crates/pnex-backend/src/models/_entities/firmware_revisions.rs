//! `SeaORM` Entity — append-only revision of a custom firmware project
//! (`main.cpp` + pinned catalog libraries). School
//! `_entities/function_versions.rs` + denormalized org_id.
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "firmware_revisions")]
pub struct Model {
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique_key = "uniq_firmware_revisions_project_number")]
    pub firmware_project_id: i64,
    /// Denormalized for direct scoping.
    pub org_id: i64,
    #[sea_orm(unique_key = "uniq_firmware_revisions_project_number")]
    pub revision_number: i64,
    #[sea_orm(column_type = "Text")]
    pub main_cpp: String,
    /// Pinned catalog ids (`Vec<String>` serialized).
    #[sea_orm(column_type = "JsonBinary")]
    pub lib_deps: Json,
    pub content_hash: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub note: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::firmware_projects::Entity",
        from = "Column::FirmwareProjectId",
        to = "super::firmware_projects::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    FirmwareProjects,
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    Organizations,
}

impl Related<super::firmware_projects::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::FirmwareProjects.def()
    }
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}
