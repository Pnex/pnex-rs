//! `SeaORM` Entity (hand-written on the sea-orm-codegen shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Training run of a Vision Lab project (D204).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "lab_training_runs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub project_id: Uuid,
    pub dataset_version_id: Uuid,
    pub base_model_id: Option<Uuid>,
    /// `queued` | `running` | `succeeded` | `failed` | `cancelled`.
    pub status: String,
    pub epochs: i32,
    /// `pnex_core::lab::LabRunProgress`.
    #[sea_orm(column_type = "JsonBinary")]
    pub progress: Json,
    pub map50: Option<f32>,
    pub error: Option<String>,
    pub error_detail: Option<String>,
    pub model_id: Option<Uuid>,
    pub created_by: String,
    pub created_at: DateTimeWithTimeZone,
    pub heartbeat_at: DateTimeWithTimeZone,
    pub finished_at: Option<DateTimeWithTimeZone>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
