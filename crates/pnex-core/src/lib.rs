//! PNEX core — types partagés backend ↔ frontend.
//!
//! Contraintes :
//! - ce crate DOIT compiler pour `x86_64-unknown-linux-gnu` **et**
//!   `wasm32-unknown-unknown` ;
//! - donc **aucune dépendance native** (pas de tokio, std::net, std::fs, … au
//!   niveau des types exposés) ;
//! - il ne contient que des DTO/constantes partagés, pas de logique métier.

/// Edge agent contract (D95).
pub mod agent;

pub mod api;
pub use api::*;

pub mod builds;
pub mod egress;
pub use builds::*;

pub mod dashboard;
pub use dashboard::*;

/// Documents viz « dashboard » (studio SCADA, D35/D36) — layout canvas
/// libre + validation partagée front/back.
pub mod viz;
pub use viz::*;

/// Mélanges de fluides personnalisés — DTO + validation partagée
/// front/back (conversion massique→molaire côté serveur via CoolProp).
pub mod fluid_mixture;
pub use fluid_mixture::*;

/// Quantities and units of the CoolProp flow node (pickers + boundary
/// conversion) — wasm-safe catalogue.
pub mod thermo_quantities;
pub use thermo_quantities::*;

pub mod devices;
pub use devices::*;

/// Device catalog as typed code (D121): boards, predefined devices,
/// device types, capabilities.
pub mod catalog;

/// Référentiels Edge — credentials WiFi + hosts serveur PNeX org-scoped,
/// piochés par l'étape Config du wizard device (payload `CreateBuild`
/// inchangé). wasm-safe (serde seul).
pub mod edge;
pub use edge::*;

// Org secrets vault (D110–D118): references and list DTOs, no values.
pub mod secrets;
pub use secrets::*;

/// Fonctions utilisateur versionnées (registre « Fonctions », groupe
/// « Automation ») — directives d'interface, codegen wrapper JS, DTOs et
/// contrat du CLI de test. wasm-safe (serde + serde_json seuls).
pub mod firmware;

pub mod functions;
pub use functions::*;

/// Flows ETL (décision D18) — modèle typé + validation + projection flows.json.
pub mod flow;
pub use flow::*;

/// Studio — document de parcours de visite 3D (mode panorama), versionné
/// école flows ; assets média référencés par UUID (jamais dupliqués, D21).
pub mod tour;
pub use tour::*;

/// Annotations sur médias (D55–D60) — couches versionnées ancrées sur
/// `media_asset_id`, cibles faibles résolues au read ; doctrine
/// `docs/architecture/annotations.md`.
pub mod annotation;
pub use annotation::*;

/// Évaluateur d'expressions calc (nœud `calc`) — pur, wasm-safe, zéro dep.
pub mod calc;
pub use calc::*;

/// Nommage métriques/clés — source de vérité unique backend ↔ runtime
/// (`normalize_measurement_name` derrière la feature `naming`).
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

/// Media ingest (media-ingest.md P2.13): streams, segments, ASR profiles,
/// transcript records, server-built names.
pub mod media_ingest;

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
pub mod last_cache;
pub use last_cache::*;

/// Messages prompb (remote-write OpenObserve) — feature `prompb`, jamais
/// compilée pour le front wasm.
#[cfg(feature = "prompb")]
pub mod prompb;
#[cfg(feature = "prompb")]
pub use prompb::*;

// Device/agent link encryption (Noise, D156) — native consumers only.
#[cfg(feature = "frame-crypto")]
pub mod frame;

pub mod pagination;
pub use pagination::*;

/// Signed OTA images (SEC-18): message bound to device + version + digest.
pub mod ota_sig;

/// Protocole fil `/ws/device` (Brick 0) — source de vérité du contrat device.
pub mod proto;
pub use proto::*;

/// Notifications (D49–D54, amendées 2026-09-14) — contrat `notify-kinds.v1`
/// (FieldSpec des formulaires canaux), DTO canaux/templates/livraisons,
/// frame du bus WS. wasm-safe (uuid serde-only, pas de v4).
pub mod notify;
pub use notify::*;

/// Math de référence des cartes de régulation (TT + PID) — spécification
/// exécutable que le firmware `regulator` (C++) reproduit à l'identique
/// (golden vectors). Pur, wasm-safe, jamais exécuté côté serveur (D13/D17 :
/// la boucle tourne sur le device).
pub mod control;
pub use control::*;

/// Chip-caps ESP8266 — validation des pins (point unique, Brick 0).
pub mod caps;
pub use caps::*;

/// Overlays board en data (`mcu_boards.details`) — types partagés (Brick 0).
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

/// Machine error codes shared backend ↔ frontend (i18n) — doctrine and
/// `ALL` registry; `err-<kebab>` resolution happens client-side at render
/// time.
pub mod err_codes;
pub use err_codes::*;

use serde::{Deserialize, Serialize};

/// Service name reported by the health endpoints.
pub const SERVICE_NAME: &str = "pnex-server";

/// Responses of the `/health/live` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthLive {
    pub status: String,
    pub service: String,
}

/// Réponses du endpoint `/health/ready` (DB critique, cache non critique).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReady {
    pub status: String,
    pub database: String,
    pub cache: String,
}

/// Identifiant d'une organisation PNEX (le tenant, décision D2).
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
            status: "ok".into(),
            service: SERVICE_NAME.into(),
        };
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, r#"{"status":"ok","service":"pnex-server"}"#);
        let back: HealthLive = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, h.status);
    }
}
