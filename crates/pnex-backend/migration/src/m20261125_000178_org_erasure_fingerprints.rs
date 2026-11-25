//! Fingerprints of erased people belong to the org, not to a Table
//! (pipelines.md D569, gap 2): a deleted Table keeps its rows, without its id,
//! and each row says which property it fingerprints, so that a Table declaring
//! that property later leaves the person out.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE dataset_erasures ADD COLUMN property text;
                 UPDATE dataset_erasures e SET property = i.property
                   FROM dataset_identity_columns i
                   WHERE i.dataset_id = e.dataset_id AND i.column_name = e.column_name
                     AND i.type_key = e.type_key;
                 ALTER TABLE dataset_erasures
                   ALTER COLUMN dataset_id DROP NOT NULL,
                   DROP CONSTRAINT dataset_erasures_dataset_id_fkey,
                   ADD CONSTRAINT dataset_erasures_dataset_id_fkey
                     FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE SET NULL;
                 CREATE INDEX dataset_erasures_kind ON dataset_erasures (org_id, type_key, property);",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP INDEX dataset_erasures_kind;
                 DELETE FROM dataset_erasures WHERE dataset_id IS NULL;
                 ALTER TABLE dataset_erasures
                   ALTER COLUMN dataset_id SET NOT NULL,
                   DROP CONSTRAINT dataset_erasures_dataset_id_fkey,
                   ADD CONSTRAINT dataset_erasures_dataset_id_fkey
                     FOREIGN KEY (dataset_id) REFERENCES datasets(id) ON DELETE CASCADE;
                 ALTER TABLE dataset_erasures DROP COLUMN property;",
            )
            .await?;
        Ok(())
    }
}
