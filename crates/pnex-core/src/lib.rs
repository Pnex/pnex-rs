//! PNEX core — types shared backend ↔ frontend.
//!
//! Constraints:
//! - this crate MUST compile for `x86_64-unknown-linux-gnu` **and**
//!   `wasm32-unknown-unknown`;
//! - hence **no native dependency** (no tokio, std::net, std::fs, … at the
//!   level of the exposed types);
//! - it only holds shared DTOs/constants, no business logic.

pub mod agent;
/// Edge agent contract (D95).
/// Applications declared in an org (external-apps.md D433).
pub mod apps;

pub mod api;
pub use api::*;

pub mod builds;
pub mod egress;
pub mod geo;
/// Position tracks of a case: normalisation, signal gaps, sampling,
/// generalisation (cases-workspace.md K25–K28).
pub mod geo_area;
pub mod track;
pub use builds::*;

pub mod dashboard;
pub use dashboard::*;

/// Viz "dashboard" documents (SCADA studio, D35/D36) — free canvas
/// layout + validation shared front/back.
pub mod viz;
pub use viz::*;

/// Custom fluid mixtures — DTO + validation shared
/// front/back (mass→molar conversion on the server via CoolProp).
pub mod fluid_mixture;
pub use fluid_mixture::*;

/// Quantities and units of the CoolProp flow node (pickers + boundary
/// conversion) — wasm-safe catalogue.
pub mod thermo_quantities;
pub use thermo_quantities::*;

pub mod decide;
pub mod devices;
pub use devices::*;

/// Device catalog as typed code (D121): boards, predefined devices,
/// device types, capabilities.
pub mod catalog;

/// Edge references — WiFi credentials + org-scoped PNeX server hosts,
/// picked by the Config step of the device wizard (`CreateBuild` payload
/// unchanged). wasm-safe (serde only).
pub mod edge;
pub use edge::*;

// Org secrets vault (D110–D118): references and list DTOs, no values.
pub mod secrets;
pub use secrets::*;

/// Versioned user functions ("Functions" registry, "Automation"
/// group) — interface directives, JS wrapper codegen, DTOs and
/// test CLI contract. wasm-safe (serde + serde_json only).
pub mod firmware;

pub mod functions;
pub use functions::*;

/// Pages (pages.md #21): collaborative documents, DTOs and editor schema
/// allowlist. wasm-safe.
pub mod pages;

/// ETL flows (decision D18) — typed model + validation + flows.json projection.
pub mod flow;
pub use flow::*;

/// Studio — 3D visit tour document (panorama mode), versioned
/// flows school; media assets referenced by UUID (never duplicated, D21).
pub mod tour;
pub use tour::*;

/// Media annotations (D55–D60) — versioned layers anchored on
/// `media_asset_id`, weak targets resolved at read; doctrine
/// `docs/architecture/annotations.md`.
pub mod annotation;
pub use annotation::*;

/// Calc expression evaluator (`calc` node) — pure, wasm-safe, zero dep.
pub mod calc;
pub use calc::*;

/// Metric/key naming — single source of truth backend ↔ runtime
/// (`normalize_measurement_name` behind the `naming` feature).
pub mod naming;
pub use naming::*;

/// Camera & video contract (camera-video.md D73–D80): uplink frame header,
/// frame sizes, capture settings, Valkey frame bus keys, segment DTO.
pub mod camera;

/// Minimal MJPEG-in-AVI writer/reader (recording segments, D78).
pub mod avi;

/// JSON events stored as OpenObserve logs (D84): stream naming, levels,
/// `event-log` node config, API DTOs.
pub mod events;

// Shared flow memory (Valkey) contract: memory-write / memory-read nodes.
pub mod memory;

/// Org controls (D125–D127): surfaces write a control value, flows listen
/// through the `control-source` node.
pub mod ui_control;

/// Home cards of the dashboards (D138): composite cards driving N controls
/// by role.
pub mod home;

/// Weather source node contract (D140): providers, normalized payloads.
pub mod weather;

/// Vision contract (D81–D83): model spec, detections, registry DTOs,
/// `vision-detect` node config.
pub mod vision;
pub mod vision_ai;

/// Media ingest (media-ingest.md P2.13): streams, segments, ASR profiles,
/// transcript records, server-built names.
pub mod media_analysis;
pub mod media_ingest;
pub mod media_review;

/// Voice identification by consented enrolment (voice-id.md, cases.md
/// K19): profile DTOs, windowing, centroid, calibrated threshold, verdict.
pub mod voice_id;

/// Browser agents (extension-collector.md D254–D269): pairing, missions,
/// `/ws/extension` protocol.
pub mod browser_agent;

/// Accent- and case-insensitive text, Rust and SQL (crm.md D447).
pub mod fold;

