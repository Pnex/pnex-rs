//! `SeaORM` Entity (hand-written on the generated shape).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Device certificate authority of an org (D153, lot L3). The private key
/// is a vault platform secret (`key_secret_id`), never stored here.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "org_device_cas")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub org_id: i64,
    #[sea_orm(column_type = "Text")]
    pub cert_pem: String,
    pub key_secret_id: Uuid,
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
}

impl Related<super::organizations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Organizations.def()
    }
}
