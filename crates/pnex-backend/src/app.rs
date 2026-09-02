use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Hooks},
    bgworker::{BackgroundWorker, Queue},
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
    Result,
};

use pnex_migration::Migrator;
use std::path::Path;

use crate::controllers;

pub struct App;

#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        "pnex-server"
    }

    fn app_version() -> String {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("BUILD_SHA")
                .or(option_env!("GITHUB_SHA"))
                .unwrap_or("dev")
        )
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        config: Config,
    ) -> Result<BootResult> {
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::health::routes())
            .add_route(controllers::oauth2::routes())
            .add_route(controllers::user_info::routes())
            .add_route(controllers::orgs::routes())
            .add_route(controllers::devices::routes())
            .add_route(controllers::devices::catalogue_routes())
            .add_route(controllers::builds::routes())
            .add_route(controllers::dashboard::routes())
            .add_route(controllers::visualization::routes())
            .add_route(controllers::ws_ingest::routes())
            .add_route(controllers::ws_device::routes())
            .add_route(controllers::pins::routes())
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        // Batcher télémétrie → OpenObserve (no-op si non configuré : le sink
        // noop reste en place — tests, déploiements sans télémétrie).
        crate::services::openobserve::spawn_batcher(ctx);
        // Reaper de liveness : ici et non connect_workers — `loco start`
        // sans flag est ServerOnly (connect_workers jamais appelé).
        crate::services::device_liveness::spawn_reaper(ctx);
        Ok(router)
    }

    async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()> {
        // Appelé uniquement en mode BackgroundQueue — c'est-à-dire quand le
        // process drive la queue (`loco start --server-and-worker` ou
        // `--worker-only`). Le reaper de liveness vit dans after_routes
        // (doit tourner y compris en ServerOnly).
        queue
            .register(crate::workers::build_firmware::BuildFirmwareWorker::build(
                ctx,
            ))
            .await?;
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(crate::tasks::seed::Seed);
    }

    async fn truncate(ctx: &AppContext) -> Result<()> {
        // Utilisé par les tests (config `dangerously_truncate`) : vide toutes
        // les tables applicatives (migrations + queue loco exclues), ids
        // réinitialisés pour des tests déterministes. Portable PG/sqlite :
        // catalogue de tables par backend, puis TRUNCATE côté PG ou DELETE
        // dans UNE transaction avec `PRAGMA defer_foreign_keys` côté sqlite
        // (le pool peut répartir des statements hors transaction sur
        // plusieurs connexions — une transaction épingle une seule connexion).
        use sea_orm::{ConnectionTrait, DatabaseBackend, Statement, TransactionTrait};
        let backend = ctx.db.get_database_backend();
        // Tables internes exclues : suivi des migrations + queue loco (pg/sqlt).
        let excluded = [
            "seaql_migrations",
            "pg_loco_queue",
            "sqlt_loco_queue",
            "sqlt_loco_queue_lock",
        ];
        let (sql, col) = match backend {
            DatabaseBackend::Sqlite => (
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%'",
                "name",
            ),
            _ => (
                "SELECT tablename FROM pg_tables WHERE schemaname = 'public'",
                "tablename",
            ),
        };
        let rows = ctx
            .db
            .query_all_raw(Statement::from_string(backend, sql.to_string()))
            .await?;
        let tables: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String>("", col).ok())
            .filter(|t| !excluded.contains(&t.as_str()))
            .collect();
        if tables.is_empty() {
            return Ok(());
        }
        match backend {
            DatabaseBackend::Sqlite => {
                ctx.db
                    .transaction(|txn| {
                        Box::pin(async move {
                            // Désaxe les FK pour la durée des DELETE (l'ordre
                            // des tables devient indifférent, repositionné au
                            // commit). Reset des autoincrement best-effort —
                            // sqlite_sequence n'existe qu'avec AUTOINCREMENT.
                            txn.execute_unprepared("PRAGMA defer_foreign_keys = ON")
                                .await?;
                            for t in &tables {
                                txn.execute_unprepared(&format!(r#"DELETE FROM "{t}""#))
                                    .await?;
                            }
                            let names = tables
                                .iter()
                                .map(|t| format!("'{t}'"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            let _ = txn
                                .execute_unprepared(&format!(
                                    "DELETE FROM sqlite_sequence WHERE name IN ({names})"
                                ))
                                .await;
                            Ok::<(), sea_orm::DbErr>(())
                        })
                    })
                    .await
                    .map_err(|e| loco_rs::Error::Message(e.to_string()))?;
            }
            _ => {
                ctx.db
                    .execute_unprepared(&format!(
                        "TRUNCATE {} RESTART IDENTITY CASCADE",
                        tables.join(", ")
                    ))
                    .await?;
            }
        }
        Ok(())
    }

    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        // Le seed catalogue passe par la tâche `seed` (register_tasks), qui
        // réutilise les fixtures YAML Django — pas par ce hook générique.
        Ok(())
    }
}
