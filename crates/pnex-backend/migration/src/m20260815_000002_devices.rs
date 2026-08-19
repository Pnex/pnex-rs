//! Devices — catalogue global (types, capabilities, MCU, predefined) +
//! registre scopé par org (D2) + tokens (hook génération en Phase 4).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "device_type",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLenUniq(100)),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "device_capability",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLenUniq(255)),
                (
                    "mode",
                    ColType::EnumWithDefault(
                        "capability_mode".to_string(),
                        vec![
                            "input".to_string(),
                            "output".to_string(),
                            "input_output".to_string(),
                        ],
                        "input".to_string(),
                    ),
                ),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "mcu_board",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLen(255)),
                (
                    "soc",
                    ColType::StringLenWithDefault(255, "esp32".to_string()),
                ),
                ("details", ColType::JsonBinaryNull),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "predefined_device",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringUniq),
                ("pretty_name", ColType::StringLenNull(255)),
                (
                    "revision",
                    ColType::StringLenWithDefault(50, "".to_string()),
                ),
                ("device_doc_url", ColType::StringLenNull(1024)),
                ("prestashop_product_id", ColType::StringLenNull(64)),
                ("prestashop_buy_url", ColType::StringLenNull(1024)),
                ("byod_doc_url", ColType::StringLenNull(1024)),
                ("image_source_url", ColType::StringLenNull(1024)),
                ("stl_files_url", ColType::StringLenNull(1024)),
                ("description", ColType::TextNull),
            ],
            &[("device_types", ""), ("mcu_boards", "board_id")],
        )
        .await?;

        // M2M predefined_device ↔ device_capability
        create_join_table(
            m,
            "predefined_device_capability",
            &[],
            &[("predefined_devices", ""), ("device_capabilities", "")],
        )
        .await?;

        create_table(
            m,
            "device_registry",
            &[
                ("id", ColType::PkAuto),
                // Identifiant device déclaré par le firmware (ex. MAC/hostname).
                ("device_id", ColType::StringLen(255)),
                ("metadata", ColType::JsonBinaryNull),
                ("active", ColType::BooleanWithDefault(false)),
                (
                    "allow_dynamic_measurements",
                    ColType::BooleanWithDefault(true),
                ),
                ("discovered_measurements", ColType::JsonBinaryNull),
                ("max_unique_measurements", ColType::IntegerWithDefault(100)),
            ],
            // D2 : le scoping passe de user_id (Django) à org_id.
            &[("organizations", "org_id"), ("predefined_devices", "")],
        )
        .await?;

        // Django : unique (user, device_id) → unique (org_id, device_id).
        create_table(
            m,
            "device_token",
            &[
                ("id", ColType::PkAuto),
                ("token", ColType::StringUniq),
                // Clé ChaCha20 encodée base64 (32 octets) — générée par hook
                // avant insert (Phase 4, parité save() Django).
                ("encryption_key", ColType::StringLenNull(64)),
                ("is_active", ColType::BooleanWithDefault(true)),
            ],
            // Un token par device (Django : unique (user, device)).
            &[("device_registries", "device_registry_id")],
        )
        .await?;

        m.get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX uniq_device_registries_org_device_id ON device_registries (org_id, device_id);
                 CREATE UNIQUE INDEX uniq_device_tokens_device_registry ON device_tokens (device_registry_id);
                 CREATE UNIQUE INDEX uniq_predefined_devices_prestashop_product_id ON predefined_devices (prestashop_product_id) WHERE prestashop_product_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "device_token").await?;
        drop_table(m, "device_registry").await?;
        drop_table(m, "predefined_device_capability").await?;
        drop_table(m, "predefined_device").await?;
        drop_table(m, "mcu_board").await?;
        drop_table(m, "device_capability").await?;
        drop_table(m, "device_type").await?;
        drop_enum_type(m, "capability_mode").await?;
        Ok(())
    }
}
