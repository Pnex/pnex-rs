//! `SeaORM` Entity — a PDF of a page version sent by a `page_pdf` node
//! (D209).

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "doc_version_exports")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub org_id: i64,
    pub doc_version_id: i64,
    /// `<flow id>:<node id>` of the exporting node.
    pub node_key: String,
    pub asset_id: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
