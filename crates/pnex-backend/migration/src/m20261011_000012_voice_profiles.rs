//! Voice profiles (voice-id.md, cases.md K19 variant A): one consented
//! profile per person object, its enrolment clips kept as audio media
//! assets, the centroid sealed with the secrets keyring (never in clear).
//! Deleting the person, or revoking the consent, deletes the profile.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
CREATE TABLE voice_profiles (
    id                  uuid PRIMARY KEY,
    org_id              bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    object_id           uuid NOT NULL UNIQUE REFERENCES objects(id) ON DELETE CASCADE,
    model_id            uuid REFERENCES ml_models(id) ON DELETE SET NULL,
    status              text NOT NULL DEFAULT 'draft',
    consent_at          timestamptz NOT NULL,
    consent_scope       text NOT NULL,
    consent_proof       text NOT NULL,
    consent_recorded_by text NOT NULL,
    centroid_ciphertext bytea,
    centroid_nonce      bytea,
    centroid_key_id     text,
    dim                 integer,
    threshold           real,
    windows_used        integer NOT NULL DEFAULT 0,
    intra_mean          real,
    intra_std           real,
    trained_at          timestamptz,
    last_error          text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT voice_profiles_status CHECK (status IN ('draft', 'ready', 'failed'))
);
CREATE INDEX voice_profiles_org ON voice_profiles (org_id, status);

CREATE TABLE voice_samples (
    id             uuid PRIMARY KEY,
    org_id         bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    profile_id     uuid NOT NULL REFERENCES voice_profiles(id) ON DELETE CASCADE,
    media_asset_id uuid NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
    duration_ms    integer,
    speech_ms      integer,
    windows        integer,
    score          real,
    status         text NOT NULL DEFAULT 'pending',
    reason         text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (profile_id, media_asset_id),
    CONSTRAINT voice_samples_status CHECK (status IN ('pending', 'used', 'rejected'))
);
CREATE INDEX voice_samples_profile ON voice_samples (profile_id);
"#;

const DOWN: &str = r#"
DROP TABLE IF EXISTS voice_samples;
DROP TABLE IF EXISTS voice_profiles;
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}
