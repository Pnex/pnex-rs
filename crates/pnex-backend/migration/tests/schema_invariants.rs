//! Invariants de schéma — garde-fous de régression sur les choix structurants.
//! Nécessite PostgreSQL (compose.yaml) : DATABASE_URL vers une DB de test.

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

/// Action ON DELETE de la FK portée par `table.col`, via PG
/// (`confdeltype` : c=cascade, n=set null, a=no action).
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

    // Scoping org obligatoire (D2) : les données d'une org lui appartiennent.
    for t in [
        "device_registries",
        // D35 : hiérarchie sites supprimée — le POI est le tenant direct.
        "map_pins",
        "build_records",
        "fluid_mixtures",
        "flows",
        // D38 : la dernière position GPS d'un device suit l'org.
        "device_positions",
        // D21 : un asset média et ses versions appartiennent à l'org
        // (org_id dénormalisé sur les versions pour le scoping direct).
        "media_assets",
        "media_versions",
        // Référentiels edge (2026-09-15) : credentials WiFi + hosts serveur
        // PNeX appartiennent à l'org.
        "wifi_credentials",
        "pnex_hosts",
        // Registre « Fonctions » (2026-09-15) : fonctions utilisateur
        // versionnées ; org_id dénormalisé sur les versions (école media).
        "functions",
        "function_versions",
        // P2.13: media streams, their segments and ASR profiles are org data.
        "media_streams",
        "media_segments",
        "asr_profiles",
    ] {
        assert_eq!(
            nullable_of(&db, t, "org_id").await,
            "NO",
            "{t}.org_id doit être NOT NULL"
        );
    }

    // Catalogue ETL partagé sans copies : org_id NULL = ligne fournie par
    // l'app (directive « pas de copies par org/utilisateur »).
    for t in ["unit_conversions", "formulas"] {
        assert_eq!(
            nullable_of(&db, t, "org_id").await,
            "YES",
            "{t}.org_id doit être nullable"
        );
        // Référence nullable loco (`("organizations?", "org_id")`) :
        // colonne nullable + ON DELETE SET NULL ('n' dans pg_constraint).
        assert_eq!(
            fk_del_type(&db, t, "org_id").await,
            "n",
            "{t}.org_id doit être SET NULL on delete"
        );
    }

    // Références obligatoires : CASCADE ('c').
    for (t, col) in [
        ("device_registries", "org_id"),
        ("build_records", "org_id"),
        // Phase 5 : état live (D9) et correspondance OpenObserve (D2) suivent
        // la disparition du device / de l'org.
        ("device_states", "device_registry_id"),
        ("openobserve_orgs", "org_id"),
        // D18 : un flow et ses versions suivent la org ; l'historique
        // append-only suit le flow.
        ("flows", "org_id"),
        ("flow_versions", "flow_id"),
        // D21 : versions média suivent l'org et l'asset.
        ("media_versions", "org_id"),
        ("media_versions", "asset_id"),
        // Fonctions versionnées : versions suivent la fonction et l'org.
        ("functions", "org_id"),
        ("function_versions", "function_id"),
        ("function_versions", "org_id"),
        // P2.13: segments follow their stream; streams and profiles the org.
        ("media_streams", "org_id"),
        ("media_segments", "org_id"),
        ("media_segments", "stream_id"),
        ("asr_profiles", "org_id"),
    ] {
        assert_eq!(
            fk_del_type(&db, t, col).await,
            "c",
            "{t}.{col} doit être CASCADE on delete"
        );
    }

    // D18 — flows : références nullable en SET NULL ('n').
    for (t, col) in [
        // Attachement produit au device : le flow survit à la disparition
        // du device, simplement dé-lié.
        ("flows", "device_registry_id"),
        // FK circulaire flows → flow_versions : une version n'est jamais
        // supprimée (append-only), mais SET NULL si tel était le cas.
        ("flows", "deployed_version_id"),
        // FK circulaire functions → function_versions (version courante).
        ("functions", "current_version_id"),
    ] {
        assert_eq!(
            nullable_of(&db, t, col).await,
            "YES",
            "{t}.{col} doit être nullable"
        );
        assert_eq!(
            fk_del_type(&db, t, col).await,
            "n",
            "{t}.{col} doit être SET NULL on delete"
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
        "l'index unique uniq_flow_versions_flow_number doit exister"
    );

    // D21 — média : FK circulaire current_version_id en SET NULL ('n'),
    // versioning (asset_id, version_number) unique.
    assert_eq!(
        nullable_of(&db, "media_assets", "current_version_id").await,
        "YES",
        "media_assets.current_version_id doit être nullable"
    );
    assert_eq!(
        fk_del_type(&db, "media_assets", "current_version_id").await,
        "n",
        "media_assets.current_version_id doit être SET NULL on delete"
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
        "l'index unique uniq_media_versions_asset_number doit exister"
    );

    // Fonctions versionnées (2026-09-15) : (function_id, version_number)
    // unique + FK circulaire current_version_id nullable.
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
        "l'index unique uniq_function_versions_function_number doit exister"
    );

    // Bail de vie (D9) : 1:1 avec le registre, last_seen toujours renseigné.
    assert_eq!(
        nullable_of(&db, "device_states", "last_seen_at").await,
        "NO",
        "device_states.last_seen_at doit être NOT NULL"
    );

    // L'abonnement porté par l'org (D11) : SET NULL quand le tier disparaît.
    assert_eq!(
        fk_del_type(&db, "organizations", "subscription_tier_id").await,
        "n",
        "organizations.subscription_tier_id doit être SET NULL on delete"
    );

    // Catalogue de fluides supprimé (directive) : le service externe FastAPI
    // (CoolProp/RefProp) est la source de vérité ; la base ne garde que les
    // mélanges custom par org. Tables périmées interdites de retour.
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
    assert_eq!(
        n, 0,
        "tables de copie et catalogue fluides doivent rester supprimées"
    );
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
