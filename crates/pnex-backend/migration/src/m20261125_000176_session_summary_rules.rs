//! The daily session summary becomes the action of an org rule
//! (markets-research.md D814, analysis.md D829; Shan's choice of 2026-11-25).
//! `rules.action` says what a rule does; a summary is claimed once per rule
//! and session. An org with `session_summary = on` gets one summary rule per
//! channel named in `move_notify_channels` that exists (the websocket kind
//! excepted: the in-app note always goes); unknown names, or no channel at
//! all, create nothing and are logged. The rule's author is the org's first
//! owner. Both settings leave the pack layer.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                r#"
ALTER TABLE rules ADD COLUMN IF NOT EXISTS action text NOT NULL DEFAULT 'notify';

DO $$
DECLARE
  p record;
  wanted text;
  ch record;
  author bigint;
  made int;
BEGIN
  FOR p IN SELECT org_id, settings FROM packs
           WHERE key = 'markets' AND lower(trim(settings ->> 'session_summary')) = 'on' LOOP
    made := 0;
    SELECT user_id INTO author FROM organization_members
      WHERE org_id = p.org_id AND role = 'owner' ORDER BY created_at, id LIMIT 1;
    FOREACH wanted IN ARRAY regexp_split_to_array(coalesce(p.settings ->> 'move_notify_channels', ''), '[,;]') LOOP
      wanted := trim(wanted);
      CONTINUE WHEN wanted = '';
      SELECT c.id, c.name AS n INTO ch FROM notify_channels c
        WHERE c.org_id = p.org_id AND c.name = wanted AND c.kind <> 'websocket' LIMIT 1;
      IF NOT FOUND THEN
        RAISE NOTICE 'session summary of org %: no e-mail, Slack or other channel named %, skipped', p.org_id, wanted;
        CONTINUE;
      END IF;
      INSERT INTO rules (id, org_id, name, condition, channel_id, created_by, action)
      VALUES (gen_random_uuid(), p.org_id, left('Session summary: ' || ch.n, 120),
              '{"kind":"objects","type_key":"market_event","filters":[{"key":"kind","op":"eq","value":"market_move"}]}'::jsonb,
              ch.id, author, 'session_summary')
      ON CONFLICT (org_id, name) DO NOTHING;
      made := made + 1;
    END LOOP;
    IF made = 0 THEN
      RAISE NOTICE 'session summary of org % was on with no valid channel: no rule created', p.org_id;
    END IF;
  END LOOP;
END $$;

UPDATE packs SET settings = settings - 'session_summary' - 'move_notify_channels'
  WHERE key = 'markets' AND jsonb_typeof(settings) = 'object';

ALTER TABLE market_session_summaries ADD COLUMN rule_id uuid REFERENCES rules(id) ON DELETE CASCADE;
ALTER TABLE market_session_summaries DROP CONSTRAINT market_session_summaries_pkey;
-- A session already summed up stays summed up for the rules made above.
INSERT INTO market_session_summaries (org_id, session, state, created_at, rule_id)
  SELECT s.org_id, s.session, s.state, s.created_at, r.id
  FROM market_session_summaries s JOIN rules r ON r.org_id = s.org_id AND r.action = 'session_summary'
  WHERE s.rule_id IS NULL;
DELETE FROM market_session_summaries WHERE rule_id IS NULL;
ALTER TABLE market_session_summaries ALTER COLUMN rule_id SET NOT NULL;
ALTER TABLE market_session_summaries ADD PRIMARY KEY (rule_id, session);
"#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection()
            .execute_unprepared(
                "DELETE FROM market_session_summaries;
                 ALTER TABLE market_session_summaries DROP CONSTRAINT market_session_summaries_pkey;
                 ALTER TABLE market_session_summaries DROP COLUMN rule_id;
                 ALTER TABLE market_session_summaries ADD PRIMARY KEY (org_id, session);
                 DELETE FROM rules WHERE action = 'session_summary';
                 ALTER TABLE rules DROP COLUMN action;",
            )
            .await?;
        Ok(())
    }
}
