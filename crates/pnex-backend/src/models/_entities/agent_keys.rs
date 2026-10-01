//! `SeaORM` Entity — free-form key discovered from an edge agent (D95).
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "agent_keys")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub device_registry_id: i64,
    pub key: String,
    #[sea_orm(nullable)]
    pub unit: Option<String>,
    /// `number` | `bool` | `text` | `json`.
    pub kind: String,
    /// OpenObserve recording toggle (values always reach Valkey).
    pub record_o2: bool,
    pub first_seen_at: DateTimeWithTimeZone,
    pub last_seen_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
