//! Devices domain DTOs — parity with the legacy `devices/serializers.py`
//! contracts (Phase 4), org scoping (D2) instead of the legacy `user`.
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

/// Predefined device name of the edge agent (D95) — a device without
/// firmware, pins or OTA (daemon on a computer).
pub const EDGE_AGENT_PREDEF: &str = "edge_agent";

/// `Announce.chip` sent by the edge agent (never a real SoC).
pub const EDGE_AGENT_CHIP: &str = "agent";

/// Catalogue models of the generic PneX family: pins driven from the UI,
/// and the only boards that may run a custom firmware project.
pub const GENERIC_IO_PREDEFS: &[&str] = &[
    "generic_esp8266",
    "generic_esp32c3",
    "generic_esp32",
    "generic_esp32s3",
    "generic_esp32c6",
];

/// Product family of a catalogue model (edge-model.md §2 bis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceFamily {
    /// Generic PneX firmware (I/O from the UI), or a custom IDE project
    /// compiled for the same board.
    Generic,
    /// Typed board whose firmware is fixed by the model and maintained by
    /// PneX: no firmware choice.
    Predefined,
    /// Edge agent: software on a computer, not a board.
    Agent,
}

impl DeviceFamily {
    /// Family of a predefined device, by its catalogue name.
    pub fn of(predefined_name: &str) -> Self {
        if predefined_name == EDGE_AGENT_PREDEF {
            Self::Agent
        } else if GENERIC_IO_PREDEFS.contains(&predefined_name) {
            Self::Generic
        } else {
            Self::Predefined
        }
    }

    /// A custom firmware project may only replace the generic firmware.
    pub fn accepts_custom_firmware(self) -> bool {
        self == Self::Generic
    }
}

// ─────────────────────── Catalogue global ───────────────────────

/// `GET /api/v1/device-capabilities` — parité `DeviceCapabilitySerializer`.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCapability {
    pub id: i64,
    pub name: String,
    /// « input » | « output » | « input_output ».
    pub mode: String,
}

/// `GET /api/v1/predefined-devices` — parité `PredefinedDeviceSerializer`
/// (no id in the legacy contract: the unique `name` acts as the key).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
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
    /// Fluent key resolved client-side (i18n chantier); `description`
    /// stays the English fallback + server-side search field.
    #[serde(default)]
    pub description_i18n: Option<String>,
    #[serde(default)]
    pub revision: String,
    pub device_type: String,
    /// Capability names (legacy SlugRelatedField → list of strings).
    pub capabilities: Vec<String>,
    /// Nom du board MCU.
    pub board: String,
    /// Le modèle peut porter un écran intégré (`peripherals.screen` en base) —
    /// option de build « écran intégré » proposée à l'utilisateur.
    #[serde(default)]
    pub has_screen: bool,
    /// Kind du driver écran (« ssd1306 »…) si `has_screen`.
    #[serde(default)]
    pub screen_kind: Option<String>,
}

// ─────────────────────── Registre devices (org) ───────────────────────

/// Token d'un device — renvoyé au porteur pour le provisioning (chiffré côté
/// device avec `encryption_key`). Parité `get_device_token`.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
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
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct LatestBuild {
    pub success: bool,
    /// « queued » | « running » | « succeeded » | « failed ».
    #[serde(default)]
    pub build_phase: Option<String>,
    /// Version stampée du build (id record) — absente des records pré-OTA.
    #[serde(default)]
    pub fw_version: Option<String>,
    /// Staleness tripwire (migration 000031): `Some(true)` = the firmware
    /// tree embedded in the CURRENT server binary differs from the one this
    /// build compiled — the artifact predates the running code, rebuild
    /// recommended. `None` = legacy record (pre-fingerprint), unknown.
    #[serde(default)]
    pub sources_stale: Option<bool>,
    /// RFC 3339 — dernier changement de phase.
    pub updated_at: String,
    /// Version of the newest SUCCESSFUL, OTA-deployable build of the device
    /// (the OTA target by default). May differ from `fw_version` above when
    /// the newest build is queued/running/failed. `None` = no deployable
    /// build.
    #[serde(default)]
    pub deployable_version: Option<String>,
    /// Failure reason of a failed build (O4, see `BuildRecord`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    /// Compiler output tail of a failed build, credentials masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
}

/// Current OTA deployment status hydrated into `Device` (list + detail).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct OtaAssignmentState {
    pub state: String,
    pub target_version: String,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub error: Option<String>,
    /// RFC 3339.
    #[serde(default)]
    pub updated_at: String,
}

