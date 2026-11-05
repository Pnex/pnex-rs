//! `SeaORM` Entity — named row-based leases (D106).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "flow_leases")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    pub holder: String,
    pub expires_at: DateTimeWithTimeZone,
    /// Server version of the holder (`app_version`), set when it renews.
    pub holder_version: String,
    /// When this holder took the lease: unchanged by a renewal.
    pub taken_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
