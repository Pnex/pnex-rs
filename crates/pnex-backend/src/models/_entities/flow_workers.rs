//! `SeaORM` Entity — flow execution cluster worker registry (D106).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "flow_workers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub boot: i64,
    pub advertise_url: String,
    pub capacity: i32,
    pub draining: bool,
    pub started_at: DateTimeWithTimeZone,
    pub heartbeat_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
