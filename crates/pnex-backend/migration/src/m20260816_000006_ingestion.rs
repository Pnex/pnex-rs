//! Ingestion Phase 5 — état live device en PG (D9 : remplace Redis db2) +
//! correspondance org PNEX ↔ org OpenObserve (D2 : 1 org O2 par org, token
//! d'ingestion correlé stocké en base pour le chemin d'ingestion).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // Bail de vie par device : last_seen rafraîchi à chaque frame valide
        // (throttlé), consulté au connect (anti-clone 4003) et par le reaper
        // (seul écrivain de device_registries.active — parité Django).
        create_table(
            m,
            "device_state",
            &[
                ("id", ColType::PkAuto),
                ("device_registry_id", ColType::BigIntegerUniq),
                ("last_seen_at", ColType::TimestampWithTimeZone),
                ("connected", ColType::BooleanWithDefault(false)),
            ],
            &[("device_registries", "device_registry_id")],
        )
        .await?;

        // Provisionnement OpenObserve par org (paresseux, première frame) :
        // org O2 + token d'ingestion créés via l'API admin et conservés ici.
        create_table(
            m,
            "openobserve_org",
            &[
                ("id", ColType::PkAuto),
                ("org_id", ColType::BigIntegerUniq),
                // Nom stable de l'org côté OpenObserve (pnex-org-{id}).
                ("o2_org", ColType::StringLenUniq(255)),
                // Token d'ingestion OpenObserve (Basic auth _bulk) — rempli
                // par le provisioning, secret jamais exposé par l'API.
                ("ingestion_token", ColType::TextNull),
                (
                    "status",
                    ColType::EnumWithDefault(
                        "openobserve_org_status".to_string(),
                        vec![
                            "pending".to_string(),
                            "provisioned".to_string(),
                            "failed".to_string(),
                        ],
                        "pending".to_string(),
                    ),
                ),
                ("last_error", ColType::TextNull),
            ],
            &[("organizations", "org_id")],
        )
        .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "openobserve_org").await?;
        drop_table(m, "device_state").await?;
        drop_enum_type(m, "openobserve_org_status").await?;
        Ok(())
    }
}
