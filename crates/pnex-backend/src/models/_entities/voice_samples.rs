//! `SeaORM` Entity (hand-written on the generated shape).
//! Enrolment clips of a voice profile (voice-id.md).

use sea_orm::entity::prelude::*;

/// One enrolment clip: an audio media asset and what the last generation
/// measured on it.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "voice_samples")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub profile_id: Uuid,
    pub media_asset_id: Uuid,
    pub duration_ms: Option<i32>,
    pub speech_ms: Option<i32>,
    pub windows: Option<i32>,
    #[sea_orm(column_type = "Float", nullable)]
    pub score: Option<f32>,
    pub status: String,
    pub reason: Option<String>,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
