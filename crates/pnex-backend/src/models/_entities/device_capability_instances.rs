//! `SeaORM` Entity — table `device_capability_instances` (Brick 0).
//!
//! Génération à la main (conforme au style codegen) : la sortie de
//! `cargo loco db entities` est identique. GPIO en i32 (colonne Integer),
//! mode en string (digital_in | digital_out | analog_in).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "device_capability_instances")]
pub struct Model {
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
    #[sea_orm(primary_key)]
    pub id: i64,
    pub device_registry_id: i64,
    pub gpio: i32,
    pub label: String,
    pub mode: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub config: Option<Json>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub constraints_snapshot: Option<Json>,
    pub enabled: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::device_registries::Entity",
        from = "Column::DeviceRegistryId",
        to = "super::device_registries::Column::Id",
        on_update = "Cascade",
        on_delete = "Cascade"
    )]
    DeviceRegistries,
}

impl Related<super::device_registries::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::DeviceRegistries.def()
    }
}

impl ActiveModelBehavior for ActiveModel {
    // Pas de hooks — updated_at/created_at gérés par les défauts de colonnes
    // (convention repo : les timestamps loco sont posés par create_table).
}
