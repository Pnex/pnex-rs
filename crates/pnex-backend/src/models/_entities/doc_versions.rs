//! `SeaORM` Entity — named version of a page (append-only, P5): its
//! frozen Yjs state.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "doc_versions")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub doc_id: Uuid,
    pub org_id: i64,
    pub version_number: i64,
    pub label: String,
    #[sea_orm(column_type = "VarBinary(StringLen::None)")]
    pub snapshot: Vec<u8>,
    pub created_by: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
