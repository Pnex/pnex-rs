//! Firmware — build records. Django : timestamp auto_now (≈ created_at),
//! argo_wf_job_name SUPPRIMÉ (plus d'Argo), sentinelle s3_key abandonnée
//! (colonne nullable, clé générée par hook Phase 6).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "build_record",
            &[
                ("id", ColType::PkAuto),
                ("device_id", ColType::StringLenNull(255)),
                ("success", ColType::BooleanWithDefault(false)),
                ("build_phase", ColType::StringLenNull(255)),
                // Clé d'artefact dans l'ArtifactStore (S3-compatible, D5) —
                // remplie par le worker de build (Phase 6).
                ("firmware_bin_s3_key", ColType::StringLenNull(255)),
            ],
            &[("?organizations", "org_id")],
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "build_record").await?;
        Ok(())
    }
}
