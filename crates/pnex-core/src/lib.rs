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

pub mod devices;
pub use devices::*;

pub mod pagination;
pub use pagination::*;

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
