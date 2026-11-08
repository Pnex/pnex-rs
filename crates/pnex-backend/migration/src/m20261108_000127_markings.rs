//! Data governance (governance.md D798–D803): markings of an org, time-bound
//! clearances tied to a purpose, the markings of tables, and their inheritance
//! through lineage, done by the database so that no write path can skip it. The
//! trigger runs as its owner: the worker's restricted login needs no right on markings.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "CREATE TABLE markings (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   key text NOT NULL,
                   name text NOT NULL,
                   description text NOT NULL DEFAULT '',
                   created_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now(),
                   UNIQUE (org_id, key)
                 );
                 CREATE TABLE clearances (
                   id uuid PRIMARY KEY,
                   org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                   user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                   marking_id uuid NOT NULL REFERENCES markings(id) ON DELETE CASCADE,
                   purpose text NOT NULL,
                   expires_at timestamptz NOT NULL,
                   revoked_at timestamptz,
                   granted_by bigint REFERENCES users(id) ON DELETE SET NULL,
                   created_at timestamptz NOT NULL DEFAULT now()
                 );
                 CREATE INDEX clearances_active ON clearances (org_id, user_id) WHERE revoked_at IS NULL;
                 CREATE TABLE dataset_markings (
                   dataset_id uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
                   marking_id uuid NOT NULL REFERENCES markings(id) ON DELETE RESTRICT,
                   PRIMARY KEY (dataset_id, marking_id)
                 );
                 CREATE FUNCTION dataset_markings_inherit() RETURNS trigger AS $$
                 BEGIN
                   INSERT INTO dataset_markings (dataset_id, marking_id)
                   SELECT o.dataset_id, m.marking_id
                     FROM dataset_transactions o, dataset_transactions i, dataset_markings m
                    WHERE o.id = NEW.transaction_id AND i.id = NEW.input_transaction_id
                      AND m.dataset_id = i.dataset_id
                   ON CONFLICT DO NOTHING;
                   RETURN NEW;
                 END $$ LANGUAGE plpgsql SECURITY DEFINER SET search_path = public;
                 CREATE TRIGGER dataset_markings_inherit AFTER INSERT ON dataset_transaction_inputs
                   FOR EACH ROW EXECUTE FUNCTION dataset_markings_inherit();",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP TRIGGER dataset_markings_inherit ON dataset_transaction_inputs;
                 DROP FUNCTION dataset_markings_inherit();
                 DROP TABLE dataset_markings; DROP TABLE clearances; DROP TABLE markings;",
            )
            .await?;
        Ok(())
    }
}
