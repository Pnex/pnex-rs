//! D497: a function's ports live in `function_versions.inputs/outputs`,
//! never in the code. Those columns were already filled on save from the
//! `@input`/`@output` comment directives; this removes the directive lines
//! from the stored code. One-way: the directives are no longer read.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // `n` flag: `^` and `$` match at line breaks.
        m.get_connection()
            .execute_unprepared(
                r"UPDATE function_versions
                  SET code = regexp_replace(code,
                      '^[ \t]*(//|#)[ \t]*@(input|output)([ \t][^\n]*)?(\n|$)', '', 'gn')
                  WHERE code ~ '@(input|output)';",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
