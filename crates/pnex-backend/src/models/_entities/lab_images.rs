//! `SeaORM` Entity (hand-written on the sea-orm-codegen shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Image of a Vision Lab project (D198): a media asset and its boxes.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "lab_images")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub project_id: Uuid,
    pub media_asset_id: Uuid,
    pub name: String,
    pub width: i32,
    pub height: i32,
    /// Import batch the image comes from (train / validation split unit).
    pub source: String,
    /// `auto` | `train` | `val`.
    pub split: String,
    /// `[{class, bbox: [x, y, w, h]}]`, normalized.
    #[sea_orm(column_type = "JsonBinary")]
    pub boxes: Json,
    pub negative: bool,
    /// Optimistic concurrency counter of the annotations.
    pub version: i32,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
