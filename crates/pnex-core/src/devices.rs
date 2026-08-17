//! DTO du domaine devices — parité des contrats Django `devices/serializers.py`
//! (Phase 4), avec le scoping org (D2) à la place du `user` Django.
//!
//! Deux familles :
//! - le **catalogue global** (lecture seule, partagé entre orgs) :
//!   `predefined-devices`, `device-capabilities` ;
//! - le **registre scopé org** : `devices` (CRUD + réactivation + quotas).
//!
//! Hors périmètre Phase 4 (décision utilisateur) : `actuator-channels` — la
//! config des canaux actionneurs attend la réflexion M2M (D13).
//!
//! Champs dates en chaînes RFC 3339 (sérialisation SeaORM), pas de chrono
//! dans le core (wasm32).

use serde::{Deserialize, Serialize};

// ─────────────────────── Catalogue global ───────────────────────

/// `GET /api/v1/device-capabilities` — parité `DeviceCapabilitySerializer`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapability {
    pub id: i64,
    pub name: String,
    /// « input » | « output » | « input_output ».
    pub mode: String,
}

/// `GET /api/v1/predefined-devices` — parité `PredefinedDeviceSerializer`
/// (pas d'id dans le contrat Django : `name` unique fait office de clé).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PredefinedDevice {
    pub name: String,
    #[serde(default)]
    pub pretty_name: Option<String>,
    #[serde(default)]
    pub prestashop_product_id: Option<String>,
    #[serde(default)]
    pub prestashop_buy_url: Option<String>,
    #[serde(default)]
    pub byod_doc_url: Option<String>,
    #[serde(default)]
    pub image_source_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub revision: String,
    pub device_type: String,
    /// Noms des capacités (SlugRelatedField Django → liste de strings).
    pub capabilities: Vec<String>,
    /// Nom du board MCU.
    pub board: String,
}

// ─────────────────────── Registre devices (org) ───────────────────────

/// Token d'un device — renvoyé au porteur pour le provisioning (chiffré côté
/// device avec `encryption_key`). Parité `get_device_token`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceTokenInfo {
    pub token: String,
    #[serde(default)]
    pub encryption_key: Option<String>,
    pub is_active: bool,
    /// RFC 3339.
    #[serde(default)]
    pub created: Option<String>,
}

/// Dernier build firmware du device — un record par (org, device_id) en base
/// (upsert au rebuild), hydraté dans le DTO pour l'UI (colonne Firmware).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestBuild {
    pub success: bool,
    /// « queued » | « running » | « succeeded » | « failed ».
    #[serde(default)]
    pub build_phase: Option<String>,
    /// RFC 3339 — dernier changement de phase.
    pub updated_at: String,
}

/// Device du registre — `GET /api/v1/devices` et détail (même forme en liste
/// et en détail, parité `DeviceRegistrySerializer`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    /// D2 : org propriétaire à la place du `user` Django.
    pub org_id: i64,
    /// Identifiant déclaré par le firmware (MAC, hostname…).
    pub device_id: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    pub predefined_device_name: String,
    /// Nom du type (sensor / actuator / mixed).
    pub device_type: String,
    /// Capacités du predefined device (objets {id, name, mode}).
    pub capabilities: Vec<DeviceCapability>,
    pub active: bool,
    /// Dernière donnée reçue (bail de vie Phase 5, `device_states`) —
    /// RFC 3339, absent si le device n'a jamais ingéré.
    #[serde(default)]
    pub last_seen: Option<String>,
    #[serde(default)]
    pub device_token: Option<DeviceTokenInfo>,
    /// Statut du dernier build firmware (`null` si jamais compilé) —
    /// enrichissement Rust, sans équivalent Django.
    #[serde(default)]
    pub latest_build: Option<LatestBuild>,
    pub allow_dynamic_measurements: bool,
    /// Noms des mesures découvertes (uniquement si dynamic autorisé).
    #[serde(default)]
    pub discovered_measurements: Vec<String>,
    pub max_unique_measurements: i32,
}

