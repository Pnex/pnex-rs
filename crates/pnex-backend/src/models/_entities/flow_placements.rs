//! `SeaORM` Entity — org → flow worker placement (D106).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "flow_placements")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: i64,
    pub worker_id: String,
    pub epoch: i64,
    pub revision: i64,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
