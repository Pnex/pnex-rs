//! Identité & tenancy — D2 : l'organisation est le tenant, plusieurs users
//! par org (membership avec rôle), le tier d'abonnement s'attache à l'org (D11).

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // Miroir minimal des comptes de l'IdP (Rauthy) — provisioning JIT en
        // Phase 3. Pas de mot de passe : l'auth user est JWT IdP uniquement
        // (D10). `idp_sub` = `sub` de l'IdP (Rauthy : 24 caractères
        // alphanumériques — pas un UUID, héritage Keycloak).
        create_table(
            m,
            "user",
            &[
                ("id", ColType::PkAuto),
                ("idp_sub", ColType::StringLenNull(64)),
                ("email", ColType::StringLenUniq(255)),
                ("full_name", ColType::StringLenNull(255)),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "subscription_tier",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLenUniq(100)),
                ("max_sensor_devices", ColType::Integer),
                ("max_actuator_devices", ColType::Integer),
                ("max_mixed_devices", ColType::Integer),
                // Django : DurationField INTERVAL PG. Rust = version officielle :
                // secondes entières, plus simples à typer côté SeaORM.
                (
                    "min_build_interval_secs",
                    ColType::BigIntegerWithDefault(900),
                ),
                ("data_retention_secs", ColType::BigIntegerNull),
            ],
            &[],
        )
        .await?;

        create_table(
            m,
            "organization",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLenUniq(255)),
            ],
            // D11 : l'abonnement est porté par l'org, pas par le user.
            // Référence nullable (`?`) : colonne nullable + ON DELETE SET NULL.
            &[("subscription_tiers?", "subscription_tier_id")],
        )
        .await?;

        create_table(
            m,
            "organization_member",
            &[
                ("id", ColType::PkAuto),
                (
                    "role",
                    ColType::EnumWithDefault(
                        "org_member_role".to_string(),
                        vec![
                            "owner".to_string(),
                            "admin".to_string(),
                            "viewer".to_string(),
                        ],
                        "viewer".to_string(),
                    ),
                ),
            ],
            &[("users", "user_id"), ("organizations", "org_id")],
        )
        .await?;

        create_table(
            m,
            "user_profile",
            &[
                ("id", ColType::PkAuto),
                (
                    "language",
                    ColType::StringLenWithDefault(10, "en".to_string()),
                ),
                (
                    "timezone",
                    ColType::StringLenWithDefault(50, "UTC".to_string()),
                ),
                ("date_format", ColType::StringLenNull(20)),
                (
                    "theme",
                    ColType::EnumWithDefault(
                        "ui_theme".to_string(),
                        vec!["light".to_string(), "dark".to_string(), "auto".to_string()],
                        "auto".to_string(),
                    ),
                ),
                ("preferences", ColType::JsonBinaryNull),
                ("grafana_url", ColType::StringLenNull(500)),
                (
                    "llm_endpoint_openapi_compatible",
                    ColType::StringLenNull(500),
                ),
                ("llm_token", ColType::StringLenNull(500)),
                ("llm_model", ColType::StringLenNull(100)),
            ],
            &[("users", "user_id")],
        )
        .await?;

        // Index uniques composites / partiels (bruts : la DSL ne couvre pas).
        m.get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX uniq_users_idp_sub ON users (idp_sub) WHERE idp_sub IS NOT NULL;
                 CREATE UNIQUE INDEX uniq_organization_members_org_user ON organization_members (org_id, user_id);
                 CREATE UNIQUE INDEX uniq_user_profiles_user ON user_profiles (user_id);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        drop_table(m, "user_profile").await?;
        drop_table(m, "organization_member").await?;
        drop_table(m, "organization").await?;
        drop_table(m, "subscription_tier").await?;
        drop_table(m, "user").await?;
        drop_enum_type(m, "ui_theme").await?;
        drop_enum_type(m, "org_member_role").await?;
        Ok(())
    }
}
