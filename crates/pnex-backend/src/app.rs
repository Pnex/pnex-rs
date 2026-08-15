use async_trait::async_trait;
use loco_rs::{
    app::{AppContext, Hooks},
    bgworker::Queue,
    boot::{create_app, BootResult, StartMode},
    config::Config,
    controller::AppRoutes,
    environment::Environment,
    task::Tasks,
    Result,
};

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
        create_app::<Self>(mode, environment, config).await
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes()
            .add_route(controllers::health::routes())
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        // Phase 6 : workers firmware-build etc. (queue PostgreSQL).
        Ok(())
    }

    fn register_tasks(_tasks: &mut Tasks) {
        // Phase 2+ : tâches seed/maintenance (équivalents commandes Django).
    }
}