/// Mailboxes as sources, messages, the `mail_received` trigger (mail.md).
pub mod mail;

/// Topic taxonomies (media-ingest.md D168): versioned topics, keyword
/// classifier, `topic_classify` node config.
pub mod taxonomy;

/// Time ranges (media-ingest.md D169, D182): named intervals, announced and
/// realigned; `range_upsert` node config.
pub mod time_range;

/// Aggregation by time range or time-of-day slice (ontology.md D182): the
/// `telemetry/aggregate` read primitive and the `range_bars` widget options.
pub mod aggregate;

/// Predictive telemetry (ml-vision.md step 3): `anomaly` / `forecast` node
/// configs, validation, port counts.
pub mod predictive;

/// Live last-value cache contract (backend write side / flow-runtime read
/// side) — pure, wasm-safe: key builder, `CachedSample` payload, TTL const,
/// freshness resolver.
pub mod lab;
pub mod last_cache;
pub use last_cache::*;

/// prompb messages (OpenObserve remote-write) — `prompb` feature, never
/// compiled for the wasm front.
#[cfg(feature = "prompb")]
pub mod prompb;
#[cfg(feature = "prompb")]
pub use prompb::*;

// Device/agent link encryption (Noise, D156) — native consumers only.
#[cfg(feature = "frame-crypto")]
pub mod frame;

/// Paragraph diff of two document versions (partner-watch.md D311).
pub mod text_diff;

/// Site navigation rules played by the server browser (site-recipes.md D323–D326).
pub mod site_recipe;

/// Web search providers and the `web_search` node (web-presence.md D360–D361).
pub mod org_resolve;
pub mod site_crawl;
pub mod web_search;
pub mod web_visit;

/// Readers of ontology import sources (partner-watch.md D301).
#[cfg(feature = "doc-extract")]
pub mod import_read;

/// Open-data ingestion flow nodes (ingestion.md D317–D322).
pub mod ingest;

pub mod pagination;
pub use pagination::*;

/// Signed OTA images (SEC-18): message bound to device + version + digest.
pub mod ota_sig;

/// `/ws/device` wire protocol (Brick 0) — source of truth of the device contract.
pub mod proto;
pub use proto::*;

/// Notifications (D49–D54, amended 2026-09-14) — `notify-kinds.v1` contract
/// (FieldSpec of the channel forms), channel/template/delivery DTOs,
/// WS bus frame. wasm-safe (uuid serde-only, no v4).
pub mod notify;
pub use notify::*;

/// Reference math of the control cards (TT + PID) — executable
/// specification that the `regulator` firmware (C++) reproduces identically
/// (golden vectors). Pure, wasm-safe, never executed on the server (D13/D17:
/// the loop runs on the device).
pub mod control;
pub use control::*;

/// ESP8266 chip-caps — pin validation (single point, Brick 0).
pub mod caps;
pub use caps::*;

/// Board overlays as data (`mcu_boards.details`) — shared types (Brick 0).
pub mod boards;
pub use boards::*;

/// Pin self-test bench (D122): plan, verdict rules and report check.
pub mod selftest;

pub mod telemetry;
pub use telemetry::*;

/// Couche d'organisation transverse (D42) : labels / containment / edges.
pub mod resources;
pub use resources::*;

/// Ontology meta-model (D176–D191): object and link types as data.
pub mod ontology;

/// Document extraction + chunking of the media library search (doc-search.md).
pub mod doc_extract;

/// Machine error codes shared backend ↔ frontend (i18n) — doctrine and
/// `ALL` registry; `err-<kebab>` resolution happens client-side at render
/// time.
pub mod err_codes;
pub use err_codes::*;

use serde::{Deserialize, Serialize};

/// Service name reported by the health endpoints.
pub const SERVICE_NAME: &str = "pnex-server";

/// State reported by the health probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Ok,
    /// Readiness only: a dependency failed.
    Degraded,
    Error,
    /// The dependency is not configured (deployments without telemetry).
    NotConfigured,
}

/// Responses of the `/health/live` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthLive {
    pub status: HealthState,
    pub service: String,
}

/// Response of `/health/ready`: the database is critical (`degraded` when it
/// fails), the telemetry backend (`cache`) is not.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthReady {
    pub status: HealthState,
    pub database: HealthState,
    /// OpenObserve telemetry backend.
    pub cache: HealthState,
}

/// Identifier of a PNEX organization (the tenant, decision D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrgId(pub i64);

/// Device identifier (numeric primary key of the device registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub i64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_live_serde_roundtrip() {
        let h = HealthLive {
            status: HealthState::Ok,
            service: SERVICE_NAME.into(),
        };
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, r#"{"status":"ok","service":"pnex-server"}"#);
        let back: HealthLive = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, h.status);
    }
}
