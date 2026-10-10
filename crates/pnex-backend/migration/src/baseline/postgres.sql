-- PNeX baseline schema, PostgreSQL (0.1.0, cut 2026-10-09).
-- Generated from the pre-release migrations, then cleaned: legacy
-- plaintext/AI-connector leftovers removed, compatibility columns dropped
-- (device metadata, host ws_ssl, build success flag). Every later change
-- goes in a new migration (docs/architecture/migrations.md).

-- ===== Enum types =====

CREATE TYPE capability_mode AS ENUM (
    'input',
    'output',
    'input_output'
);

CREATE TYPE constant_kind AS ENUM (
    'number',
    'string',
    'boolean'
);

CREATE TYPE conversion_kind AS ENUM (
    'linear',
    'affine',
    'custom'
);

CREATE TYPE data_source_kind AS ENUM (
    'device',
    'constant'
);

CREATE TYPE formula_kind AS ENUM (
    'simple_math',
    'fluid_property',
    'power_calculation',
    'rate_of_change'
);

CREATE TYPE openobserve_org_status AS ENUM (
    'pending',
    'provisioned',
    'failed'
);

CREATE TYPE org_member_role AS ENUM (
    'owner',
    'admin',
    'member',
    'viewer'
);

CREATE TYPE ui_theme AS ENUM (
    'light',
    'dark',
    'auto'
);

-- ===== Tables =====

CREATE TABLE agent_enrollments (
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    code_hash character varying(64) NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    hostname character varying(255),
    os character varying(32),
    arch character varying(32),
    agent_version character varying(32),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE agent_keys (
    id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    key character varying(255) NOT NULL,
    unit character varying(64),
    kind character varying(16) DEFAULT 'number'::character varying NOT NULL,
    record_o2 boolean DEFAULT false NOT NULL,
    first_seen_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    last_seen_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE annotation_layer_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    layer_id uuid NOT NULL,
    version_number bigint NOT NULL,
    doc jsonb NOT NULL,
    author character varying(255),
    note text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE annotation_layers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    published_version_id uuid,
    media_asset_id uuid,
    tour_id uuid
);

CREATE TABLE build_records (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    device_id character varying(255),
    build_phase character varying(255) DEFAULT 'queued' NOT NULL,
    firmware_bin_s3_key character varying(255),
    org_id bigint NOT NULL,
    fw_version text,
    ota_sha256 text,
    ota_size_bytes bigint,
    sources_fingerprint text,
    firmware_revision_id bigint,
    failure_code text,
    failure_detail text
);

CREATE TABLE dashboard_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    dashboard_id uuid NOT NULL,
    version_number bigint NOT NULL,
    layout jsonb NOT NULL,
    author character varying(255),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE dashboards (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    current_version_number bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE device_cameras (
    device_registry_id bigint NOT NULL,
    org_id bigint NOT NULL,
    framesize character varying(16) DEFAULT 'vga'::character varying NOT NULL,
    quality smallint DEFAULT 12 NOT NULL,
    fps smallint DEFAULT 5 NOT NULL,
    capture_mode character varying(16) DEFAULT 'on_demand'::character varying NOT NULL,
    vflip boolean DEFAULT false NOT NULL,
    hmirror boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE device_capabilities (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    mode capability_mode DEFAULT 'input'::capability_mode NOT NULL
);

CREATE TABLE device_capability_instances (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    gpio integer NOT NULL,
    label character varying(64) NOT NULL,
    mode character varying(32) NOT NULL,
    config jsonb,
    constraints_snapshot jsonb,
    enabled boolean DEFAULT true NOT NULL
);

CREATE TABLE device_placements (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    device_id character varying(255) NOT NULL,
    pin_id uuid NOT NULL,
    location_detail character varying(255)
);

CREATE TABLE device_positions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    device_id character varying(255) NOT NULL,
    latitude numeric(9,6) NOT NULL,
    longitude numeric(9,6) NOT NULL,
    altitude_m numeric(8,2),
    accuracy_m numeric(8,2),
    speed_mps numeric(6,2),
    heading_deg numeric(5,1),
    source character varying(16) DEFAULT 'telemetry'::character varying NOT NULL,
    positioned_at timestamp with time zone NOT NULL
);

CREATE TABLE device_registries (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    device_id character varying(255) NOT NULL,
    active boolean DEFAULT false NOT NULL,
    allow_dynamic_measurements boolean DEFAULT true NOT NULL,
    discovered_measurements jsonb,
    max_unique_measurements integer DEFAULT 100 NOT NULL,
    org_id bigint NOT NULL,
    predefined_device_id bigint NOT NULL,
    soc text,
    board_id bigint,
    peripherals jsonb,
    fw_version text,
    ota_ready boolean,
    announced_caps jsonb,
    firmware_project_id bigint
);

CREATE TABLE device_states (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    last_seen_at timestamp with time zone NOT NULL
);

CREATE TABLE device_tokens (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    token character varying NOT NULL,
    encryption_key character varying(64) NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    device_registry_id bigint NOT NULL
);

CREATE TABLE device_types (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(100) NOT NULL
);

CREATE TABLE firmware_artifacts (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    key character varying(255) NOT NULL,
    bytes bytea NOT NULL,
    size_bytes bigint NOT NULL,
    sha256 character varying(64)
);

CREATE TABLE firmware_checks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    firmware_project_id bigint NOT NULL,
    revision_number bigint NOT NULL,
    status character varying(16) DEFAULT 'queued'::character varying NOT NULL,
    diagnostics jsonb,
    log_tail text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE firmware_projects (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    description text,
    chip_family character varying(16) NOT NULL,
    current_revision_id bigint
);

CREATE TABLE firmware_revisions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    revision_number bigint NOT NULL,
    main_cpp text NOT NULL,
    lib_deps jsonb NOT NULL,
    content_hash character varying(64) NOT NULL,
    note text,
    firmware_project_id bigint NOT NULL,
    org_id bigint NOT NULL
);

CREATE TABLE flow_leases (
    name character varying(64) NOT NULL,
    holder character varying(128) NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

CREATE TABLE flow_placements (
    org_id bigint NOT NULL,
    worker_id character varying(128) NOT NULL,
    epoch bigint DEFAULT 1 NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone NOT NULL
);

CREATE TABLE flow_versions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    version_number bigint NOT NULL,
    graph jsonb NOT NULL,
    author character varying(255),
    note text,
    flow_id bigint NOT NULL
);

CREATE TABLE flow_workers (
    id character varying(128) NOT NULL,
    boot bigint NOT NULL,
    advertise_url character varying(512) DEFAULT ''::character varying NOT NULL,
    capacity integer DEFAULT 0 NOT NULL,
    draining boolean DEFAULT false NOT NULL,
    started_at timestamp with time zone NOT NULL,
    heartbeat_at timestamp with time zone NOT NULL
);

CREATE TABLE flows (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(200) NOT NULL,
    status character varying(32) DEFAULT 'draft'::character varying NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint,
    deployed_version_id bigint
);

CREATE TABLE fluid_mixtures (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(100) NOT NULL,
    description text,
    composition jsonb NOT NULL,
    org_id bigint NOT NULL
);

CREATE TABLE formula_data_sources (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    source_type data_source_kind NOT NULL,
    measurement_name character varying(255),
    constant_type constant_kind,
    constant_value text,
    variable_name character varying(100) NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    formula_id bigint NOT NULL,
    device_registry_id bigint,
    unit_conversion_id bigint
);

CREATE TABLE formulas (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    formula_type formula_kind NOT NULL,
    expression text NOT NULL,
    result_unit text,
    fluid_config jsonb,
    is_predefined boolean DEFAULT false NOT NULL,
    global_id uuid,
    version integer DEFAULT 1 NOT NULL,
    category character varying(50),
    tags jsonb,
    import_count integer DEFAULT 0 NOT NULL,
    compute_on_event boolean DEFAULT false NOT NULL,
    cache_ttl integer DEFAULT 60 NOT NULL,
    last_computed_at timestamp with time zone,
    org_id bigint
);

CREATE TABLE function_versions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    version_number bigint NOT NULL,
    code text NOT NULL,
    inputs jsonb NOT NULL,
    outputs jsonb NOT NULL,
    note text,
    function_id bigint NOT NULL,
    org_id bigint NOT NULL
);

