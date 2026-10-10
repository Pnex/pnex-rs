//! D191: a 0.1 database migrates to the 0.2 ontology without loss —
//! every existing row gets an identity, edges resolve to it, and the
//! triggers keep identities in step on later writes.

use pnex_migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};

async fn scratch_db() -> (DatabaseConnection, String, String) {
    let base = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string());
    let (prefix, _) = base.rsplit_once('/').unwrap();
    let admin_url = format!("{prefix}/postgres");
    let scratch = format!("pnex_onto_migration_test_{}", std::process::id());
    let admin = sea_orm::Database::connect(&admin_url).await.unwrap();
    let _ = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {scratch}"))
        .await;
    admin
        .execute_unprepared(&format!("CREATE DATABASE {scratch}"))
        .await
        .unwrap();
    let db = sea_orm::Database::connect(format!("{prefix}/{scratch}"))
        .await
        .unwrap();
    (db, admin_url, scratch)
}

async fn text(db: &DatabaseConnection, sql: &str) -> Option<String> {
    db.query_one_raw(Statement::from_string(db.get_database_backend(), sql))
        .await
        .unwrap()
        .and_then(|r| r.try_get::<Option<String>>("", "v").unwrap())
}

#[tokio::test]
async fn v01_database_migrates_to_ontology() {
    let (db, admin_url, scratch) = scratch_db().await;
    // 0.1 schema with data.
    Migrator::up(&db, Some(1)).await.unwrap();
    db.execute_unprepared(
        "INSERT INTO organizations (id, name) VALUES (1, 'Org');
         INSERT INTO map_pins (id, org_id, mode, label, x, y)
             VALUES ('00000000-0000-0000-0000-0000000000a1', 1, 'plan', 'Site A', 1, 1);
         INSERT INTO media_assets (id, org_id, kind, name)
             VALUES ('00000000-0000-0000-0000-0000000000b1', 1, 'pano', 'Hall');
         INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id)
             VALUES (1, 'placed_on', 'map_pin', '00000000-0000-0000-0000-0000000000a1',
                     'media_asset', '00000000-0000-0000-0000-0000000000b1');",
    )
    .await
    .unwrap();

    Migrator::up(&db, None).await.unwrap();

    // Backfill: identities and edge ends.
    assert_eq!(
        text(
            &db,
            "SELECT o.title AS v FROM objects o JOIN map_pins p ON p.object_id = o.id"
        )
        .await,
        Some("Site A".into())
    );
    assert_eq!(
        text(
            &db,
            "SELECT (source_object_id IS NOT NULL AND target_object_id IS NOT NULL)::text AS v
             FROM resource_edges"
        )
        .await,
        Some("true".into())
    );

    // Triggers: insert, rename, delete.
    db.execute_unprepared("INSERT INTO resource_folders (org_id, name) VALUES (1, 'Pumps')")
        .await
        .unwrap();
    assert_eq!(
        text(
            &db,
            "SELECT o.type_key AS v FROM objects o JOIN resource_folders f ON f.object_id = o.id"
        )
        .await,
        Some("folder".into())
    );
    db.execute_unprepared("UPDATE map_pins SET label = 'Site B'")
        .await
        .unwrap();
    assert_eq!(
        text(
            &db,
            "SELECT o.title AS v FROM objects o JOIN map_pins p ON p.object_id = o.id"
        )
        .await,
        Some("Site B".into())
    );
    db.execute_unprepared("DELETE FROM media_assets")
        .await
        .unwrap();
    assert_eq!(
        text(
            &db,
            "SELECT (valid_to IS NOT NULL)::text AS v FROM objects WHERE type_key = 'media_asset'"
        )
        .await,
        Some("true".into()),
        "a deleted row closes its identity, never deletes it"
    );

    // Temporal links: a closed link does not block a new open one.
    db.execute_unprepared(
        "UPDATE resource_edges SET valid_to = now();
         INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id)
             VALUES (1, 'placed_on', 'map_pin', '00000000-0000-0000-0000-0000000000a1',
                     'media_asset', '00000000-0000-0000-0000-0000000000b1');",
    )
    .await
    .unwrap();

    // Round trip.
    Migrator::down(&db, Some(1)).await.unwrap();
    Migrator::up(&db, None).await.unwrap();

    drop(db);
    let admin = sea_orm::Database::connect(&admin_url).await.unwrap();
    let _ = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {scratch} WITH (FORCE)"))
        .await;
}
