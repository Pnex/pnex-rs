//! Brick 0 — capability instances : l'état live d'un pin d'un device
//! générique (mode courant + config + snapshot des contraintes validées à
//! l'admission). FK device_registries **cascade** (la carte de pins suit
//! le device), unique (device_registry_id, gpio) — adressage fil GPIO.

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "device_capability_instances",
            &[
                ("id", ColType::PkAuto),
                ("device_registry_id", ColType::BigInteger),
                // Adressage fil = GPIO (u16); le label overlay est dénormalisé.
                ("gpio", ColType::Integer),
                ("label", ColType::StringLen(64)),
                // digital_in | digital_out | analog_in (enum code, pas PG enum).
                ("mode", ColType::StringLen(32)),
                // pullup, safe_state, interval_ms — poussés via SetMode.
                ("config", ColType::JsonBinaryNull),
                // Ce qui a été validé par caps::validate à l'admission.
                ("constraints_snapshot", ColType::JsonBinaryNull),
                // Laissé enabled (désactivation sans perte de config).
                ("enabled", ColType::BooleanWithDefault(true)),
            ],
            &[("device_registries", "device_registry_id")],
        )
        .await?;
        m.get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX uniq_dci_device_gpio ON device_capability_instances (device_registry_id, gpio);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "device_capability_instances").await?;
        Ok(())
    }
}
