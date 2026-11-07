//! Site recipe runs move into the unified run model (agent-unification.md D781, lot U1):
//! a `server_browser` job holds the state, an `agent_runs` row what the recipe played.
//! The view `recipe_runs` keeps the old journal shape for the readers.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "ALTER TABLE agent_runs
                   ADD COLUMN recipe_id uuid REFERENCES site_recipes(id) ON DELETE SET NULL,
                   ADD COLUMN recipe_version integer,
                   ADD COLUMN site varchar(255),
                   ADD COLUMN name varchar(120),
                   ADD COLUMN plan jsonb,
                   ADD COLUMN trigger varchar(16),
                   ADD COLUMN failed_step integer,
                   ADD COLUMN reason varchar(48),
                   ADD COLUMN steps jsonb NOT NULL DEFAULT '[]',
                   ADD COLUMN extracts jsonb NOT NULL DEFAULT '[]',
                   ADD COLUMN keys jsonb NOT NULL DEFAULT '{}';
                 CREATE INDEX agent_runs_site ON agent_runs (org_id, site, created_at DESC)
                   WHERE site IS NOT NULL;
                 CREATE INDEX agent_runs_recipe ON agent_runs (recipe_id, created_at DESC)
                   WHERE recipe_id IS NOT NULL;
                 -- A run's author may be gone: kept unknown, like ON DELETE SET NULL.
                 INSERT INTO runner_jobs (id, org_id, kind, spec, state, created_by, created_at,
                     started_at, finished_at, queue_expires_at)
                   SELECT id, org_id, 'server_browser', '{}',
                     CASE status WHEN 'ok' THEN 'succeeded' WHEN 'running' THEN 'running' ELSE 'failed' END,
                     (SELECT u.id FROM users u WHERE u.id = r.created_by),
                     started_at, started_at, finished_at, started_at
                   FROM site_recipe_runs r;
                 INSERT INTO agent_runs (id, org_id, job_id, llm_calls_max, created_by, created_at,
                     recipe_id, recipe_version, site, name, plan, trigger, failed_step, reason,
                     steps, extracts, keys)
                   SELECT id, org_id, id, 1, (SELECT u.id FROM users u WHERE u.id = r.created_by),
                     started_at, recipe_id, recipe_version,
                     site, name, jsonb_build_object('yaml', yaml, 'params', params), trigger,
                     failed_step, reason, steps, extracts,
                     CASE WHEN jsonb_typeof(keys) = 'object' THEN keys ELSE '{}' END
                   FROM site_recipe_runs r;
                 DROP TABLE site_recipe_runs;
                 CREATE VIEW recipe_runs AS
                   SELECT a.id, a.org_id, a.recipe_id, a.recipe_version, a.site, a.name,
                     a.plan ->> 'yaml' AS yaml, a.plan -> 'params' AS params, a.trigger,
                     (CASE j.state WHEN 'succeeded' THEN 'ok' WHEN 'running' THEN 'running'
                        WHEN 'queued' THEN 'running' ELSE 'failed' END)::varchar(16) AS status,
                     a.failed_step, a.reason, a.steps, a.extracts, a.keys, a.created_by,
                     COALESCE(j.started_at, a.created_at) AS started_at, j.finished_at,
                     j.kind AS engine
                   FROM agent_runs a JOIN runner_jobs j ON j.id = a.job_id
                   WHERE j.kind IN ('server_browser', 'extension');",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DROP VIEW recipe_runs;
                 CREATE TABLE site_recipe_runs (
                     id uuid PRIMARY KEY,
                     org_id bigint NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                     recipe_id uuid REFERENCES site_recipes(id) ON DELETE SET NULL,
                     recipe_version integer,
                     site varchar(255) NOT NULL,
                     name varchar(120) NOT NULL,
                     yaml text NOT NULL,
                     params jsonb NOT NULL DEFAULT '{}',
                     trigger varchar(16) NOT NULL,
                     status varchar(16) NOT NULL,
                     failed_step integer,
                     reason varchar(48),
                     steps jsonb NOT NULL DEFAULT '[]',
                     extracts jsonb NOT NULL DEFAULT '[]',
                     keys jsonb NOT NULL DEFAULT '[]',
                     created_by bigint,
                     started_at timestamptz NOT NULL DEFAULT now(),
                     finished_at timestamptz
                 );
                 CREATE INDEX idx_site_recipe_runs_site ON site_recipe_runs (org_id, site, started_at DESC);
                 CREATE INDEX idx_site_recipe_runs_recipe ON site_recipe_runs (recipe_id, started_at DESC);
                 DELETE FROM runner_jobs WHERE kind = 'server_browser';
                 ALTER TABLE agent_runs
                   DROP COLUMN recipe_id, DROP COLUMN recipe_version, DROP COLUMN site,
                   DROP COLUMN name, DROP COLUMN plan, DROP COLUMN trigger,
                   DROP COLUMN failed_step, DROP COLUMN reason, DROP COLUMN steps,
                   DROP COLUMN extracts, DROP COLUMN keys;",
            )
            .await?;
        Ok(())
    }
}
