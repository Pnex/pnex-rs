//! `SeaORM` Entity — couche d'organisation transverse (D42).
//!
//! Arêtes many-to-many avec `placement` (opaque pour le moteur, structuré
//! par relation). Absorbe `viz_links` (D39 → D42). Unicité de paire
//! `(org, relation, source, target)` en base ; purge symétrique des deux
//! bouts via `purge_for`.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "resource_edges")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub relation: String,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub placement: Option<Json>,
    /// Link attributes, validated by the link type (D179).
    #[sea_orm(column_type = "JsonBinary")]
    pub attributes: Json,
    /// Validity: a closed link (`valid_to` set) is history, never deleted.
    pub valid_from: DateTimeWithTimeZone,
    pub valid_to: Option<DateTimeWithTimeZone>,
    /// Provenance (D184).
    pub source_ref: Option<String>,
    /// Identities of both ends, resolved by a trigger on insert.
    pub source_object_id: Option<Uuid>,
    pub target_object_id: Option<Uuid>,
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
