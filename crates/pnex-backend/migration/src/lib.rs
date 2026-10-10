#![allow(elided_lifetimes_in_paths)]
#![allow(clippy::wildcard_imports)]
//! SeaORM migrations of the PNeX backend (Loco).
//!
//! `m20261009_000001_baseline` is the whole schema of 0.1.0, as plain
//! PostgreSQL (`baseline/postgres.sql`). It replaces the
//! pre-release chain: a database created before it is refused (its applied
//! migrations no longer exist) and must be recreated. Every later change
//! is a new migration following docs/architecture/migrations.md.
//! PostgreSQL is the only supported backend (decision #19).

pub use sea_orm_migration::prelude::*;

mod m20261009_000001_baseline;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20261009_000001_baseline::Migration)]
    }
}
