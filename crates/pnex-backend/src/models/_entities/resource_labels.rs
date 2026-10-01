//! `SeaORM` Entity — couche d'organisation transverse (D42).
//!
//! Classification requêtable : 1 doc JSONB par ressource
//! (unique `(org_id, resource_kind, resource_id)`), index GIN PG.
//! `labels` nullable en DB (portable) — le service garantit la sémantique
//! non-null (`COALESCE(labels, '{}')` au read, toujours `Some` à l'écrit).

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "resource_labels")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub resource_kind: String,
    pub resource_id: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub labels: Option<Json>,
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

impl ActiveModelBehavior for ActiveModel {}
