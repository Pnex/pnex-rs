//! `SeaORM` Entity — comment of a page (P12): a thread root carries the
//! anchor, replies point to it with `parent_id`.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "doc_comments")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub doc_id: Uuid,
    pub org_id: i64,
    pub parent_id: Option<i64>,
    #[sea_orm(column_type = "Text", nullable)]
    pub anchor: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub quote: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub body: String,
    pub author_id: Option<i64>,
    pub resolved: bool,
    pub resolved_by: Option<i64>,
    pub version_number: Option<i64>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
