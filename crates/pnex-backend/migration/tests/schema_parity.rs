//! PostgreSQL / SQLite schema parity: the same tables, the same columns
//! and the same foreign keys on both backends once every migration ran.
//! Guards future migrations against the drift found when the baseline was
//! built (tables dropped on PG only, columns and FKs added on PG only).
//! Requires PostgreSQL (`TEST_DATABASE_URL`, compose.yaml).

use std::collections::{BTreeMap, BTreeSet};

use pnex_migration::Migrator;
use sea_orm_migration::sea_orm::{ConnectionTrait, Database, DatabaseConnection, Statement};
use sea_orm_migration::MigratorTrait;

type Schema = (
    BTreeMap<String, BTreeSet<String>>,
    BTreeSet<(String, String, String)>,
);

async fn rows(db: &DatabaseConnection, sql: &str) -> Vec<sea_orm_migration::sea_orm::QueryResult> {
    db.query_all_raw(Statement::from_string(db.get_database_backend(), sql))
        .await
        .unwrap()
}

async fn pg_schema(db: &DatabaseConnection) -> Schema {
    let mut cols: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows(
        db,
        "select c.table_name::text as t, c.column_name::text as c \
         from information_schema.columns c join information_schema.tables t \
         on t.table_name = c.table_name and t.table_schema = c.table_schema \
         where c.table_schema = 'public' and t.table_type = 'BASE TABLE' \
         and c.table_name <> 'seaql_migrations'",
    )
    .await
    {
        cols.entry(r.try_get("", "t").unwrap())
            .or_default()
            .insert(r.try_get("", "c").unwrap());
    }
    let mut fks = BTreeSet::new();
    for r in rows(
        db,
        "select tc.relname::text as t, a.attname::text as c, rc.relname::text as r \
         from pg_constraint k \
         join pg_class tc on tc.oid = k.conrelid \
         join pg_class rc on rc.oid = k.confrelid \
         join pg_attribute a on a.attrelid = k.conrelid and a.attnum = k.conkey[1] \
         where k.contype = 'f'",
    )
    .await
    {
        fks.insert((
            r.try_get("", "t").unwrap(),
            r.try_get("", "c").unwrap(),
            r.try_get("", "r").unwrap(),
        ));
    }
    (cols, fks)
}

async fn sqlite_schema(db: &DatabaseConnection) -> Schema {
    let mut cols: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut fks = BTreeSet::new();
    let tables: Vec<String> = rows(
        db,
        "select name from sqlite_master where type = 'table' \
         and name not like 'sqlite_%' and name <> 'seaql_migrations'",
    )
    .await
    .into_iter()
    .map(|r| r.try_get("", "name").unwrap())
    .collect();
    for t in tables {
        for r in rows(db, &format!("pragma table_info(\"{t}\")")).await {
            cols.entry(t.clone())
                .or_default()
                .insert(r.try_get("", "name").unwrap());
        }
        for r in rows(db, &format!("pragma foreign_key_list(\"{t}\")")).await {
            fks.insert((
                t.clone(),
                r.try_get("", "from").unwrap(),
                r.try_get("", "table").unwrap(),
            ));
        }
    }
    (cols, fks)
}

#[tokio::test]
async fn postgres_and_sqlite_schemas_match() {
    let base = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string());
    let admin_url = base
        .rsplit_once('/')
        .map_or_else(|| base.clone(), |(prefix, _)| format!("{prefix}/postgres"));
    let scratch = format!("pnex_schema_parity_test_{}", std::process::id());
    let admin = Database::connect(&admin_url).await.unwrap();
    let _ = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {scratch}"))
        .await;
    admin
        .execute_unprepared(&format!("CREATE DATABASE {scratch}"))
        .await
        .unwrap();
    let pg_url = base
        .rsplit_once('/')
        .map_or_else(|| base.clone(), |(prefix, _)| format!("{prefix}/{scratch}"));
    let pg = Database::connect(&pg_url).await.unwrap();
    Migrator::up(&pg, None).await.unwrap();
    let pg_side = pg_schema(&pg).await;
    drop(pg);
    let _ = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"))
        .await;

    let sqlite = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&sqlite, None).await.unwrap();
    let sqlite_side = sqlite_schema(&sqlite).await;

    assert_eq!(
        pg_side.0.keys().collect::<Vec<_>>(),
        sqlite_side.0.keys().collect::<Vec<_>>(),
        "same tables on both backends"
    );
    for (table, columns) in &pg_side.0 {
        assert_eq!(
            Some(columns),
            sqlite_side.0.get(table),
            "same columns for `{table}`"
        );
    }
    assert_eq!(
        pg_side.1, sqlite_side.1,
        "same foreign keys on both backends"
    );
}
