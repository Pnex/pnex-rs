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

/// Version composée du serveur (semver + sha de build) — source de vérité
/// unique : le hook `Hooks::app_version` délègue ici et le contrôleur meta
/// l'expose sur HTTP (`GET /api/v1/meta/version`).
pub fn app_version() -> String {
    format!(
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        option_env!("BUILD_SHA")
            .or(option_env!("GITHUB_SHA"))
            .unwrap_or("dev")
    )
}

/// Production server host imposed by the deployment (`PNEX_PROD_HOST`, bare
/// host such as `iot.example.com` or `iot.example.com:8443`). When set,
/// firmware builds always target it and users can no longer pick or register
/// another PNeX server host. `None` when unset or blank (dev, self-hosted).
pub fn prod_host() -> Option<String> {
    parse_prod_host(std::env::var("PNEX_PROD_HOST").ok().as_deref())
}

/// Blank means unset; a pasted scheme or trailing slash is tolerated since
/// devices and builds expect a bare host.
fn parse_prod_host(raw: Option<&str>) -> Option<String> {
    let host = raw?.trim();
    let host = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("wss://"))
        .unwrap_or(host)
        .trim_end_matches('/');
    (!host.is_empty()).then(|| host.to_string())
}

/// Idempotent catalog seed at API boot (models, boards, conversions…), so a
/// fresh container deployment has a usable device catalog without a manual
/// `task seed`. On by default in production, opt-in elsewhere; override with
/// `PNEX_AUTO_SEED=true|false`. Tiers are only seeded in SaaS mode.
/// Non-fatal: a failure is logged and the server still starts.
async fn seed_catalog_at_boot(ctx: &AppContext) {
    let enabled = match std::env::var("PNEX_AUTO_SEED") {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => ctx.environment == Environment::Production,
    };
    if !enabled {
        return;
    }
    let base = std::env::var("PNEX_FIXTURES_DIR").unwrap_or_else(|_| "fixtures".to_string());
    let with_tiers = crate::services::retention::DeploymentMode::from_env()
        == crate::services::retention::DeploymentMode::Saas;
    match crate::tasks::seed::seed_catalog(&ctx.db, std::path::Path::new(&base), with_tiers).await {
        Ok(()) => tracing::info!(%base, with_tiers, "catalog seed done"),
        Err(e) => tracing::error!(%base, error = %e, "catalog seed failed"),
    }
}

