//! QR tags (parts-docs.md D962): a printed QR carries a short address to a
//! stable token, never data in clear. A token is blank (printed ahead, on a
//! sheet), linked to any object of the org, or revoked.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE qr_tags (
                    token      text PRIMARY KEY,
                    org_id     bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                    object_id  uuid REFERENCES objects(id) ON DELETE SET NULL,
                    state      text NOT NULL DEFAULT 'blank',
                    sheet_id   uuid,
                    created_at timestamptz NOT NULL DEFAULT now(),
                    linked_at  timestamptz,
                    linked_by  bigint REFERENCES users(id) ON DELETE SET NULL,
                    revoked_at timestamptz,
                    CONSTRAINT qr_tags_state CHECK (state IN ('blank', 'linked', 'revoked')),
                    CONSTRAINT qr_tags_linked CHECK (state <> 'linked' OR object_id IS NOT NULL)
                );
                CREATE INDEX qr_tags_object ON qr_tags (org_id, object_id) WHERE object_id IS NOT NULL;
                CREATE INDEX qr_tags_sheet ON qr_tags (org_id, sheet_id) WHERE sheet_id IS NOT NULL;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS qr_tags;")
            .await?;
        Ok(())
    }
}