/// Corps du `POST /api/v1/devices`.
///
/// Si un device inactif porte déjà ce `device_id` dans l'org : réactivation
/// (200 + `{"detail": "Device reactivated successfully."}`), pas de création.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDevice {
    pub device_id: String,
    pub predefined_device_name: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

/// Corps du `PUT/PATCH /api/v1/devices/{id}` — **metadata uniquement**
/// (contrat Django : tout autre champ → 400 « Only metadata updates are
/// allowed. », vérifié par le contrôleur car la charge est rejetée en amont).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDevice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forme de sortie exacte d'un device (parité DeviceRegistrySerializer,
    /// org_id à la place de user).
    #[test]
    fn device_shape_roundtrip() {
        let json = r#"{
            "id": 42,
            "org_id": 7,
            "device_id": "4-chan-dev-shan",
            "metadata": { "location": "serre" },
            "predefined_device_name": "4_chan_relay",
            "device_type": "actuator",
            "capabilities": [{ "id": 1, "name": "relay", "mode": "output" }],
            "active": false,
            "last_seen": "2026-08-16T12:00:00+00:00",
            "device_token": {
                "token": "tok",
                "encryption_key": "key",
                "is_active": true,
                "created": "2026-08-16T10:00:00+00:00"
            },
            "latest_build": {
                "success": true,
                "build_phase": "succeeded",
                "updated_at": "2026-08-16T11:00:00+00:00"
            },
            "allow_dynamic_measurements": false,
            "discovered_measurements": [],
            "max_unique_measurements": 100
        }"#;
        let device: Device = serde_json::from_str(json).unwrap();
        assert_eq!(device.org_id, 7);
        assert_eq!(device.device_token.as_ref().unwrap().token, "tok");
        assert_eq!(
            device.last_seen.as_deref(),
            Some("2026-08-16T12:00:00+00:00")
        );
        let build = device.latest_build.as_ref().unwrap();
        assert!(build.success);
        assert_eq!(build.build_phase.as_deref(), Some("succeeded"));
        let back = serde_json::to_value(&device).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(json).unwrap());
    }

    #[test]
    fn latest_build_absent_parses() {
        // Charge sans le champ (client ancien / backend non enrichi) → None.
        let json = r#"{
            "id": 42,
            "org_id": 7,
            "device_id": "d",
            "metadata": null,
            "predefined_device_name": "soil_sensor",
            "device_type": "sensor",
            "capabilities": [],
            "active": true,
            "last_seen": null,
            "device_token": null,
            "allow_dynamic_measurements": false,
            "discovered_measurements": [],
            "max_unique_measurements": 100
        }"#;
        let device: Device = serde_json::from_str(json).unwrap();
        assert!(device.latest_build.is_none());
    }

    #[test]
    fn create_device_minimal() {
        // Charge minimale du contrat (docs/contracts/device.http).
        let payload: CreateDevice = serde_json::from_str(
            r#"{ "device_id": "4-chan-dev-shan", "predefined_device_name": "4_chan_relay" }"#,
        )
        .unwrap();
        assert!(payload.metadata.is_none());
    }

    #[test]
    fn predefined_device_capabilities_are_names() {
        // SlugRelatedField Django : capabilities = liste de strings.
        let json = r#"{
            "name": "4_chan_relay",
            "pretty_name": null,
            "prestashop_product_id": null,
            "prestashop_buy_url": null,
            "byod_doc_url": null,
            "image_source_url": null,
            "description": null,
            "revision": "v2",
            "device_type": "actuator",
            "capabilities": ["relay", "pwm"],
            "board": "esp32"
        }"#;
        let pd: PredefinedDevice = serde_json::from_str(json).unwrap();
        assert_eq!(pd.capabilities, vec!["relay".to_string(), "pwm".to_string()]);
        // Pas d'id dans le contrat Django.
        let back = serde_json::to_value(&pd).unwrap();
        assert!(back.get("id").is_none());
    }



}
