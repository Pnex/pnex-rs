//! Where a user's own network account stands with a person (crm.md
//! D977): declared on the person's page, it gates the exploration of that
//! person's connections (D976). One row per user, person and network.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE network_connections (
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                   person_id uuid NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
                   network varchar(32) NOT NULL,
                   state varchar(24) NOT NULL
                     CHECK (state IN ('none', 'invitation_sent', 'connected')),
                   since timestamptz NOT NULL DEFAULT now(),
                   explored_at timestamptz,
                   PRIMARY KEY (org_id, user_id, person_id, network)
                 );",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE network_connections;")
            .await?;
        Ok(())
    }
}
