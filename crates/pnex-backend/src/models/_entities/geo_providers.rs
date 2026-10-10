//! `SeaORM` Entity, hand-written on the generated shape (geo-layers.md §8).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Org geo provider (L16). Never serialized to a client as is: the secret
/// reference stays server-side (R4, R16).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "geo_providers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    /// Owning org (no platform fallback, D119).
    pub org_id: i64,
    pub name: String,
    /// `pnex_core::geo::GeoProviderKind`, snake_case.
    pub kind: String,
    /// JSON array of `pnex_core::geo::GeoCapability`.
    pub capabilities: Json,
    pub base_url: String,
    /// Vault secret holding the API key (`secret_usages` tracks the use).
    pub secret_id: Option<Uuid>,
    /// Query parameter the key is sent in.
    pub key_param: String,
    /// Non-secret query parameters (language, country, profile).
    pub params: Json,
    pub rate_limit_per_s: Option<f32>,
    pub timeout_ms: i32,
    pub store_allowed: bool,
    pub created_by: Option<i64>,
    pub updated_by: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::organizations::Entity",
        from = "Column::OrgId",
        to = "super::organizations::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Organizations,
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}
