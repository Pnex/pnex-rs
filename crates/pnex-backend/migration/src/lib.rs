#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
//! SeaORM migrations of the PNeX backend (Loco).
//!
//! `m20261001_000001_baseline` is the whole schema of the first release,
//! as plain SQL per backend (`baseline/postgres.sql`, `baseline/sqlite.sql`,
//! kept identical in shape by `tests/schema_parity.rs`). Every later
//! change is a new migration following docs/architecture/migrations.md:
//! additive first, destructive only in a later expand/contract step, and
//! always written for both PostgreSQL and SQLite.

pub use sea_orm_migration::prelude::*;

mod m20261001_000001_baseline;
mod m20261003_000002_controls;
mod m20261004_000003_control_origin;
mod m20261004_000004_ai_conversations;
mod m20261004_000005_org_ai_retention;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20261001_000001_baseline::Migration),
            Box::new(m20261003_000002_controls::Migration),
            Box::new(m20261004_000003_control_origin::Migration),
            Box::new(m20261004_000004_ai_conversations::Migration),
            Box::new(m20261004_000005_org_ai_retention::Migration),
        ]
    }
}
