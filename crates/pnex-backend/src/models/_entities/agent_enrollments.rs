//! `SeaORM` Entity — single-use edge agent install code (D95). Only the
//! SHA-256 of the code is stored.
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "agent_enrollments")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub device_registry_id: i64,
    pub code_hash: String,
    pub expires_at: DateTimeWithTimeZone,
    #[sea_orm(nullable)]
    pub used_at: Option<DateTimeWithTimeZone>,
    #[sea_orm(nullable)]
    pub hostname: Option<String>,
    #[sea_orm(nullable)]
    pub os: Option<String>,
    #[sea_orm(nullable)]
    pub arch: Option<String>,
    #[sea_orm(nullable)]
    pub agent_version: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
