//! Background workers (PostgreSQL queue `pg_loco_queue`). Registered in
//! `connect_workers` — called only in `BackgroundQueue` mode, driven
//! by `loco start --server-and-worker` (same binary, different flag).

pub mod build_firmware;
pub mod check_asr_model;
pub mod firmware_check;
pub mod index_document;
pub mod media_analysis;
pub mod stitch_panorama;
pub mod transcribe_segment;