/// Device du registre — `GET /api/v1/devices` et détail (même forme en liste
/// et en détail, parité `DeviceRegistrySerializer`).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: i64,
    /// D2: owning org instead of the legacy `user`.
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
    /// Live WS session (`device_states.connected`). False = actuellement
    /// déconnecté (ou jamais connecté) — l'UI grise le déploiement OTA.
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub device_token: Option<DeviceTokenInfo>,
    /// Statut du dernier build firmware (`null` si jamais compilé) —
    /// Rust-side enrichment, no legacy equivalent.
    #[serde(default)]
    pub latest_build: Option<LatestBuild>,
    /// Firmware version announced by the device ("who runs where",
    /// edge-model.md §9) — None until the first announce of a stamped build.
    #[serde(default)]
    pub fw_version: Option<String>,
    /// OTA admission: the announced caps manifest contains the `ota` cap.
    #[serde(default)]
    pub ota_ready: bool,
    /// Active (non-terminal) OTA deployment, if any — list + detail
    /// enrichment; progress updates as the device reports OtaState frames.
    #[serde(default)]
    pub ota: Option<OtaAssignmentState>,
    pub allow_dynamic_measurements: bool,
    /// Noms des mesures découvertes (uniquement si dynamic autorisé).
    #[serde(default)]
    pub discovered_measurements: Vec<String>,
    pub max_unique_measurements: i32,
    /// Custom firmware project the device runs (custom-firmware.md D94) —
    /// `None` = generic preset firmware.
    #[serde(default)]
    pub firmware_project_id: Option<i64>,
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
    /// Board variante figée à l'enregistrement (`mcu_boards.id`) — absent =
    /// board par défaut du modèle. Validé : existence + soc compatible avec
    /// le board par défaut du modèle.
    #[serde(default)]
    pub board_id: Option<i64>,
    /// Custom firmware chosen at provisioning (custom-firmware.md D94) —
    /// must belong to the org and target the board's chip. Absent = the
    /// generic preset firmware.
    #[serde(default)]
    pub firmware_project_id: Option<i64>,
}

/// Corps du `PUT /api/v1/devices/{id}/peripherals` — écran choisi par
/// l'utilisateur (`null` = aucun, kind string = écran externe, `true` =
/// compat rows legacy) : pins réservées + define au build.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDevicePeripherals {
    pub screen: crate::boards::ScreenChoice,
}

/// Body of `PUT/PATCH /api/v1/devices/{id}` — **metadata only**
/// (legacy contract: any other field → 400 "Only metadata updates are
/// allowed.", enforced by the controller since the payload is rejected upstream).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateDevice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {

    #[test]
    fn device_family_by_catalogue_name() {
        assert_eq!(DeviceFamily::of("generic_esp32"), DeviceFamily::Generic);
        assert_eq!(DeviceFamily::of("soil_sensor"), DeviceFamily::Predefined);
        assert_eq!(
            DeviceFamily::of("generic_esp32cam"),
            DeviceFamily::Predefined
        );
        assert_eq!(DeviceFamily::of(EDGE_AGENT_PREDEF), DeviceFamily::Agent);
        assert!(DeviceFamily::of("generic_esp8266").accepts_custom_firmware());
        assert!(!DeviceFamily::of("soil_sensor").accepts_custom_firmware());
        assert!(!DeviceFamily::of(EDGE_AGENT_PREDEF).accepts_custom_firmware());
    }

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
            "predefined_device_name": "relay_1ch",
            "device_type": "actuator",
            "capabilities": [{ "id": 1, "name": "relay", "mode": "output" }],
            "active": false,
            "last_seen": "2026-08-16T12:00:00+00:00",
            "connected": false,
            "device_token": {
                "token": "tok",
                "encryption_key": "key",
                "is_active": true,
                "created": "2026-08-16T10:00:00+00:00"
            },
            "latest_build": {
                "success": true,
                "build_phase": "succeeded",
                "fw_version": "42",
                "sources_stale": null,
                "updated_at": "2026-08-16T11:00:00+00:00",
                "deployable_version": "42"
            },
            "allow_dynamic_measurements": false,
            "discovered_measurements": [],
            "max_unique_measurements": 100,
            "firmware_project_id": null,
            "fw_version": "42",
            "ota_ready": true,
            "ota": null
        }"#;
        let device: Device = serde_json::from_str(json).unwrap();
        assert_eq!(device.org_id, 7);
        assert_eq!(device.device_token.as_ref().unwrap().token, "tok");
        assert_eq!(
            device.last_seen.as_deref(),
            Some("2026-08-16T12:00:00+00:00")
        );
        assert_eq!(device.fw_version.as_deref(), Some("42"));
        assert!(device.ota_ready);
        let build = device.latest_build.as_ref().unwrap();
        assert!(build.success);
        assert_eq!(build.build_phase.as_deref(), Some("succeeded"));
        assert_eq!(build.fw_version.as_deref(), Some("42"));
        assert_eq!(build.deployable_version.as_deref(), Some("42"));
        let back = serde_json::to_value(&device).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
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
            r#"{ "device_id": "4-chan-dev-shan", "predefined_device_name": "relay_1ch" }"#,
        )
        .unwrap();
        assert!(payload.metadata.is_none());
    }

    #[test]
    fn predefined_device_capabilities_are_names() {
        // Legacy SlugRelatedField: capabilities = list of strings.
        let json = r#"{
            "name": "relay_1ch",
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
        assert_eq!(
            pd.capabilities,
            vec!["relay".to_string(), "pwm".to_string()]
        );
        // No id in the legacy contract.
        let back = serde_json::to_value(&pd).unwrap();
        assert!(back.get("id").is_none());
    }
}
