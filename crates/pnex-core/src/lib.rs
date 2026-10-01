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

/// Vision contract (D81–D83): model spec, detections, registry DTOs,
/// `vision-detect` node config.
pub mod vision;

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

// Device/agent frame encryption (D8) — native consumers only.
#[cfg(feature = "frame-crypto")]
pub mod frame;

pub mod pagination;
pub use pagination::*;

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

pub mod telemetry;
pub use telemetry::*;

/// Couche d'organisation transverse (D42) : labels / containment / edges.
pub mod resources;
pub use resources::*;

/// Machine error codes shared backend ↔ frontend (i18n) — doctrine and
/// `ALL` registry; `err-<kebab>` resolution happens client-side at render
/// time.
pub mod err_codes;
pub use err_codes::*;

use serde::{Deserialize, Serialize};

/// Service name. The legacy stack answered `og-device-hub` — obsolete, the
/// service is now named `pnex-server` (rename confirmed 2026-08-15).
pub const SERVICE_NAME: &str = "pnex-server";

/// Responses of the `/health/live` endpoint (parity with the legacy `health/views.py`).
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

/// Device identifier (legacy `DeviceRegistry` PK, preserved on the target side).
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
