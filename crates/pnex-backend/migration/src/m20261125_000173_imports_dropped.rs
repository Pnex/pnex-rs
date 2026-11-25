//! Imports are gone (pipelines.md D1050, step 5): their tables go once the
//! boot conversion emptied them. A table still holding imports stays, with
//! its runs, for the boot conversion: a later release drops it. Builds that
//! name an import go: no build runs one any more. `datasets.import_id`
//! stays: the guard of an import's table still reads it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM data_builds \
                   WHERE target_kind = 'import' OR steps @> '[{\"kind\": \"import\"}]'; \
                 DO $$ \
                 BEGIN \
                   IF to_regclass('object_imports') IS NOT NULL THEN \
                     IF NOT EXISTS (SELECT 1 FROM object_imports) THEN \
                       DROP TABLE object_import_runs; \
                       DROP TABLE object_imports CASCADE; \
                     END IF; \
                   END IF; \
                 END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        // Nothing to bring back: the imports were converted into pipelines.
        Ok(())
    }
}
