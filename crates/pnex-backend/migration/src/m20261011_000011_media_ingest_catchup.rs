//! Catch-up for databases created before 0.1.0-beta.8: the media ingest
//! lots (P2.13, D159–D175) were added to `baseline/postgres.sql` in place,
//! so a database whose baseline was applied earlier never got them
//! (`ml_models.audio_meta`, `media_streams`… → 500 on /ml/models, /streams).
//! Idempotent: a no-op on a database created from the current baseline.
//! `down()` is a no-op, since these objects belong to the baseline.

use sea_orm_migration::prelude::*;

const UP: &str = r#"
ALTER TABLE ml_models ADD COLUMN IF NOT EXISTS audio_meta jsonb;
ALTER TABLE video_segments ALTER COLUMN device_registry_id DROP NOT NULL;
ALTER TABLE video_segments ADD COLUMN IF NOT EXISTS stream_id uuid;

-- ===== Media ingest (P2.13, D159-D164) =====

CREATE TABLE IF NOT EXISTS asr_profiles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    asr_model_id uuid NOT NULL,
    vad_model_id uuid,
    diarization_model_id uuid,
    diarization_embedding_model_id uuid,
    language character varying(16) DEFAULT 'fr'::character varying NOT NULL,
    beam integer DEFAULT 1 NOT NULL,
    word_timestamps boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT asr_profiles_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-asr_profiles-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-asr_profiles-asr_model_id" FOREIGN KEY (asr_model_id)
        REFERENCES ml_models(id) ON DELETE CASCADE,
    CONSTRAINT "fk-asr_profiles-vad_model_id" FOREIGN KEY (vad_model_id)
        REFERENCES ml_models(id) ON DELETE SET NULL,
    CONSTRAINT "fk-asr_profiles-diarization_model_id" FOREIGN KEY (diarization_model_id)
        REFERENCES ml_models(id) ON DELETE SET NULL,
    CONSTRAINT "fk-asr_profiles-diarization_embedding_model_id" FOREIGN KEY (diarization_embedding_model_id)
        REFERENCES ml_models(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_asr_profiles_org_name ON asr_profiles USING btree (org_id, name);

CREATE TABLE IF NOT EXISTS media_streams (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    slug character varying(48) NOT NULL,
    kind character varying(16) NOT NULL,
    url character varying(2048) NOT NULL,
    secret_id uuid,
    enabled boolean DEFAULT false NOT NULL,
    capture_on character varying(64) DEFAULT 'server'::character varying NOT NULL,
    asr_profile_id uuid,
    tracks character varying(16) DEFAULT 'audio'::character varying NOT NULL,
    fps integer DEFAULT 1 NOT NULL,
    segment_secs integer DEFAULT 30 NOT NULL,
    overlap_secs integer DEFAULT 1 NOT NULL,
    audio_retention character varying(16) DEFAULT 'none'::character varying NOT NULL,
    notify_channel_id uuid,
    timezone character varying(64) DEFAULT 'Europe/Paris'::character varying NOT NULL,
    tdm_checked_at timestamp with time zone,
    tdm_note text,
    capture_state character varying(16) DEFAULT 'stopped'::character varying NOT NULL,
    capture_error character varying(64),
    capture_changed_at timestamp with time zone,
    deleted_at timestamp with time zone,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT media_streams_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-media_streams-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-media_streams-asr_profile_id" FOREIGN KEY (asr_profile_id)
        REFERENCES asr_profiles(id) ON DELETE SET NULL,
    CONSTRAINT "fk-media_streams-notify_channel_id" FOREIGN KEY (notify_channel_id)
        REFERENCES notify_channels(id) ON DELETE SET NULL,
    CONSTRAINT "fk-media_streams-created_by" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_media_streams_org_slug ON media_streams USING btree (org_id, slug);

CREATE TABLE IF NOT EXISTS media_segments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    stream_id uuid NOT NULL,
    seq bigint NOT NULL,
    started_at timestamp with time zone NOT NULL,
    ended_at timestamp with time zone NOT NULL,
    clock_source character varying(16) NOT NULL,
    storage_key character varying(512),
    size_bytes bigint NOT NULL,
    state character varying(24) NOT NULL,
    asr_model_id uuid,
    asr_model_version bigint,
    asr_ms bigint,
    error character varying(64),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT media_segments_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-media_segments-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-media_segments-stream_id" FOREIGN KEY (stream_id)
        REFERENCES media_streams(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_media_segments_org_id_stream_id_started_at ON media_segments USING btree (org_id, stream_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_media_segments_state_updated_at ON media_segments USING btree (state, updated_at);

CREATE TABLE IF NOT EXISTS ml_model_checks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    model_id uuid NOT NULL,
    carrier character varying(64) NOT NULL,
    check_status character varying(16) NOT NULL,
    check_error text,
    load_ms bigint,
    rtf double precision,
    wer double precision,
    checked_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT ml_model_checks_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-ml_model_checks-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-ml_model_checks-model_id" FOREIGN KEY (model_id)
        REFERENCES ml_models(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_ml_model_checks_model_carrier ON ml_model_checks USING btree (model_id, carrier);

-- ===== Topic taxonomies (P2.13, D168) =====

CREATE TABLE IF NOT EXISTS taxonomies (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    description text,
    current_version integer DEFAULT 0 NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT taxonomies_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-taxonomies-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-taxonomies-created_by" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_taxonomies_org_name ON taxonomies USING btree (org_id, name);

CREATE TABLE IF NOT EXISTS taxonomy_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    taxonomy_id uuid NOT NULL,
    version integer NOT NULL,
    topics jsonb NOT NULL,
    note text,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT taxonomy_versions_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-taxonomy_versions-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-taxonomy_versions-taxonomy_id" FOREIGN KEY (taxonomy_id)
        REFERENCES taxonomies(id) ON DELETE CASCADE,
    CONSTRAINT "fk-taxonomy_versions-created_by" FOREIGN KEY (created_by)
        REFERENCES users(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_taxonomy_versions_taxonomy_version ON taxonomy_versions USING btree (taxonomy_id, version);

-- ===== Time ranges (P2.13, D169, D182) =====

CREATE TABLE IF NOT EXISTS time_ranges (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    scope_kind character varying(32) NOT NULL,
    scope_id character varying(64) NOT NULL,
    label character varying(200) NOT NULL,
    external_id character varying(200),
    category character varying(64),
    planned_start timestamp with time zone,
    planned_end timestamp with time zone,
    actual_start timestamp with time zone,
    actual_end timestamp with time zone,
    origin character varying(16) NOT NULL,
    confidence real,
    source_url character varying(2048),
    source_ref character varying(200),
    attrs jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT time_ranges_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-time_ranges-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT time_ranges_complete_pair CHECK (
        (planned_start IS NOT NULL AND planned_end IS NOT NULL)
        OR (actual_start IS NOT NULL AND actual_end IS NOT NULL)
    )
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_time_ranges_org_scope_external_id ON time_ranges USING btree (org_id, scope_kind, scope_id, external_id) WHERE external_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_time_ranges_org_scope_planned_start ON time_ranges USING btree (org_id, scope_kind, scope_id, planned_start);

-- D175: a recording comes from a device or from an IP stream, never both.
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk-video_segments-stream_id') THEN
        ALTER TABLE ONLY video_segments
            ADD CONSTRAINT "fk-video_segments-stream_id" FOREIGN KEY (stream_id) REFERENCES media_streams(id) ON DELETE CASCADE;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_video_segments_one_source') THEN
        ALTER TABLE ONLY video_segments
            ADD CONSTRAINT chk_video_segments_one_source CHECK (num_nonnulls(device_registry_id, stream_id) = 1);
    END IF;
END $$;
CREATE INDEX IF NOT EXISTS idx_video_segments_org_stream_started ON video_segments USING btree (org_id, stream_id, started_at);
"#;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, _m: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
