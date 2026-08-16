//! Workers de fond (queue PostgreSQL `pg_loco_queue`). Enregistrés dans
//! `connect_workers` — appelé uniquement en mode `BackgroundQueue`, drivé
//! par `loco start --server-and-worker` (même binaire, flag différent).

pub mod build_firmware;
