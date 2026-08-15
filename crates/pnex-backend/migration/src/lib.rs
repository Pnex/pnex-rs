#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
//! Migrations SeaORM du backend PNEX (cadre Loco).
//!
//! Une migration par domaine, dans l'ordre des dépendances FK :
//! 1. orgs/users — identité & tenancy (D2 : l'org est le tenant)
//! 2. devices — catalogue + registre + tokens
//! 3. etl — conversions, formules, fluides
//! 4. sites — sites/SVG/diagrammes/annotations (PK UUID)
//! 5. firmware — build records

pub use sea_orm_migration::prelude::*;

mod m20260815_000001_orgs_users;
mod m20260815_000002_devices;
mod m20260815_000003_etl;
mod m20260815_000004_sites;
mod m20260815_000005_firmware;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260815_000001_orgs_users::Migration),
            Box::new(m20260815_000002_devices::Migration),
            Box::new(m20260815_000003_etl::Migration),
            Box::new(m20260815_000004_sites::Migration),
            Box::new(m20260815_000005_firmware::Migration),
        ]
    }
}
