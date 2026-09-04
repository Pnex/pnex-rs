//! PNEX core — types partagés backend ↔ frontend.
//!
//! Contraintes (migration.md Phase 1) :
//! - ce crate DOIT compiler pour `x86_64-unknown-linux-gnu` **et**
//!   `wasm32-unknown-unknown` ;
//! - donc **aucune dépendance native** (pas de tokio, std::net, std::fs, … au
//!   niveau des types exposés) ;
//! - il ne contient que des DTO/constantes partagés, pas de logique métier.

pub mod api;
pub use api::*;

pub mod builds;
pub use builds::*;

pub mod dashboard;
pub use dashboard::*;

pub mod devices;
pub use devices::*;

/// Flows ETL (décision D18) — modèle typé + validation + projection flows.json.
pub mod flow;
pub use flow::*;

/// Évaluateur d'expressions calc (nœud `calc`) — pur, wasm-safe, zéro dep.
pub mod calc;
pub use calc::*;

/// Nommage métriques/clés — source de vérité unique backend ↔ runtime
/// (`normalize_measurement_name` derrière la feature `naming`).
pub mod naming;
pub use naming::*;

/// Messages prompb (remote-write OpenObserve) — feature `prompb`, jamais
/// compilée pour le front wasm.
#[cfg(feature = "prompb")]
pub mod prompb;
#[cfg(feature = "prompb")]
pub use prompb::*;

pub mod pagination;
pub use pagination::*;

/// Protocole fil `/ws/device` (Brick 0) — source de vérité du contrat device.
pub mod proto;
pub use proto::*;

/// Chip-caps ESP8266 — validation des pins (point unique, Brick 0).
pub mod caps;
pub use caps::*;

/// Overlays board en data (`mcu_boards.details`) — types partagés (Brick 0).
pub mod boards;
pub use boards::*;

pub mod telemetry;
pub use telemetry::*;

use serde::{Deserialize, Serialize};

/// Nom du service. Django répondait `og-device-hub` — obsolète, le service
/// s'appelle désormais `pnex-server` (renommage confirmé 2026-08-15).
pub const SERVICE_NAME: &str = "pnex-server";

/// Réponses du endpoint `/health/live` (parité Django `health/views.py`).
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

/// Identifiant d'un device (PK Django `DeviceRegistry`, préservée en cible).
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
