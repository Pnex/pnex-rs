//! Workers de fond (queue PostgreSQL `pg_loco_queue`). Enregistrés dans
//! `connect_workers` — appelé uniquement en mode `BackgroundQueue`, drivé
//! par `loco start --server-and-worker` (même binaire, flag différent).

pub mod build_firmware;
pub mod check_asr_model;
pub mod firmware_check;
pub mod index_document;
pub mod stitch_panorama;
pub mod transcribe_segment;
