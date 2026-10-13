//! `SeaORM` Entity (hand-written on the sea-orm-codegen shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Frozen dataset of a Vision Lab project (D199), append-only.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "lab_dataset_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub project_id: Uuid,
    pub version: i32,
    pub note: String,
    /// `pnex_core::lab::DatasetSnapshot`.
    #[sea_orm(column_type = "JsonBinary")]
    pub snapshot: Json,
    /// `pnex_core::lab::LabStats`.
    #[sea_orm(column_type = "JsonBinary")]
    pub stats: Json,
    pub created_by: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
