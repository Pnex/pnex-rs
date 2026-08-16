//! Invariants de schéma — garde-fous de régression sur les choix structurants.
//! Nécessite PostgreSQL (compose.yaml) : DATABASE_URL vers une DB de test.

use pnex_migration::Migrator;
use sea_orm_migration::{MigratorTrait, sea_orm};

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
        "sites",
        "svg_files",
        "build_records",
        "fluid_mixtures",
    ] {
        assert_eq!(nullable_of(&db, t, "org_id").await, "NO", "{t}.org_id doit être NOT NULL");
    }

    // Catalogue ETL partagé sans copies : org_id NULL = ligne fournie par
    // l'app (directive « pas de copies par org/utilisateur »).
    for t in ["unit_conversions", "formulas"] {
        assert_eq!(nullable_of(&db, t, "org_id").await, "YES", "{t}.org_id doit être nullable");
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
    ] {
        assert_eq!(
            fk_del_type(&db, t, col).await,
            "c",
            "{t}.{col} doit être CASCADE on delete"
        );
    }

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
