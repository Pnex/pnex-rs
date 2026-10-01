//! Contrat d'interopérabilité PNEX — source de vérité unique app ↔ serveur.
//!
//! Embarqué par le backend ET le front (wasm32 comme natif) : le numéro de
//! contrat compilé dans chaque binaire est comparé à l'exécution via
//! `GET {META_VERSION_PATH}` — une app périmée devant un serveur plus récent
//! (ou l'inverse) est rejetée proprement au boot au lieu de produire des
//! erreurs API opaques. Règle de bump : toute rupture d'API incrémente
//! `CONTRACT` ici, les deux côtés basculent ensemble au build suivant.
//!
//! wasm-safe (serde seul) — même exigence que `pnex-core`.

use serde::{Deserialize, Serialize};

/// Version du contrat attendue par CE binaire. Un bump = rupture d'API.
///
/// 2 (2026-09-12, D43) : `Poi.device_id` (colonne unique) remplacé par
/// `Poi.devices: Vec<DevicePlacement>` — plusieurs devices par POI ;
/// attacher via `POST /pois/{id}/devices`, déplacer via
/// `PATCH /pois/placements/{id}` (plus de PATCH `device_id` sur le POI).
pub const CONTRACT: u32 = 2;

/// Identité annoncée par le endpoint meta — alignée sur
/// `pnex_core::SERVICE_NAME` (convention.md : le service s'appelle
/// `pnex-server`). Le scan LAN compare ce champ pour distinguer un serveur
/// PNEX de toute autre chose répondant sur :5150.
pub const SERVICE: &str = "pnex-server";

/// Chemin du endpoint meta (scan LAN + porte de compatibilité).
pub const META_VERSION_PATH: &str = "/api/v1/meta/version";

/// Public download of the root CA that signs the TLS edge certificate
/// (D70, local mode). Native apps pin it after a trust-on-first-use
/// confirmation; phones and browsers install it from there. 404 when the
/// server has no local CA to hand out (no edge, or public Let's Encrypt).
pub const META_CA_PATH: &str = "/api/v1/meta/ca";

/// Carte d'identité du serveur, servie par `GET {META_VERSION_PATH}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Toujours [`SERVICE`] pour un serveur PNEX.
    pub service: String,
    /// Version composée (`"0.1.0 (dev)"` — semver + sha de build).
    pub version: String,
    /// Version du contrat d'API servie par ce binaire.
    pub contract: u32,
}

/// Compatibilité app ↔ serveur : égalité stricte des contrats (pas de
/// fenêtre de tolérance — un désaccord est une rupture, on la nomme).
#[must_use]
pub fn compatible(a: u32, b: u32) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_serde_roundtrip() {
        let info = ServerInfo {
            service: SERVICE.into(),
            version: "0.1.0 (dev)".into(),
            contract: CONTRACT,
        };
        let back: ServerInfo =
            serde_json::from_str(&serde_json::to_string(&info).unwrap()).expect("roundtrip");
        assert_eq!(back, info);
    }

    #[test]
    fn compatibilite_stricte() {
        assert!(compatible(CONTRACT, CONTRACT));
        assert!(!compatible(1, 2));
        assert!(!compatible(2, 1));
    }

    #[test]
    fn identite_service_est_pnex_server() {
        // Le scan LAN rejette tout ce qui ne s'annonce pas exactement ainsi.
        assert_eq!(SERVICE, "pnex-server");
    }
}