CREATE TABLE functions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    language character varying(16) DEFAULT 'js'::character varying NOT NULL,
    current_version_id bigint,
    description text
);

CREATE TABLE llm_providers (
    id uuid NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    kind character varying(32) NOT NULL,
    base_url character varying(1024),
    model character varying(200) NOT NULL,
    secret_id uuid,
    is_default boolean DEFAULT false NOT NULL,
    created_by bigint,
    updated_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE map_pins (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    mode character varying(16) NOT NULL,
    latitude numeric(9,6),
    longitude numeric(9,6),
    x numeric(10,2),
    y numeric(10,2),
    label character varying(255) NOT NULL,
    emoji character varying(16),
    metadata jsonb,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    location_detail character varying(255),
    preview_kind character varying(255),
    preview_id character varying(255),
    CONSTRAINT chk_map_pins_mode_coords CHECK (((((mode)::text = 'geo'::text) AND (latitude IS NOT NULL) AND (longitude IS NOT NULL) AND (x IS NULL) AND (y IS NULL)) OR (((mode)::text = 'plan'::text) AND (x IS NOT NULL) AND (y IS NOT NULL) AND (latitude IS NULL) AND (longitude IS NULL))))
);

CREATE TABLE mcu_boards (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    soc character varying(255) DEFAULT 'esp32'::character varying NOT NULL,
    details jsonb,
    pretty_name text,
    pio_board text
);

CREATE TABLE media_assets (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    kind character varying(32) NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    metadata jsonb,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    current_version_id uuid
);

CREATE TABLE media_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    asset_id uuid NOT NULL,
    org_id bigint NOT NULL,
    version_number bigint NOT NULL,
    filename character varying(255) NOT NULL,
    content_type character varying(100) NOT NULL,
    size_bytes bigint NOT NULL,
    storage_key text NOT NULL,
    sha256 character varying(64),
    metadata jsonb,
    note text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE ml_models (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    description text,
    task character varying(32) DEFAULT 'detection'::character varying NOT NULL,
    family character varying(32) DEFAULT 'yolox'::character varying NOT NULL,
    asset_id uuid NOT NULL,
    asset_version bigint,
    input_width integer DEFAULT 416 NOT NULL,
    input_height integer DEFAULT 416 NOT NULL,
    labels jsonb DEFAULT '[]'::jsonb NOT NULL,
    score_threshold real DEFAULT 0.35 NOT NULL,
    nms_iou real DEFAULT 0.45 NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    check_status character varying(16) DEFAULT 'unchecked'::character varying NOT NULL,
    check_error text,
    infer_ms bigint,
    checked_at timestamp with time zone,
    audio_meta jsonb
);

CREATE TABLE notify_channels (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    kind character varying(32) NOT NULL,
    name character varying(200) NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE notify_templates (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    subject text,
    body text NOT NULL,
    vars jsonb DEFAULT '[]'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE openobserve_orgs (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    o2_org character varying(255) NOT NULL,
    ingestion_token text,
    status openobserve_org_status DEFAULT 'pending'::openobserve_org_status NOT NULL,
    last_error text
);

CREATE TABLE org_secrets (
    id uuid NOT NULL,
    org_id bigint,
    name character varying(255) NOT NULL,
    description text,
    ciphertext bytea NOT NULL,
    nonce bytea NOT NULL,
    key_id character varying(32) NOT NULL,
    created_by bigint,
    updated_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE organization_members (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    role org_member_role DEFAULT 'viewer'::org_member_role NOT NULL,
    user_id bigint NOT NULL,
    org_id bigint NOT NULL
);

CREATE TABLE organizations (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    subscription_tier_id bigint,
    data_retention_days integer,
    ai_retention_days integer
);

CREATE TABLE ota_assignments (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    target_version character varying(64) NOT NULL,
    artifact_key character varying(255) NOT NULL,
    sha256 character varying(64) NOT NULL,
    size_bytes bigint,
    state character varying(16) NOT NULL,
    progress integer,
    error text,
    cmd_id character varying(64)
);

CREATE TABLE pnex_hosts (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    host character varying(255) NOT NULL
);

CREATE TABLE predefined_device_capabilities (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    predefined_device_id bigint NOT NULL,
    device_capability_id bigint NOT NULL
);

CREATE TABLE predefined_devices (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying NOT NULL,
    pretty_name character varying(255),
    revision character varying(50) DEFAULT ''::character varying NOT NULL,
    device_doc_url character varying(1024),
    prestashop_product_id character varying(64),
    prestashop_buy_url character varying(1024),
    byod_doc_url character varying(1024),
    image_source_url character varying(1024),
    stl_files_url character varying(1024),
    description text,
    device_type_id bigint NOT NULL,
    board_id bigint NOT NULL,
    peripherals jsonb,
    description_i18n text
);

CREATE TABLE regulator_configs (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    node_id character varying(64) NOT NULL,
    kind character varying(16) NOT NULL,
    config jsonb NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    flow_id bigint NOT NULL
);

CREATE TABLE resource_containments (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    child_kind character varying(32) NOT NULL,
    child_id character varying(64) NOT NULL,
    parent_kind character varying(32) NOT NULL,
    parent_id character varying(64) NOT NULL,
    sort_key character varying(64)
);

CREATE TABLE resource_edges (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    relation character varying(64) NOT NULL,
    source_kind character varying(32) NOT NULL,
    source_id character varying(64) NOT NULL,
    target_kind character varying(32) NOT NULL,
    target_id character varying(64) NOT NULL,
    placement jsonb
);

CREATE TABLE resource_folders (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    emoji character varying(16),
    created_by bigint
);

CREATE TABLE resource_labels (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    resource_kind character varying(32) NOT NULL,
    resource_id character varying(64) NOT NULL,
    labels jsonb,
    updated_by bigint
);

CREATE TABLE secret_usages (
    id bigint NOT NULL,
    secret_id uuid NOT NULL,
    consumer_kind character varying(32) NOT NULL,
    consumer_id character varying(64) NOT NULL,
    field character varying(128) NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE stitch_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    asset_id uuid NOT NULL,
    state character varying(16) DEFAULT 'queued'::character varying NOT NULL,
    error text,
    frames_total integer NOT NULL,
    frames_received integer DEFAULT 0 NOT NULL,
    poses_json jsonb NOT NULL,
    hfov_deg real NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE subscription_tiers (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(100) NOT NULL,
    max_sensor_devices integer NOT NULL,
    max_actuator_devices integer NOT NULL,
    max_mixed_devices integer NOT NULL,
    min_build_interval_secs bigint DEFAULT 900 NOT NULL,
    data_retention_secs bigint,
    max_telemetry_mb bigint
);

CREATE TABLE system_settings (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    key character varying(100) NOT NULL,
    value text NOT NULL,
    updated_by bigint
);

CREATE TABLE tour_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    tour_id uuid NOT NULL,
    version_number bigint NOT NULL,
    doc jsonb NOT NULL,
    author character varying(255),
    note text,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE tours (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    mode character varying(16) DEFAULT 'panorama'::character varying NOT NULL,
    share_token character varying(64),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    published_version_id uuid
);

CREATE TABLE unit_conversions (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    name character varying(255) NOT NULL,
    from_unit character varying(50) NOT NULL,
    to_unit character varying(50) NOT NULL,
    conversion_type conversion_kind NOT NULL,
    multiplier double precision DEFAULT 1 NOT NULL,
    "offset" double precision DEFAULT 0 NOT NULL,
    expression text,
    description text,
    is_predefined boolean DEFAULT false NOT NULL,
    global_id uuid,
    version integer DEFAULT 1 NOT NULL,
    category character varying(50),
    tags jsonb,
    import_count integer DEFAULT 0 NOT NULL,
    org_id bigint
);

CREATE TABLE user_profiles (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    language character varying(10) DEFAULT 'en'::character varying NOT NULL,
    timezone character varying(50) DEFAULT 'UTC'::character varying NOT NULL,
    date_format character varying(20),
    theme ui_theme DEFAULT 'auto'::ui_theme NOT NULL,
    preferences jsonb,
    grafana_url character varying(500),
    user_id bigint NOT NULL
);

CREATE TABLE users (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    idp_sub character varying(64),
    email character varying(255) NOT NULL,
    full_name character varying(255),
    platform_admin boolean DEFAULT false NOT NULL
);

CREATE TABLE video_segments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    flow_id bigint,
    node_id character varying(64) NOT NULL,
    stream character varying(128) NOT NULL,
    started_at timestamp with time zone NOT NULL,
    ended_at timestamp with time zone NOT NULL,
    frame_count integer NOT NULL,
    size_bytes bigint NOT NULL,
    width integer NOT NULL,
    height integer NOT NULL,
    storage_key character varying(512) NOT NULL,
    expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE viz_widget_library (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(255) NOT NULL,
    kind character varying(16) NOT NULL,
    config jsonb NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);

CREATE TABLE wifi_credentials (
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    id bigint NOT NULL,
    org_id bigint NOT NULL,
    ssid character varying(64) NOT NULL,
    secret_id uuid
);

-- ===== Identity columns =====

ALTER TABLE agent_enrollments ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME agent_enrollments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE agent_keys ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME agent_keys_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE build_records ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME build_records_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_capabilities ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_capabilities_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_capability_instances ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_capability_instances_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_placements ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_placements_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_positions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_positions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_registries ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_registries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_states ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_states_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_tokens ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_tokens_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE device_types ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME device_types_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE firmware_artifacts ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME firmware_artifacts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE firmware_projects ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME firmware_projects_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE firmware_revisions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME firmware_revisions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE flow_versions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME flow_versions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE flows ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME flows_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE fluid_mixtures ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME fluid_mixtures_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE formula_data_sources ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME formula_data_sources_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE formulas ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME formulas_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE function_versions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME function_versions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE functions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME functions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE mcu_boards ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME mcu_boards_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE openobserve_orgs ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME openobserve_orgs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE organization_members ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME organization_members_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE organizations ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME organizations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE ota_assignments ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME ota_assignments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE pnex_hosts ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME pnex_hosts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE predefined_devices ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME predefined_devices_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE regulator_configs ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME regulator_configs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE resource_containments ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME resource_containments_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE resource_edges ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME resource_edges_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE resource_folders ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME resource_folders_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE resource_labels ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME resource_labels_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE secret_usages ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME secret_usages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE subscription_tiers ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME subscription_tiers_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE system_settings ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME system_settings_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE unit_conversions ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME unit_conversions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE user_profiles ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME user_profiles_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE users ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

ALTER TABLE wifi_credentials ALTER COLUMN id ADD GENERATED BY DEFAULT AS IDENTITY (
    SEQUENCE NAME wifi_credentials_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

-- ===== Primary keys and unique constraints =====

ALTER TABLE ONLY agent_enrollments
    ADD CONSTRAINT agent_enrollments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY agent_keys
    ADD CONSTRAINT agent_keys_pkey PRIMARY KEY (id);

ALTER TABLE ONLY annotation_layer_versions
    ADD CONSTRAINT annotation_layer_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY annotation_layers
    ADD CONSTRAINT annotation_layers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY build_records
    ADD CONSTRAINT build_records_pkey PRIMARY KEY (id);

ALTER TABLE ONLY dashboard_versions
    ADD CONSTRAINT dashboard_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY dashboards
    ADD CONSTRAINT dashboards_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_cameras
    ADD CONSTRAINT device_cameras_pkey PRIMARY KEY (device_registry_id);

ALTER TABLE ONLY device_capabilities
    ADD CONSTRAINT device_capabilities_name_key UNIQUE (name);

ALTER TABLE ONLY device_capabilities
    ADD CONSTRAINT device_capabilities_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_capability_instances
    ADD CONSTRAINT device_capability_instances_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_placements
    ADD CONSTRAINT device_placements_device_registry_id_key UNIQUE (device_registry_id);

ALTER TABLE ONLY device_placements
    ADD CONSTRAINT device_placements_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_positions
    ADD CONSTRAINT device_positions_device_registry_id_key UNIQUE (device_registry_id);

ALTER TABLE ONLY device_positions
    ADD CONSTRAINT device_positions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_registries
    ADD CONSTRAINT device_registries_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_states
    ADD CONSTRAINT device_states_device_registry_id_key UNIQUE (device_registry_id);

ALTER TABLE ONLY device_states
    ADD CONSTRAINT device_states_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_tokens
    ADD CONSTRAINT device_tokens_pkey PRIMARY KEY (id);

ALTER TABLE ONLY device_tokens
    ADD CONSTRAINT device_tokens_token_key UNIQUE (token);

ALTER TABLE ONLY device_types
    ADD CONSTRAINT device_types_name_key UNIQUE (name);

ALTER TABLE ONLY device_types
    ADD CONSTRAINT device_types_pkey PRIMARY KEY (id);

ALTER TABLE ONLY firmware_artifacts
    ADD CONSTRAINT firmware_artifacts_key_key UNIQUE (key);

ALTER TABLE ONLY firmware_artifacts
    ADD CONSTRAINT firmware_artifacts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY firmware_checks
    ADD CONSTRAINT firmware_checks_pkey PRIMARY KEY (id);

ALTER TABLE ONLY firmware_projects
    ADD CONSTRAINT firmware_projects_pkey PRIMARY KEY (id);

ALTER TABLE ONLY firmware_revisions
    ADD CONSTRAINT firmware_revisions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY flow_leases
    ADD CONSTRAINT flow_leases_pkey PRIMARY KEY (name);

ALTER TABLE ONLY flow_placements
    ADD CONSTRAINT flow_placements_pkey PRIMARY KEY (org_id);

ALTER TABLE ONLY flow_versions
    ADD CONSTRAINT flow_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY flow_workers
    ADD CONSTRAINT flow_workers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY flows
    ADD CONSTRAINT flows_pkey PRIMARY KEY (id);

ALTER TABLE ONLY fluid_mixtures
    ADD CONSTRAINT fluid_mixtures_pkey PRIMARY KEY (id);

ALTER TABLE ONLY formula_data_sources
    ADD CONSTRAINT formula_data_sources_pkey PRIMARY KEY (id);

ALTER TABLE ONLY formulas
    ADD CONSTRAINT formulas_pkey PRIMARY KEY (id);

ALTER TABLE ONLY function_versions
    ADD CONSTRAINT function_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY functions
    ADD CONSTRAINT functions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY predefined_device_capabilities
    ADD CONSTRAINT "idx-predefined_device_capabilities-refs-pk" PRIMARY KEY (predefined_device_id, device_capability_id);

ALTER TABLE ONLY llm_providers
    ADD CONSTRAINT llm_providers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY map_pins
    ADD CONSTRAINT map_pins_pkey PRIMARY KEY (id);

ALTER TABLE ONLY mcu_boards
    ADD CONSTRAINT mcu_boards_pkey PRIMARY KEY (id);

ALTER TABLE ONLY media_assets
    ADD CONSTRAINT media_assets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY media_versions
    ADD CONSTRAINT media_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ml_models
    ADD CONSTRAINT ml_models_pkey PRIMARY KEY (id);

ALTER TABLE ONLY notify_channels
    ADD CONSTRAINT notify_channels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY notify_templates
    ADD CONSTRAINT notify_templates_pkey PRIMARY KEY (id);

ALTER TABLE ONLY openobserve_orgs
    ADD CONSTRAINT openobserve_orgs_o2_org_key UNIQUE (o2_org);

ALTER TABLE ONLY openobserve_orgs
    ADD CONSTRAINT openobserve_orgs_org_id_key UNIQUE (org_id);

ALTER TABLE ONLY openobserve_orgs
    ADD CONSTRAINT openobserve_orgs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY org_secrets
    ADD CONSTRAINT org_secrets_pkey PRIMARY KEY (id);

ALTER TABLE ONLY organization_members
    ADD CONSTRAINT organization_members_pkey PRIMARY KEY (id);

ALTER TABLE ONLY organizations
    ADD CONSTRAINT organizations_name_key UNIQUE (name);

ALTER TABLE ONLY organizations
    ADD CONSTRAINT organizations_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ota_assignments
    ADD CONSTRAINT ota_assignments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY pnex_hosts
    ADD CONSTRAINT pnex_hosts_pkey PRIMARY KEY (id);

ALTER TABLE ONLY predefined_devices
    ADD CONSTRAINT predefined_devices_name_key UNIQUE (name);

ALTER TABLE ONLY predefined_devices
    ADD CONSTRAINT predefined_devices_pkey PRIMARY KEY (id);

ALTER TABLE ONLY regulator_configs
    ADD CONSTRAINT regulator_configs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resource_containments
    ADD CONSTRAINT resource_containments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resource_edges
    ADD CONSTRAINT resource_edges_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resource_folders
    ADD CONSTRAINT resource_folders_pkey PRIMARY KEY (id);

ALTER TABLE ONLY resource_labels
    ADD CONSTRAINT resource_labels_pkey PRIMARY KEY (id);

ALTER TABLE ONLY secret_usages
    ADD CONSTRAINT secret_usages_pkey PRIMARY KEY (id);

ALTER TABLE ONLY stitch_jobs
    ADD CONSTRAINT stitch_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY subscription_tiers
    ADD CONSTRAINT subscription_tiers_name_key UNIQUE (name);

ALTER TABLE ONLY subscription_tiers
    ADD CONSTRAINT subscription_tiers_pkey PRIMARY KEY (id);

ALTER TABLE ONLY system_settings
    ADD CONSTRAINT system_settings_key_key UNIQUE (key);

ALTER TABLE ONLY system_settings
    ADD CONSTRAINT system_settings_pkey PRIMARY KEY (id);

ALTER TABLE ONLY tour_versions
    ADD CONSTRAINT tour_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY tours
    ADD CONSTRAINT tours_pkey PRIMARY KEY (id);

ALTER TABLE ONLY unit_conversions
    ADD CONSTRAINT unit_conversions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY user_profiles
    ADD CONSTRAINT user_profiles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY users
    ADD CONSTRAINT users_email_key UNIQUE (email);

ALTER TABLE ONLY users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

ALTER TABLE ONLY video_segments
    ADD CONSTRAINT video_segments_pkey PRIMARY KEY (id);

ALTER TABLE ONLY viz_widget_library
    ADD CONSTRAINT viz_widget_library_pkey PRIMARY KEY (id);

ALTER TABLE ONLY wifi_credentials
    ADD CONSTRAINT wifi_credentials_pkey PRIMARY KEY (id);

-- ===== Indexes =====

CREATE INDEX idx_agent_enrollments_device ON agent_enrollments USING btree (device_registry_id);

CREATE INDEX idx_annotation_layers_org ON annotation_layers USING btree (org_id);

CREATE INDEX idx_dashboards_org_name ON dashboards USING btree (org_id, name);

CREATE INDEX idx_device_placements_pin ON device_placements USING btree (org_id, pin_id);

CREATE INDEX idx_device_positions_org ON device_positions USING btree (org_id);

CREATE INDEX idx_firmware_checks_project ON firmware_checks USING btree (firmware_project_id);

CREATE INDEX idx_firmware_projects_org_name ON firmware_projects USING btree (org_id, name);

CREATE INDEX idx_flow_placements_worker_id ON flow_placements USING btree (worker_id);

CREATE INDEX idx_flows_status_org_id ON flows USING btree (status, org_id);

CREATE INDEX idx_functions_org_name ON functions USING btree (org_id, name);

CREATE INDEX idx_map_pins_org_geo ON map_pins USING btree (org_id, latitude, longitude);

CREATE INDEX idx_media_assets_org_kind ON media_assets USING btree (org_id, kind);

CREATE INDEX idx_media_assets_org_name ON media_assets USING btree (org_id, name);

CREATE INDEX idx_media_versions_org ON media_versions USING btree (org_id);

CREATE INDEX idx_org_secrets_key_id ON org_secrets USING btree (key_id);

CREATE INDEX idx_ota_assignment_device ON ota_assignments USING btree (device_registry_id, id DESC);

CREATE INDEX idx_resource_containment_parent ON resource_containments USING btree (org_id, parent_kind, parent_id);

CREATE INDEX idx_resource_edges_target ON resource_edges USING btree (org_id, target_kind, target_id);

CREATE INDEX idx_resource_labels_gin ON resource_labels USING gin (labels jsonb_path_ops);

CREATE INDEX idx_secret_usages_secret ON secret_usages USING btree (secret_id);

CREATE INDEX idx_stitch_jobs_org ON stitch_jobs USING btree (org_id);

CREATE INDEX idx_tours_org ON tours USING btree (org_id);

CREATE INDEX idx_video_segments_expires_at ON video_segments USING btree (expires_at);

CREATE INDEX idx_video_segments_org_device_started ON video_segments USING btree (org_id, device_registry_id, started_at);

CREATE UNIQUE INDEX uniq_agent_enrollments_code_hash ON agent_enrollments USING btree (code_hash);

CREATE UNIQUE INDEX uniq_agent_keys_device_key ON agent_keys USING btree (device_registry_id, key);

CREATE UNIQUE INDEX uniq_annotation_layer_versions_layer_number ON annotation_layer_versions USING btree (layer_id, version_number);

CREATE UNIQUE INDEX uniq_dashboard_versions_doc_number ON dashboard_versions USING btree (dashboard_id, version_number);

CREATE UNIQUE INDEX uniq_dci_device_gpio ON device_capability_instances USING btree (device_registry_id, gpio);

CREATE UNIQUE INDEX uniq_device_registries_org_device_id ON device_registries USING btree (org_id, device_id);

CREATE UNIQUE INDEX uniq_device_tokens_device_registry ON device_tokens USING btree (device_registry_id);

CREATE UNIQUE INDEX uniq_firmware_revisions_project_number ON firmware_revisions USING btree (firmware_project_id, revision_number);

CREATE UNIQUE INDEX uniq_flow_versions_flow_number ON flow_versions USING btree (flow_id, version_number);

CREATE UNIQUE INDEX uniq_fluid_mixtures_org_name ON fluid_mixtures USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_formula_data_sources_formula_var ON formula_data_sources USING btree (formula_id, variable_name);

CREATE UNIQUE INDEX uniq_formulas_global_id ON formulas USING btree (global_id) WHERE (global_id IS NOT NULL);

CREATE UNIQUE INDEX uniq_function_versions_function_number ON function_versions USING btree (function_id, version_number);

CREATE UNIQUE INDEX uniq_llm_providers_org_default ON llm_providers USING btree (org_id) WHERE is_default;

CREATE UNIQUE INDEX uniq_llm_providers_org_name ON llm_providers USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_media_versions_asset_number ON media_versions USING btree (asset_id, version_number);

CREATE UNIQUE INDEX uniq_ml_models_org_name ON ml_models USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_notify_channels_org_name ON notify_channels USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_notify_templates_org_name ON notify_templates USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_org_secrets_org_name ON org_secrets USING btree (org_id, name) WHERE (org_id IS NOT NULL);

CREATE UNIQUE INDEX uniq_org_secrets_platform_name ON org_secrets USING btree (name) WHERE (org_id IS NULL);

CREATE UNIQUE INDEX uniq_organization_members_org_user ON organization_members USING btree (org_id, user_id);

CREATE UNIQUE INDEX uniq_pnex_hosts_org_host ON pnex_hosts USING btree (org_id, host);

CREATE UNIQUE INDEX uniq_predefined_devices_prestashop_product_id ON predefined_devices USING btree (prestashop_product_id) WHERE (prestashop_product_id IS NOT NULL);

CREATE UNIQUE INDEX uniq_regulator_config_slot ON regulator_configs USING btree (device_registry_id, flow_id, node_id);

CREATE UNIQUE INDEX uniq_resource_containment_child ON resource_containments USING btree (org_id, child_kind, child_id);

CREATE UNIQUE INDEX uniq_resource_edges_pair ON resource_edges USING btree (org_id, relation, source_kind, source_id, target_kind, target_id);

CREATE UNIQUE INDEX uniq_resource_labels_resource ON resource_labels USING btree (org_id, resource_kind, resource_id);

CREATE UNIQUE INDEX uniq_secret_usages_consumer_field ON secret_usages USING btree (consumer_kind, consumer_id, field);

CREATE UNIQUE INDEX uniq_tour_versions_tour_number ON tour_versions USING btree (tour_id, version_number);

CREATE UNIQUE INDEX uniq_tours_share_token ON tours USING btree (share_token);

CREATE UNIQUE INDEX uniq_unit_conversions_org_units ON unit_conversions USING btree (org_id, from_unit, to_unit) WHERE (org_id IS NOT NULL);

CREATE UNIQUE INDEX uniq_unit_conversions_predefined_units ON unit_conversions USING btree (from_unit, to_unit) WHERE is_predefined;

CREATE UNIQUE INDEX uniq_user_profiles_user ON user_profiles USING btree (user_id);

CREATE UNIQUE INDEX uniq_users_idp_sub ON users USING btree (idp_sub) WHERE (idp_sub IS NOT NULL);

CREATE UNIQUE INDEX uniq_viz_widget_library_org_name ON viz_widget_library USING btree (org_id, name);

CREATE UNIQUE INDEX uniq_wifi_credentials_org_ssid ON wifi_credentials USING btree (org_id, ssid);

CREATE UNIQUE INDEX uq_ota_assignments_one_active ON ota_assignments USING btree (device_registry_id) WHERE ((state)::text = ANY ((ARRAY['pending'::character varying, 'downloading'::character varying, 'flashing'::character varying])::text[]));

-- ===== Foreign keys =====

ALTER TABLE ONLY annotation_layers
    ADD CONSTRAINT annotation_layers_media_asset_id_fkey FOREIGN KEY (media_asset_id) REFERENCES media_assets(id) ON DELETE SET NULL;

ALTER TABLE ONLY annotation_layers
    ADD CONSTRAINT annotation_layers_tour_id_fkey FOREIGN KEY (tour_id) REFERENCES tours(id) ON DELETE SET NULL;

ALTER TABLE ONLY agent_enrollments
    ADD CONSTRAINT "fk-agent_enrollments-device_registry_id" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE CASCADE;

ALTER TABLE ONLY agent_enrollments
    ADD CONSTRAINT "fk-agent_enrollments-org_id" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY agent_keys
    ADD CONSTRAINT "fk-agent_keys-device_registry_id" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE CASCADE;

ALTER TABLE ONLY annotation_layer_versions
    ADD CONSTRAINT "fk-annotation_layer_versions-layer_id-to-annotation_layers" FOREIGN KEY (layer_id) REFERENCES annotation_layers(id) ON DELETE CASCADE;

ALTER TABLE ONLY annotation_layers
    ADD CONSTRAINT "fk-annotation_layers-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY annotation_layers
    ADD CONSTRAINT "fk-annotation_layers-published_version_id-to-annotation_layer_v" FOREIGN KEY (published_version_id) REFERENCES annotation_layer_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY build_records
    ADD CONSTRAINT "fk-build_records-firmware_revision_id" FOREIGN KEY (firmware_revision_id) REFERENCES firmware_revisions(id) ON DELETE SET NULL;

ALTER TABLE ONLY build_records
    ADD CONSTRAINT "fk-build_records-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY dashboard_versions
    ADD CONSTRAINT "fk-dashboard_versions-dashboard_id-to-dashboards" FOREIGN KEY (dashboard_id) REFERENCES dashboards(id) ON DELETE CASCADE;

ALTER TABLE ONLY dashboards
    ADD CONSTRAINT "fk-dashboards-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY device_cameras
    ADD CONSTRAINT "fk-device_cameras-device_registry_id" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE CASCADE;

ALTER TABLE ONLY device_cameras
    ADD CONSTRAINT "fk-device_cameras-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY device_capability_instances
    ADD CONSTRAINT "fk-device_capability_instances-device_registry_id-to-device_reg" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_placements
    ADD CONSTRAINT "fk-device_placements-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_placements
    ADD CONSTRAINT "fk-device_placements-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_placements
    ADD CONSTRAINT "fk-device_placements-pin_id-to-map_pins" FOREIGN KEY (pin_id) REFERENCES map_pins(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_positions
    ADD CONSTRAINT "fk-device_positions-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_positions
    ADD CONSTRAINT "fk-device_positions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_registries
    ADD CONSTRAINT "fk-device_registries-firmware_project_id" FOREIGN KEY (firmware_project_id) REFERENCES firmware_projects(id) ON DELETE SET NULL;

ALTER TABLE ONLY device_registries
    ADD CONSTRAINT "fk-device_registries-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_registries
    ADD CONSTRAINT "fk-device_registries-predefined_device_id-to-predefined_devices" FOREIGN KEY (predefined_device_id) REFERENCES predefined_devices(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_states
    ADD CONSTRAINT "fk-device_states-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_tokens
    ADD CONSTRAINT "fk-device_tokens-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY firmware_checks
    ADD CONSTRAINT "fk-firmware_checks-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY firmware_checks
    ADD CONSTRAINT "fk-firmware_checks-project_id-to-firmware_projects" FOREIGN KEY (firmware_project_id) REFERENCES firmware_projects(id) ON DELETE CASCADE;

ALTER TABLE ONLY firmware_projects
    ADD CONSTRAINT "fk-firmware_projects-current_revision_id" FOREIGN KEY (current_revision_id) REFERENCES firmware_revisions(id) ON DELETE SET NULL;

ALTER TABLE ONLY firmware_projects
    ADD CONSTRAINT "fk-firmware_projects-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY firmware_revisions
    ADD CONSTRAINT "fk-firmware_revisions-firmware_project_id-to-firmware_projects" FOREIGN KEY (firmware_project_id) REFERENCES firmware_projects(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY firmware_revisions
    ADD CONSTRAINT "fk-firmware_revisions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY flow_placements
    ADD CONSTRAINT "fk-flow_placements-org_id" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY flow_versions
    ADD CONSTRAINT "fk-flow_versions-flow_id-to-flows" FOREIGN KEY (flow_id) REFERENCES flows(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY flows
    ADD CONSTRAINT "fk-flows-deployed_version_id-to-flow_versions" FOREIGN KEY (deployed_version_id) REFERENCES flow_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY flows
    ADD CONSTRAINT "fk-flows-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE SET NULL;

ALTER TABLE ONLY flows
    ADD CONSTRAINT "fk-flows-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY fluid_mixtures
    ADD CONSTRAINT "fk-fluid_mixtures-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY formula_data_sources
    ADD CONSTRAINT "fk-formula_data_sources-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE SET NULL;

ALTER TABLE ONLY formula_data_sources
    ADD CONSTRAINT "fk-formula_data_sources-formula_id-to-formulas" FOREIGN KEY (formula_id) REFERENCES formulas(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY formula_data_sources
    ADD CONSTRAINT "fk-formula_data_sources-unit_conversion_id-to-unit_conversions" FOREIGN KEY (unit_conversion_id) REFERENCES unit_conversions(id) ON DELETE SET NULL;

ALTER TABLE ONLY formulas
    ADD CONSTRAINT "fk-formulas-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY function_versions
    ADD CONSTRAINT "fk-function_versions-function_id-to-functions" FOREIGN KEY (function_id) REFERENCES functions(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY function_versions
    ADD CONSTRAINT "fk-function_versions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY functions
    ADD CONSTRAINT "fk-functions-current_version_id-to-function_versions" FOREIGN KEY (current_version_id) REFERENCES function_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY functions
    ADD CONSTRAINT "fk-functions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY llm_providers
    ADD CONSTRAINT "fk-llm_providers-created_by" FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE ONLY llm_providers
    ADD CONSTRAINT "fk-llm_providers-org_id" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY llm_providers
    ADD CONSTRAINT "fk-llm_providers-updated_by" FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE ONLY map_pins
    ADD CONSTRAINT "fk-map_pins-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY media_assets
    ADD CONSTRAINT "fk-media_assets-current_version_id-to-media_versions" FOREIGN KEY (current_version_id) REFERENCES media_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY media_assets
    ADD CONSTRAINT "fk-media_assets-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY media_versions
    ADD CONSTRAINT "fk-media_versions-asset_id-to-media_assets" FOREIGN KEY (asset_id) REFERENCES media_assets(id) ON DELETE CASCADE;

ALTER TABLE ONLY media_versions
    ADD CONSTRAINT "fk-media_versions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY ml_models
    ADD CONSTRAINT "fk-ml_models-asset_id-to-media_assets" FOREIGN KEY (asset_id) REFERENCES media_assets(id) ON DELETE CASCADE;

ALTER TABLE ONLY ml_models
    ADD CONSTRAINT "fk-ml_models-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY notify_channels
    ADD CONSTRAINT "fk-notify_channels-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY notify_templates
    ADD CONSTRAINT "fk-notify_templates-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY openobserve_orgs
    ADD CONSTRAINT "fk-openobserve_orgs-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY org_secrets
    ADD CONSTRAINT "fk-org_secrets-created_by" FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE ONLY org_secrets
    ADD CONSTRAINT "fk-org_secrets-org_id" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY org_secrets
    ADD CONSTRAINT "fk-org_secrets-updated_by" FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL;

ALTER TABLE ONLY organization_members
    ADD CONSTRAINT "fk-organization_members-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY organization_members
    ADD CONSTRAINT "fk-organization_members-user_id-to-users" FOREIGN KEY (user_id) REFERENCES users(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY organizations
    ADD CONSTRAINT "fk-organizations-subscription_tier_id-to-subscription_tiers" FOREIGN KEY (subscription_tier_id) REFERENCES subscription_tiers(id) ON DELETE SET NULL;

ALTER TABLE ONLY ota_assignments
    ADD CONSTRAINT "fk-ota_assignments-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY ota_assignments
    ADD CONSTRAINT "fk-ota_assignments-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY pnex_hosts
    ADD CONSTRAINT "fk-pnex_hosts-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY predefined_device_capabilities
    ADD CONSTRAINT "fk-predefined_device_capabilities-device_capability_id-to-devic" FOREIGN KEY (device_capability_id) REFERENCES device_capabilities(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY predefined_device_capabilities
    ADD CONSTRAINT "fk-predefined_device_capabilities-predefined_device_id-to-prede" FOREIGN KEY (predefined_device_id) REFERENCES predefined_devices(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY predefined_devices
    ADD CONSTRAINT "fk-predefined_devices-board_id-to-mcu_boards" FOREIGN KEY (board_id) REFERENCES mcu_boards(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY predefined_devices
    ADD CONSTRAINT "fk-predefined_devices-device_type_id-to-device_types" FOREIGN KEY (device_type_id) REFERENCES device_types(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY regulator_configs
    ADD CONSTRAINT "fk-regulator_configs-device_registry_id-to-device_registries" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY regulator_configs
    ADD CONSTRAINT "fk-regulator_configs-flow_id-to-flows" FOREIGN KEY (flow_id) REFERENCES flows(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY regulator_configs
    ADD CONSTRAINT "fk-regulator_configs-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY resource_containments
    ADD CONSTRAINT "fk-resource_containments-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY resource_edges
    ADD CONSTRAINT "fk-resource_edges-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY resource_folders
    ADD CONSTRAINT "fk-resource_folders-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY resource_labels
    ADD CONSTRAINT "fk-resource_labels-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY secret_usages
    ADD CONSTRAINT "fk-secret_usages-secret_id" FOREIGN KEY (secret_id) REFERENCES org_secrets(id) ON DELETE CASCADE;

ALTER TABLE ONLY stitch_jobs
    ADD CONSTRAINT "fk-stitch_jobs-asset_id-to-media_assets" FOREIGN KEY (asset_id) REFERENCES media_assets(id) ON DELETE CASCADE;

ALTER TABLE ONLY stitch_jobs
    ADD CONSTRAINT "fk-stitch_jobs-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY tour_versions
    ADD CONSTRAINT "fk-tour_versions-tour_id-to-tours" FOREIGN KEY (tour_id) REFERENCES tours(id) ON DELETE CASCADE;

ALTER TABLE ONLY tours
    ADD CONSTRAINT "fk-tours-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY tours
    ADD CONSTRAINT "fk-tours-published_version_id-to-tour_versions" FOREIGN KEY (published_version_id) REFERENCES tour_versions(id) ON DELETE SET NULL;

ALTER TABLE ONLY unit_conversions
    ADD CONSTRAINT "fk-unit_conversions-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE SET NULL;

ALTER TABLE ONLY user_profiles
    ADD CONSTRAINT "fk-user_profiles-user_id-to-users" FOREIGN KEY (user_id) REFERENCES users(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY video_segments
    ADD CONSTRAINT "fk-video_segments-device_registry_id" FOREIGN KEY (device_registry_id) REFERENCES device_registries(id) ON DELETE CASCADE;

ALTER TABLE ONLY video_segments
    ADD CONSTRAINT "fk-video_segments-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY viz_widget_library
    ADD CONSTRAINT "fk-viz_widget_library-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON DELETE CASCADE;

ALTER TABLE ONLY wifi_credentials
    ADD CONSTRAINT "fk-wifi_credentials-org_id-to-organizations" FOREIGN KEY (org_id) REFERENCES organizations(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY device_registries
    ADD CONSTRAINT fk_device_registries_board_id FOREIGN KEY (board_id) REFERENCES mcu_boards(id) ON DELETE SET NULL;

-- ===== Controls, assistant conversations, device PKI =====

CREATE TABLE controls (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    key character varying(64) NOT NULL,
    label character varying(255) NOT NULL,
    kind character varying(16) NOT NULL,
    spec jsonb NOT NULL,
    created_by bigint,
    origin character varying(255),
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT controls_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-controls-org_id-to-organizations" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_controls_org_key ON controls USING btree (org_id, key);
CREATE UNIQUE INDEX uniq_controls_org_origin ON controls USING btree (org_id, origin);

CREATE TABLE ai_conversations (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    user_id bigint NOT NULL,
    title character varying(200) NOT NULL,
    busy_until timestamp with time zone,
    last_message_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT ai_conversations_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-ai_conversations-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-ai_conversations-user_id" FOREIGN KEY (user_id)
        REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_ai_conversations_user_org_last ON ai_conversations USING btree (user_id, org_id, last_message_at DESC);
CREATE INDEX idx_ai_conversations_last_message_at ON ai_conversations USING btree (last_message_at);

CREATE TABLE ai_messages (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    conversation_id uuid NOT NULL,
    seq integer NOT NULL,
    role character varying(16) NOT NULL,
    content text NOT NULL,
    tool_trace jsonb,
    page character varying(255),
    tokens_in integer,
    tokens_out integer,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT ai_messages_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-ai_messages-conversation_id" FOREIGN KEY (conversation_id)
        REFERENCES ai_conversations(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_ai_messages_conversation_seq ON ai_messages USING btree (conversation_id, seq);

CREATE TABLE org_device_cas (
    org_id bigint NOT NULL,
    cert_pem text NOT NULL,
    key_secret_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT org_device_cas_pkey PRIMARY KEY (org_id),
    CONSTRAINT "fk-org_device_cas-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE
);

CREATE TABLE device_certificates (
    id bigint GENERATED BY DEFAULT AS IDENTITY NOT NULL,
    org_id bigint NOT NULL,
    device_registry_id bigint NOT NULL,
    serial character varying(64) NOT NULL,
    fingerprint_sha256 character varying(64) NOT NULL,
    not_after timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    CONSTRAINT device_certificates_pkey PRIMARY KEY (id),
    CONSTRAINT "fk-device_certificates-org_id" FOREIGN KEY (org_id)
        REFERENCES organizations(id) ON DELETE CASCADE,
    CONSTRAINT "fk-device_certificates-device_registry_id" FOREIGN KEY (device_registry_id)
        REFERENCES device_registries(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX uniq_device_certificates_fingerprint ON device_certificates USING btree (fingerprint_sha256);
CREATE INDEX idx_device_certificates_device ON device_certificates USING btree (device_registry_id);

-- ===== Media ingest (P2.13, D159-D164) =====

CREATE TABLE asr_profiles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    org_id bigint NOT NULL,
    name character varying(200) NOT NULL,
    asr_model_id uuid NOT NULL,
    vad_model_id uuid,
    diarization_model_id uuid,
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
        REFERENCES ml_models(id) ON DELETE SET NULL
);
CREATE UNIQUE INDEX uniq_asr_profiles_org_name ON asr_profiles USING btree (org_id, name);

CREATE TABLE media_streams (
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
CREATE UNIQUE INDEX uniq_media_streams_org_slug ON media_streams USING btree (org_id, slug);

CREATE TABLE media_segments (
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
CREATE INDEX idx_media_segments_org_id_stream_id_started_at ON media_segments USING btree (org_id, stream_id, started_at DESC);
CREATE INDEX idx_media_segments_state_updated_at ON media_segments USING btree (state, updated_at);

CREATE TABLE ml_model_checks (
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
CREATE UNIQUE INDEX uniq_ml_model_checks_model_carrier ON ml_model_checks USING btree (model_id, carrier);
