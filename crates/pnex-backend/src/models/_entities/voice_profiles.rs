//! `SeaORM` Entity (hand-written on the generated shape).
//! Voice profiles (voice-id.md, cases.md K19).

use sea_orm::entity::prelude::*;

/// One consented voice profile per person object. The centroid is sealed
/// with the secrets keyring (AAD = org + profile id): no `Serialize`, the
/// row never leaves the server as is.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "voice_profiles")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: i64,
    pub object_id: Uuid,
    pub model_id: Option<Uuid>,
    pub status: String,
    pub consent_at: DateTimeWithTimeZone,
    pub consent_scope: String,
    pub consent_proof: String,
    pub consent_recorded_by: String,
    pub centroid_ciphertext: Option<Vec<u8>>,
    pub centroid_nonce: Option<Vec<u8>>,
    pub centroid_key_id: Option<String>,
    pub dim: Option<i32>,
    #[sea_orm(column_type = "Float", nullable)]
    pub threshold: Option<f32>,
    pub windows_used: i32,
    #[sea_orm(column_type = "Float", nullable)]
    pub intra_mean: Option<f32>,
    #[sea_orm(column_type = "Float", nullable)]
    pub intra_std: Option<f32>,
    pub trained_at: Option<DateTimeWithTimeZone>,
    pub last_error: Option<String>,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
