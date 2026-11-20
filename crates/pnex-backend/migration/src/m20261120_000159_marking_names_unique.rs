//! A marking name is unique in its org, case aside (governance.md, choice of
//! 2026-11-20): two markings with one name made pick lists ambiguous. Names
//! already shared get their key appended before the index is built.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "UPDATE markings m SET name = m.name || ' (' || m.key || ')'
                 WHERE EXISTS (SELECT 1 FROM markings o
                               WHERE o.org_id = m.org_id AND lower(o.name) = lower(m.name)
                                 AND o.key < m.key);
                 CREATE UNIQUE INDEX markings_org_name ON markings (org_id, lower(name));",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS markings_org_name;")
            .await?;
        Ok(())
    }
}
