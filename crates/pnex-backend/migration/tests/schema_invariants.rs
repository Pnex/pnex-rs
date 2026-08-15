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
    ] {
        assert_eq!(nullable_of(&db, t, "org_id").await, "NO", "{t}.org_id doit être NOT NULL");
    }

    // Catalogue ETL partagé sans copies : org_id NULL = ligne fournie par
    // l'app (directive « pas de copies par org/utilisateur »).
    for t in ["unit_conversions", "formulas", "fluid_catalogs"] {
        assert_eq!(nullable_of(&db, t, "org_id").await, "YES", "{t}.org_id doit être nullable");
    }

    // Les tables de copie Django (formula/conversion_imports) n'existent plus.
    let row = sea_orm::ConnectionTrait::query_one_raw(
        &db,
        sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            "select count(*)::int as n from information_schema.tables \
             where table_name in ('formula_imports', 'conversion_imports')",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let n: i32 = row.try_get("", "n").unwrap();
    assert_eq!(n, 0, "les tables d'import par copie doivent rester supprimées");
}