#[async_trait]
impl Hooks for App {
    fn app_name() -> &'static str {
        "pnex-server"
    }

    fn app_version() -> String {
        // Sans préfixe : résout la fonction libre ci-dessus (pas de récursion
        // — les items du module ne sont pas masqués par les méthodes de trait).
        app_version()
    }

    async fn boot(
        mode: StartMode,
        environment: &Environment,
        mut config: Config,
    ) -> Result<BootResult> {
        // Egress policy of outbound requests to user-chosen hosts (R8):
        // `settings.egress` (lan | public | open), env `PNEX_EGRESS` wins.
        let egress = std::env::var("PNEX_EGRESS").ok().or_else(|| {
            config
                .settings
                .as_ref()
                .and_then(|s| s.get("egress"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        pnex_core::egress::init(pnex_core::egress::EgressPolicy::parse(
            egress.as_deref().unwrap_or_default(),
        ));
        // Several pods boot together with auto_migrate: on Postgres the
        // migration runs here under a session advisory lock (pods
        // serialize), then loco's own unlocked auto-migrate is disabled.
        // `dangerously_recreate` keeps the framework path (tests only).
        if config.database.auto_migrate && !config.database.dangerously_recreate {
            let migrated =
                crate::services::db_lock::migrate_under_lock::<Migrator>(&config.database.uri)
                    .await
                    .map_err(|e| loco_rs::Error::Message(format!("boot migration failed: {e}")))?;
            if migrated {
                config.database.auto_migrate = false;
            }
        }
        create_app::<Self, Migrator>(mode, environment, config).await
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::health::routes())
            .add_route(controllers::meta::routes())
            .add_route(controllers::oauth2::routes())
            .add_route(controllers::user_info::routes())
            .add_route(controllers::orgs::routes())
            .add_route(controllers::devices::routes())
            .add_route(controllers::devices::catalogue_routes())
            .add_route(controllers::builds::routes())
            // Référentiels Edge (wizard device : fin de la ressaisie).
            .add_route(controllers::edge_refs::routes())
            // Org secrets vault (D110–D118): CRUD, never a value.
            .add_route(controllers::secrets::routes())
            // Edge agent (D95): keys, enrollment, public enroll + downloads.
            .add_route(controllers::edge_agents::routes())
            // Registre « Fonctions » (groupe « Automation ») — versionné +
            // test en live via le binaire runtime.
            .add_route(controllers::functions::routes())
            .add_route(controllers::firmware_projects::routes())
            .add_route(controllers::flows::routes())
            .add_route(controllers::media::routes())
            .add_route(controllers::stitch_jobs::routes())
            // POI-first (D35) : le contrôleur sites est supplanté.
            .add_route(controllers::pois::routes())
            // Couche d'organisation transverse (D42) : labels/containment/edges.
            .add_route(controllers::resources::routes())
            // Studio SCADA (D40/D41) : dashboards versionnés + bibliothèque.
            .add_route(controllers::dashboards::routes())
            .add_route(controllers::viz_widgets::routes())
            .add_route(controllers::controls::routes())
            // Mélanges de fluides personnalisés (CoolProp in-process).
            .add_route(controllers::fluid_mixtures::routes())
            // Diagrammes thermodynamiques (pnex-coolprop in-process).
            .add_route(controllers::thermo::routes())
            .add_route(controllers::tours::routes())
            .add_route(controllers::public_tours::routes())
            .add_route(controllers::annotation_layers::routes())
            .add_route(controllers::annotation_layers::media_routes())
            .add_route(controllers::dashboard::routes())
            .add_route(controllers::visualization::routes())
            // Org shared memory (flow memory-write) for dashboards.
            .add_route(controllers::memory::routes())
            .add_route(controllers::ai::routes())
            // Notifications (D49–D54) : canaux/templates/journal + bus WS +
            // endpoint interne de livraison (nœud flow / loopback Test).
            .add_route(controllers::notify::routes())
            .add_route(controllers::ws_notify::routes())
            .add_route(controllers::ws_ticket::routes())
            .add_route(controllers::internal_notify::routes())
            // Écriture device depuis le runtime (nœud pnex-device-write) —
            // token interne settings.flow.runtime_token.
            .add_route(controllers::internal_flow::routes())
            .add_route(controllers::flow_cluster::routes())
            .add_route(controllers::ws_ingest::routes())
            .add_route(controllers::ws_device::routes())
            // Camera & video (D73–D80): uplink + live WS, settings/segments API.
            .add_route(controllers::ws_camera::routes())
            .add_route(controllers::cameras::routes())
            .add_route(controllers::camera_recordings::routes())
            .add_route(controllers::events::routes())
            .add_route(controllers::ml_models::routes())
            .add_route(controllers::ml_models::internal_routes())
            .add_route(controllers::pins::routes())
            .add_route(controllers::ota::routes())
            .add_route(controllers::boards::routes())
            // Global typeahead (D69) — additive endpoint, read-only.
            .add_route(controllers::global_search::routes())
            // System: O2 retention/cleanup + platform status (D72).
            .add_route(controllers::system::routes())
    }

    async fn after_routes(router: axum::Router, ctx: &AppContext) -> Result<axum::Router> {
        // Valkey is mandatory (D108): device liveness leases live there.
        crate::services::device_liveness::init(&ctx.config).await?;
        // The secrets keyring is mandatory too (D112).
        crate::services::secrets::boot_guard(&ctx.config)
            .map_err(|e| loco_rs::Error::string(&e.to_string()))?;
        // Multi-pod deployments must not keep media on a pod-local disk.
        crate::services::media::media_boot_guard(&ctx.config)
            .map_err(|e| loco_rs::Error::string(&e))?;
        seed_catalog_at_boot(ctx).await;
        // Batcher télémétrie → OpenObserve (no-op si non configuré : le sink
        // noop reste en place — tests, déploiements sans télémétrie).
        crate::services::openobserve::spawn_batcher(ctx).await;
        // Reaper de liveness : ici et non connect_workers — `loco start`
        // sans flag est ServerOnly (connect_workers jamais appelé).
        crate::services::device_liveness::spawn_reaper(ctx);
        // Notify delivery journal → O2 (D86): background writer.
        crate::services::notify_journal::spawn_writer(ctx);
        crate::services::notify::warn_missing_token(ctx);
        // Cross-pod fan-out of /ws/notify frames (no-op without Valkey).
        crate::services::notify::spawn_bus(ctx).await;
        crate::services::ota::spawn_watchdog(ctx);
        // Camera frame bus (Valkey → flow camera nodes) + segment retention.
        crate::services::camera::spawn_bus(ctx).await;
        // Cross-pod device command bus (D107; no-op without Valkey).
        crate::services::device_bus::spawn(ctx).await;
        crate::services::video::spawn_pruner(ctx);
        // Hourly O2 retention reconcile (D72): new streams get the value.
        crate::services::retention::spawn_reconciler(ctx);
        // Erasure of inactive assistant conversations (D145).
        crate::services::ai::conversations::spawn_purger(ctx);
        // Flow execution cluster (D106): this process as a flow worker (one
        // supervised runtime for the orgs placed on it) and as a placement
        // controller candidate — here and not connect_workers (ServerOnly
        // too), gated on settings.flow.enabled.
        crate::services::flow_supervisor::warn_colocated_runtime(&ctx.config);
        crate::services::flow_cluster::boot(ctx).await;
        // Cross-pod rate limit of unauthenticated / sensitive routes
        // (oauth2, agent enroll, device sockets, public links).
        let rate_limit = std::sync::Arc::new(
            crate::services::rate_limit::RateLimitState::from_config(&ctx.config).await,
        );
        let router = router.layer(axum::middleware::from_fn_with_state(
            rate_limit,
            crate::services::rate_limit::middleware,
        ));
        // `/internal/*` is never served to requests that came through the
        // public edge (SEC-4).
        let router = router.layer(axum::middleware::from_fn(
            crate::auth::internal_guard::middleware,
        ));
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
        // Take 360 V2 : assemblage HD serveur des prises 360° (rare, même
        // queue que les builds firmware — 1 worker partagé, plan B3).
        queue
            .register(crate::workers::stitch_panorama::StitchPanoramaWorker::build(ctx))
            .await?;
        // Custom firmware compile-only checks (custom-firmware.md D90):
        // must run where PlatformIO lives (pnex-builder in containers).
        queue
            .register(crate::workers::firmware_check::FirmwareCheckWorker::build(
                ctx,
            ))
            .await?;
        Ok(())
    }

    async fn on_shutdown(ctx: &AppContext) {
        // Hand the orgs of the local flow worker over before exiting (D106).
        crate::services::flow_cluster::shutdown().await;
        // Flush buffered writers (O2 telemetry batches, notify journal):
        // their senders live in statics, the channels never close by
        // themselves. Bounded: the pod's grace period is finite.
        crate::services::drain::shutdown(std::time::Duration::from_secs(10)).await;
        // Sweeps move to another pod at once instead of after the expiry.
        crate::services::singleton::release_all(&ctx.db).await;
    }

    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(crate::tasks::seed::Seed);
        tasks.register(crate::tasks::seed_demo::SeedDemo);
        tasks.register(crate::tasks::secrets_rekey::SecretsRekey);
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
        // Ids restart: liveness leases of the previous test must go too.
        crate::services::device_liveness::init(&ctx.config).await?;
        crate::services::device_liveness::clear_all().await?;
        Ok(())
    }

    async fn seed(_ctx: &AppContext, _base: &Path) -> Result<()> {
        // The catalog seed goes through the `seed` task (register_tasks), which
        // reuses the legacy YAML fixtures — not through this generic hook.
        Ok(())
    }
}

#[cfg(test)]
mod prod_host_tests {
    use super::parse_prod_host;

    #[test]
    fn unset_or_blank_is_none() {
        assert_eq!(parse_prod_host(None), None);
        assert_eq!(parse_prod_host(Some("  ")), None);
    }

    #[test]
    fn bare_host_is_kept_and_scheme_stripped() {
        assert_eq!(
            parse_prod_host(Some(" iot.example.com ")).as_deref(),
            Some("iot.example.com")
        );
        assert_eq!(
            parse_prod_host(Some("https://iot.example.com:8443/")).as_deref(),
            Some("iot.example.com:8443")
        );
    }
}
