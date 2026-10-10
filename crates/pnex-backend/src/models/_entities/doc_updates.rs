//! `SeaORM` Entity — append-only Yjs update log of a page, folded into
//! `docs.snapshot` by the compaction.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "doc_updates")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub doc_id: Uuid,
    #[sea_orm(column_type = "VarBinary(StringLen::None)")]
    pub data: Vec<u8>,
    pub user_id: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
