//! D18 — flows ETL : identité (`flows`) + historique append-only
//! (`flow_versions`). La source de vérité du graphe est la base ; le
//! `flows.json` n'est qu'un artefact de déploiement projeté par Loco.
//!
//! Tables physiques : `flows` + `flow_versions` (`normalize_table` pluriélise
//! les noms DSL `flow` / `flow_version`).
//!
//! FK circulaire `flows.deployed_version_id` → `flow_versions.id` : posée en
//! SQL brut car l'helper refs de Loco ne peut pas référencer une table créée
//! dans la même migration après `flows`, et fige ON DELETE CASCADE. Sur le
//! tier sqlite, `ALTER TABLE ADD CONSTRAINT` n'existe pas : la colonne reste
//! sans contrainte (l'intégrité est portée par le contrôleur) — écart
//! documenté dans `docs/architecture/flow-engine.md`.

use loco_rs::schema::*;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const FK_DEPLOYED: &str = "fk-flows-deployed_version_id-to-flow_versions";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        create_table(
            m,
            "flow",
            &[
                ("id", ColType::PkAuto),
                ("name", ColType::StringLen(200)),
                // draft | deployed | error (code string, pas PG enum —
                // parité avec device_capability_instances.mode).
                ("status", ColType::StringLenWithDefault(32, "draft".into())),
            ],
            &[
                // D2 : le tenant est l'org — un flow suit sa org (CASCADE).
                ("organizations", "org_id"),
                // Attachement produit optionnel à un device (SET NULL si le
                // device disparaît : le flow survit, dé-lié).
                ("device_registries?", "device_registry_id"),
            ],
        )
        .await?;

        create_table(
            m,
            "flow_version",
            &[
                ("id", ColType::PkAuto),
                // Incrémental par flow (unique (flow_id, version_number)).
                ("version_number", ColType::BigInteger),
                // Miroir du modèle typé PNEX (pnex_core::flow::FlowGraph).
                ("graph", ColType::JsonBinary),
                ("author", ColType::StringLenNull(255)),
                ("note", ColType::TextNull),
            ],
            &[("flow", "flow_id")],
        )
        .await?;

        m.get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX uniq_flow_versions_flow_number \
                 ON flow_versions (flow_id, version_number);",
            )
            .await?;

        // FK circulaire : colonne ajoutée après coup, contrainte en SQL brut
        // (Postgres uniquement — voir doc de module pour le tier sqlite).
        m.alter_table(
            Table::alter()
                .table(Alias::new("flows"))
                .add_column(
                    ColumnDef::new(Alias::new("deployed_version_id")).big_integer().null(),
                )
                .to_owned(),
        )
        .await?;
        if m.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
            m.get_connection()
                .execute_unprepared(&format!(
                    "ALTER TABLE flows ADD CONSTRAINT \"{FK_DEPLOYED}\" \
                     FOREIGN KEY (deployed_version_id) REFERENCES flow_versions(id) \
                     ON DELETE SET NULL"
                ))
                .await?;
        }
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        if m.get_database_backend() == sea_orm::DatabaseBackend::Postgres {
            m.get_connection()
                .execute_unprepared(&format!("ALTER TABLE flows DROP CONSTRAINT IF EXISTS \"{FK_DEPLOYED}\""))
                .await?;
        }
        // La contrainte circulaire retirée, l'ordre de drop redevient libre.
        drop_table(m, "flow_version").await?;
        drop_table(m, "flow").await?;
        Ok(())
    }
}
