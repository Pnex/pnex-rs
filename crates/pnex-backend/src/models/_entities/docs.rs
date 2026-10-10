//! `SeaORM` Entity — one page (pages.md #21). The content is a Yjs state:
//! `snapshot` (compacted) + the `doc_updates` rows after `snapshot_seq`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "docs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub title: String,
    #[sea_orm(column_type = "VarBinary(StringLen::None)", nullable)]
    pub snapshot: Option<Vec<u8>>,
    pub snapshot_seq: i64,
    pub created_by: Option<i64>,
    /// Universal identity row (`objects`), filled by the database trigger.
    pub object_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
