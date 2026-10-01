//! `SeaORM` Entity — compile-only check of a custom firmware revision
//!, written by the queue worker, polled by the IDE.
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "firmware_checks")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub firmware_project_id: i64,
    pub revision_number: i64,
    pub status: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub diagnostics: Option<Json>,
    #[sea_orm(column_type = "Text", nullable)]
    pub log_tail: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
