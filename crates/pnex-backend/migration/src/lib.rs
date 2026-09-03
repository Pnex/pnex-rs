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
//! 6. ingestion — état live device (D9) + correspondance OpenObserve (D2)
//! 7. firmware artefacts — binaires en base (D5 v2)
//! 8. capability instances — état live des pins (Brick 0)
//! 9. flows ETL — graphe versionné append-only + deploy (D18)

pub use sea_orm_migration::prelude::*;

mod m20260815_000001_orgs_users;
mod m20260815_000002_devices;
mod m20260815_000003_etl;
mod m20260815_000004_sites;
mod m20260815_000005_firmware;
mod m20260816_000006_ingestion;
mod m20260817_000007_firmware_artifacts;
mod m20260819_000008_device_capability_instances;
mod m20260903_000009_flows;

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
            Box::new(m20260816_000006_ingestion::Migration),
            Box::new(m20260817_000007_firmware_artifacts::Migration),
            Box::new(m20260819_000008_device_capability_instances::Migration),
            Box::new(m20260903_000009_flows::Migration),
        ]
    }
}
