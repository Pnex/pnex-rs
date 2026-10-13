//! `SeaORM` Entity (hand-written on the sea-orm-codegen shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Vision Lab project (media-vision-studio.md D198): object classes of a
/// custom detector.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "lab_projects")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub name: String,
    pub description: String,
    /// Class names (JSON array of strings); boxes refer to them by index.
    #[sea_orm(column_type = "JsonBinary")]
    pub classes: Json,
    pub created_by: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
