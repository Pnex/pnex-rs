//! `SeaORM` Entity (hand-written on the generated shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A device certificate issued by its org CA (D153, lot L3): identity of
/// the device on the TLS link. Revocation = `revoked_at` (no CRL).
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "device_certificates")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub device_registry_id: i64,
    pub serial: String,
    #[sea_orm(unique)]
    pub fingerprint_sha256: String,
    pub not_after: DateTimeWithTimeZone,
    pub revoked_at: Option<DateTimeWithTimeZone>,
    pub created_at: DateTimeWithTimeZone,
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
    #[sea_orm(
        belongs_to = "super::device_registries::Entity",
        from = "Column::DeviceRegistryId",
        to = "super::device_registries::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    DeviceRegistries,
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}

impl Related<super::device_registries::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DeviceRegistries.def()
    }
}
