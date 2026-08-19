//! Firmware — artefacts en base (D5 v2 : tier postgres/sqlite, tout sur la DB).
//!
//! Binaires mergés ESP 1–4 Mo ; la clé logique `org_{id}/firmware/{device}-firmware.bin`
//! est stable par device → `put()` en upsert, zéro artefact orphelin. Pas de FK org :
//! l'org est embarqué dans la clé, la rétention reste D6. `Blob` → BYTEA (PG) / BLOB (sqlite).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "firmware_artifact",
            &[
                ("id", ColType::PkAuto),
                // Clé logique de l'ArtifactStore — unicité requise pour l'upsert ON CONFLICT.
                ("key", ColType::StringLenUniq(255)),
                ("bytes", ColType::Blob),
                ("size_bytes", ColType::BigInteger),
                ("sha256", ColType::StringLenNull(64)),
            ],
            &[],
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "firmware_artifact").await?;
        Ok(())
    }
}
