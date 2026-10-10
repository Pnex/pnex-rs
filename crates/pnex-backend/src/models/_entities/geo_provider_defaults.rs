//! `SeaORM` Entity, hand-written on the generated shape (geo-layers.md L16).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Default provider of an org for one capability.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "geo_provider_defaults")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: i64,
    /// `pnex_core::geo::GeoCapability`, snake_case.
    #[sea_orm(primary_key, auto_increment = false)]
    pub capability: String,
    pub provider_id: Uuid,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::geo_providers::Entity",
        from = "Column::ProviderId",
        to = "super::geo_providers::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    GeoProviders,
}

impl Related<super::geo_providers::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::GeoProviders.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
