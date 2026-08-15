use loco_rs::cli;
use pnex_backend::app::App;
use pnex_migration::Migrator;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    cli::main::<App, Migrator>().await
}
