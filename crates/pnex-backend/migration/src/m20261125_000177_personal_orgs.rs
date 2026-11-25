//! The personal org of a user (crm.md D999): the org provisioning made
//! for them, where nobody receives a share. Backfill: each user's lowest-id
//! org where they are the only owner; a user with several such orgs is
//! reported, since the guess may be wrong.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE organizations
                   ADD COLUMN personal_of bigint REFERENCES users(id) ON DELETE SET NULL;
                 CREATE UNIQUE INDEX organizations_personal_of ON organizations (personal_of);
DO $$
DECLARE r record;
BEGIN
  FOR r IN
    WITH sole AS (
      SELECT org_id, min(user_id) AS user_id FROM organization_members
      WHERE role = 'owner' GROUP BY org_id HAVING count(*) = 1
    )
    SELECT user_id, min(org_id) AS org_id, count(*) AS n FROM sole GROUP BY user_id
  LOOP
    UPDATE organizations SET personal_of = r.user_id WHERE id = r.org_id;
    IF r.n > 1 THEN
      RAISE NOTICE 'personal org of user %: % orgs where they are the only owner, org % chosen', r.user_id, r.n, r.org_id;
    END IF;
  END LOOP;
END $$;",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared("ALTER TABLE organizations DROP COLUMN personal_of;")
            .await?;
        Ok(())
    }
}
