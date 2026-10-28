//! Schema invariants — regression guards on structural choices.
//! Requires PostgreSQL (compose.yaml): DATABASE_URL to a test DB.

use pnex_migration::Migrator;
use sea_orm_migration::{sea_orm, MigratorTrait};

async fn nullable_of(db: &sea_orm::DatabaseConnection, table: &str, col: &str) -> String {
    let row = sea_orm::ConnectionTrait::query_one_raw(
        db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "select is_nullable from information_schema.columns \
                 where table_name = '{table}' and column_name = '{col}'"
            ),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    row.try_get("", "is_nullable").unwrap()
}

/// ON DELETE action of the FK carried by `table.col`, via PG
/// (`confdeltype`: c=cascade, n=set null, a=no action).
async fn fk_del_type(db: &sea_orm::DatabaseConnection, table: &str, col: &str) -> String {
    let row = sea_orm::ConnectionTrait::query_one_raw(
        db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!(
                "select c.confdeltype::text from pg_constraint c \
                 join pg_class t on t.oid = c.conrelid \
                 join pg_attribute a on a.attrelid = t.oid and a.attname = '{col}' \
                 where c.contype = 'f' and t.relname = '{table}' \
                 and c.conkey = array[a.attnum]"
            ),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    row.try_get("", "confdeltype").unwrap()
}

#[tokio::test]
async fn scoping_org_et_catalogue_global_sans_copies() {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string());
    let db = sea_orm::Database::connect(url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    // Mandatory org scoping (D2): an org's data belongs to it.
    for t in [
        "device_registries",
        // D35: sites hierarchy removed — the POI is the direct tenant.
        "map_pins",
        "build_records",
        "fluid_mixtures",
        "flows",
        // D38: a device's last GPS position follows the org.
        "device_positions",
        // D21: a media asset and its versions belong to the org
        // (org_id denormalized on versions for direct scoping).
        "media_assets",
        "media_versions",
        // Edge references (2026-09-15): WiFi credentials + PNeX server hosts
        // belong to the org.
        "wifi_credentials",
        "pnex_hosts",
        // "Functions" registry (2026-09-15): versioned user functions;
        // org_id denormalized on versions (media school).
        "functions",
        "function_versions",
        // P2.13: media streams, their segments and ASR profiles are org data.
        "media_streams",
        "media_segments",
        "asr_profiles",
        "ml_model_checks",
        // D168: topic taxonomies and their append-only versions are org data.
        "taxonomies",
        "taxonomy_versions",
        // D169: time ranges are org data.
        "time_ranges",
    ] {
        assert_eq!(
            nullable_of(&db, t, "org_id").await,
            "NO",
            "{t}.org_id must be NOT NULL"
        );
    }

    // Shared ETL catalogue without copies: org_id NULL = row provided by
    // the app (directive "no per-org/per-user copies").
    for t in ["unit_conversions", "formulas"] {
        assert_eq!(
            nullable_of(&db, t, "org_id").await,
            "YES",
            "{t}.org_id must be nullable"
        );
        // loco nullable reference (`("organizations?", "org_id")`):
        // nullable column + ON DELETE SET NULL ('n' in pg_constraint).
        assert_eq!(
            fk_del_type(&db, t, "org_id").await,
            "n",
            "{t}.org_id must be SET NULL on delete"
        );
    }

    // Mandatory references: CASCADE ('c').
    for (t, col) in [
        ("device_registries", "org_id"),
        ("build_records", "org_id"),
        // Phase 5: live state (D9) and OpenObserve mapping (D2) follow
        // the removal of the device / of the org.
        ("device_states", "device_registry_id"),
        ("openobserve_orgs", "org_id"),
        // D18: a flow and its versions follow the org; the append-only
        // history follows the flow.
        ("flows", "org_id"),
        ("flow_versions", "flow_id"),
        // D21: media versions follow the org and the asset.
        ("media_versions", "org_id"),
        ("media_versions", "asset_id"),
        // Versioned functions: versions follow the function and the org.
        ("functions", "org_id"),
        ("function_versions", "function_id"),
        ("function_versions", "org_id"),
        // P2.13: segments follow their stream; streams and profiles the org.
        ("media_streams", "org_id"),
        ("media_segments", "org_id"),
        ("media_segments", "stream_id"),
        ("asr_profiles", "org_id"),
        // D167: a per-carrier check follows its model.
        ("ml_model_checks", "org_id"),
        ("ml_model_checks", "model_id"),
        // D168: versions follow their taxonomy; both follow the org.
        ("taxonomies", "org_id"),
        ("taxonomy_versions", "org_id"),
        ("taxonomy_versions", "taxonomy_id"),
        // D169: time ranges follow the org.
        ("time_ranges", "org_id"),
    ] {
        assert_eq!(
            fk_del_type(&db, t, col).await,
            "c",
            "{t}.{col} must be CASCADE on delete"
        );
    }

    // D18 — flows: nullable references as SET NULL ('n').
    for (t, col) in [
        // Product attachment to the device: the flow survives the removal
        // of the device, simply unlinked.
        ("flows", "device_registry_id"),
        // Circular FK flows → flow_versions: a version is never
        // deleted (append-only), but SET NULL if it were.
        ("flows", "deployed_version_id"),
        // Circular FK functions → function_versions (current version).
        ("functions", "current_version_id"),
    ] {
        assert_eq!(
            nullable_of(&db, t, col).await,
            "YES",
            "{t}.{col} must be nullable"
        );
        assert_eq!(
            fk_del_type(&db, t, col).await,
            "n",
            "{t}.{col} must be SET NULL on delete"
        );
    }

    // D18 — versionnement : (flow_id, version_number) unique.
    let idx = sea_orm::ConnectionTrait::query_one_raw(
        &db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "select count(*)::int as n from pg_indexes \
             where indexname = 'uniq_flow_versions_flow_number' \
             and tablename = 'flow_versions'",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let n_idx: i32 = idx.try_get("", "n").unwrap();
    assert_eq!(
        n_idx, 1,
        "the unique index uniq_flow_versions_flow_number must exist"
    );

    // D21 — media: circular FK current_version_id as SET NULL ('n'),
    // versioning (asset_id, version_number) unique.
    assert_eq!(
        nullable_of(&db, "media_assets", "current_version_id").await,
        "YES",
        "media_assets.current_version_id must be nullable"
    );
    assert_eq!(
        fk_del_type(&db, "media_assets", "current_version_id").await,
        "n",
        "media_assets.current_version_id must be SET NULL on delete"
    );
    let idx = sea_orm::ConnectionTrait::query_one_raw(
        &db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "select count(*)::int as n from pg_indexes \
             where indexname = 'uniq_media_versions_asset_number' \
             and tablename = 'media_versions'",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let n_idx: i32 = idx.try_get("", "n").unwrap();
    assert_eq!(
        n_idx, 1,
        "the unique index uniq_media_versions_asset_number must exist"
    );

    // Versioned functions (2026-09-15): (function_id, version_number)
    // unique + nullable circular FK current_version_id.
    let idx = sea_orm::ConnectionTrait::query_one_raw(
        &db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "select count(*)::int as n from pg_indexes \
             where indexname = 'uniq_function_versions_function_number' \
             and tablename = 'function_versions'",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let n_idx: i32 = idx.try_get("", "n").unwrap();
    assert_eq!(
        n_idx, 1,
        "the unique index uniq_function_versions_function_number must exist"
    );

    // Liveness lease (D9): 1:1 with the registry, last_seen always set.
    assert_eq!(
        nullable_of(&db, "device_states", "last_seen_at").await,
        "NO",
        "device_states.last_seen_at must be NOT NULL"
    );

    // The subscription carried by the org (D11): SET NULL when the tier disappears.
    assert_eq!(
        fk_del_type(&db, "organizations", "subscription_tier_id").await,
        "n",
        "organizations.subscription_tier_id must be SET NULL on delete"
    );

    // Fluid catalogue removed (directive): the external FastAPI service
    // (CoolProp/RefProp) is the source of truth; the database only keeps
    // custom mixtures per org. Stale tables must not come back.
    let row = sea_orm::ConnectionTrait::query_one_raw(
        &db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "select count(*)::int as n from information_schema.tables \
             where table_name in ('formula_imports', 'conversion_imports', \
             'fluid_catalogs', 'fluid_property_groups')",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let n: i32 = row.try_get("", "n").unwrap();
    assert_eq!(n, 0, "copy tables and fluid catalogue must stay removed");
}

/// Baseline round trip: `down()` drops every table and enum type (the
/// flows <-> flow_versions FK cycle included), `up()` runs again cleanly.
/// Self-contained: the scratch database is created and dropped here.
#[tokio::test]
async fn baseline_down_drops_everything_and_up_replays() {
    let base = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string());
    let admin_url = base
        .rsplit_once('/')
        .map_or_else(|| base.clone(), |(prefix, _)| format!("{prefix}/postgres"));
    let scratch = format!("pnex_baseline_down_test_{}", std::process::id());

    let admin = sea_orm::Database::connect(&admin_url).await.unwrap();
    let _ = sea_orm::ConnectionTrait::execute_unprepared(
        &admin,
        &format!("DROP DATABASE IF EXISTS {scratch}"),
    )
    .await;
    sea_orm::ConnectionTrait::execute_unprepared(&admin, &format!("CREATE DATABASE {scratch}"))
        .await
        .unwrap();

    let url = base
        .rsplit_once('/')
        .map_or_else(|| base.clone(), |(prefix, _)| format!("{prefix}/{scratch}"));
    let db = sea_orm::Database::connect(&url).await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    Migrator::down(&db, None).await.unwrap();

    let count = |sql: &'static str| {
        let db = db.clone();
        async move {
            let row = sea_orm::ConnectionTrait::query_one_raw(
                &db,
                sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql),
            )
            .await
            .unwrap()
            .unwrap();
            row.try_get::<i32>("", "n").unwrap()
        }
    };
    assert_eq!(
        count(
            "select count(*)::int as n from information_schema.tables \
             where table_schema = 'public' and table_name <> 'seaql_migrations'"
        )
        .await,
        0,
        "every table is dropped by down()"
    );
    assert_eq!(
        count(
            "select count(*)::int as n from pg_type t join pg_namespace n \
             on n.oid = t.typnamespace where n.nspname = 'public' and t.typtype = 'e'"
        )
        .await,
        0,
        "every enum type is dropped by down()"
    );
    Migrator::up(&db, None).await.unwrap();
    drop(db);

    let _ = sea_orm::ConnectionTrait::execute_unprepared(
        &admin,
        &format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"),
    )
    .await;
}
