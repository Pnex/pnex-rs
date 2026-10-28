//! Devices domain DTOs (Phase 4), scoped by org (D2).
//!
//! Two families:
//! - the **global catalogue** (read-only, shared across orgs):
//!   `predefined-devices`, `device-capabilities`;
//! - the **org-scoped registry**: `devices` (CRUD + reactivation + quotas).
//!
//! Dates are RFC 3339 strings (SeaORM serialization), no chrono in the
//! core (wasm32).

use serde::{Deserialize, Serialize};

/// Predefined device name of the edge agent (D95) — a device without
/// firmware, pins or OTA (daemon on a computer).
pub const EDGE_AGENT_PREDEF: &str = "edge_agent";

/// Predefined device name of a browser agent (extension-collector.md
/// D255): a browser extension paired from its options page, never created
/// from the device wizard.
pub const BROWSER_AGENT_PREDEF: &str = "browser_agent";

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
    /// Browser agent (D255): an extension, paired from the browser.
    Browser,
}

impl DeviceFamily {
    /// Family of a predefined device, by its catalogue name.
    pub fn of(predefined_name: &str) -> Self {
        if predefined_name == EDGE_AGENT_PREDEF {
            Self::Agent
        } else if predefined_name == BROWSER_AGENT_PREDEF {
            Self::Browser
        } else if GENERIC_IO_PREDEFS.contains(&predefined_name) {
            Self::Generic
        } else {
            Self::Predefined
        }
    }

    /// Software agents (edge or browser): no firmware, pins nor OTA.
    pub fn is_software(self) -> bool {
        matches!(self, Self::Agent | Self::Browser)
    }

    /// A custom firmware project may only replace the generic firmware.
    pub fn accepts_custom_firmware(self) -> bool {
        self == Self::Generic
    }
}

// ─────────────────────── Catalogue global ───────────────────────

/// `GET /api/v1/device-capabilities`.
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceCapability {
    pub id: i64,
    pub name: String,
    pub mode: crate::catalog::CapabilityMode,
}

/// `GET /api/v1/predefined-devices` (no id: the unique `name` acts as the key).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// Fluent key resolved client-side (i18n work); `description`
    /// stays the English fallback + server-side search field.
    #[serde(default)]
    pub description_i18n: Option<String>,
    #[serde(default)]
    pub revision: String,
    pub device_type: String,
    /// Capability names (list of strings).
    pub capabilities: Vec<String>,
    /// MCU board name.
    pub board: String,
    /// The model can carry a builtin screen (`peripherals.screen` in base),
    /// offered as a build option.
    #[serde(default)]
    pub has_screen: bool,
    /// Screen driver kind (`ssd1306`…) when `has_screen`.
    #[serde(default)]
    pub screen_kind: Option<String>,
}

// ─────────────────────── Device registry (org) ───────────────────────

/// Device token, returned to writers for provisioning (the device encrypts
/// with `encryption_key`).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceTokenInfo {
    pub token: String,
    pub encryption_key: String,
    pub is_active: bool,
    /// RFC 3339.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub created: Option<String>,
}

/// Latest firmware build of the device, hydrated into the DTO for the UI
/// (Firmware column).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LatestBuild {
    pub build_phase: crate::builds::BuildPhase,
    /// Stamped build version (record id); absent on pre-OTA records.
    #[serde(default)]
    pub fw_version: Option<String>,
    /// Staleness tripwire (migration 000031): `Some(true)` = the firmware
    /// tree embedded in the CURRENT server binary differs from the one this
    /// build compiled — the artifact predates the running code, rebuild
    /// recommended. `None` = legacy record (pre-fingerprint), unknown.
    #[serde(default)]
    pub sources_stale: Option<bool>,
    /// RFC 3339, last phase change.
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OtaAssignmentState {
    pub state: String,
    pub target_version: String,
    #[serde(default)]
    pub progress: Option<u8>,
    #[serde(default)]
    pub error: Option<String>,
    /// RFC 3339.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub updated_at: String,
}

/// Registered device: `GET /api/v1/devices` and detail (same shape in list
/// and detail).
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Device {
    pub id: i64,
    /// D2: owning org.
    pub org_id: i64,
    /// Identifier declared by the firmware (MAC, hostname…).
    pub device_id: String,
    pub predefined_device_name: String,
    /// Type name (`sensor`, `actuator`, `mixed`…).
    pub device_type: String,
    /// Capabilities of the predefined model.
    pub capabilities: Vec<DeviceCapability>,
    pub active: bool,
    /// Last data received (liveness lease), RFC 3339; absent when the
    /// device never ingested.
    #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub last_seen: Option<String>,
    /// Live websocket session. False = currently disconnected (or never
    /// connected); the UI greys out OTA deployment.
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub device_token: Option<DeviceTokenInfo>,
    /// Latest firmware build status (`null` when never built),
    /// server-side enrichment.
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
    /// Discovered measurement names (only when dynamic measurements are
    /// allowed).
    #[serde(default)]
    pub discovered_measurements: Vec<String>,
    pub max_unique_measurements: i32,
    /// Custom firmware project the device runs (custom-firmware.md D94) —
    /// `None` = generic preset firmware.
    #[serde(default)]
    pub firmware_project_id: Option<i64>,
}

/// Body of `POST /api/v1/devices`.
///
/// When an inactive device already carries this `device_id` in the org, it
/// is reactivated (200 + `{"reactivated": true}`) instead of created.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateDevice {
    pub device_id: String,
    pub predefined_device_name: String,
    /// Board variant frozen at registration (`mcu_boards.id`); absent = the
    /// model's default board. Must exist and carry the same SoC as the
    /// model's default board.
    #[serde(default)]
    pub board_id: Option<i64>,
    /// Custom firmware chosen at provisioning (custom-firmware.md D94) —
    /// must belong to the org and target the board's chip. Absent = the
    /// generic preset firmware.
    #[serde(default)]
    pub firmware_project_id: Option<i64>,
}

/// Body of `PUT /api/v1/devices/{id}/peripherals`: screen picked by the
/// user (`null` = none, kind string = a screen of the profile); its pins are
/// reserved and the firmware define applies at the next build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateDevicePeripherals {
    /// `null` = no screen, otherwise the driver kind of the board profile.
    #[cfg_attr(feature = "openapi", schema(value_type = Option<String>))]
    pub screen: crate::boards::ScreenChoice,
}

/// Body of `PUT /api/v1/devices/{id}/board` — moves the device to another
/// board variant of the same chip (O39). Takes effect at the next build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateDeviceBoard {
    pub board_id: i64,
}

/// Answer of `POST /api/v1/devices` when a known inactive device is
/// reactivated (200 instead of 201).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceReactivated {
    pub reactivated: bool,
}

/// Answer of `PUT /api/v1/devices/{id}/board`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceBoardUpdated {
    pub board_id: i64,
    /// False when the device already ran on that board.
    pub rebuild_required: bool,
}

/// Answer of `PUT /api/v1/devices/{id}/peripherals`; takes effect at the
/// next build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DevicePeripheralsUpdated {
    /// Stored screen kind, `null` = none.
    pub screen: Option<String>,
    pub screen_kind: Option<String>,
    pub rebuild_required: bool,
}

#[cfg(test)]
mod tests {

    #[test]
    fn device_family_by_catalogue_name() {
        assert_eq!(DeviceFamily::of("generic_esp32"), DeviceFamily::Generic);
        assert_eq!(DeviceFamily::of("temp_sensor"), DeviceFamily::Predefined);
        assert_eq!(
            DeviceFamily::of("generic_esp32cam"),
            DeviceFamily::Predefined
        );
        assert_eq!(DeviceFamily::of(EDGE_AGENT_PREDEF), DeviceFamily::Agent);
        assert!(DeviceFamily::of("generic_esp8266").accepts_custom_firmware());
        assert!(!DeviceFamily::of("temp_sensor").accepts_custom_firmware());
        assert!(!DeviceFamily::of(EDGE_AGENT_PREDEF).accepts_custom_firmware());
    }

    use super::*;

    /// Exact output shape of a device (scoped by `org_id`).
    #[test]
    fn device_shape_roundtrip() {
        let json = r#"{
            "id": 42,
            "org_id": 7,
            "device_id": "4-chan-dev-shan",
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
        assert_eq!(build.build_phase, crate::builds::BuildPhase::Succeeded);
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
        // Payload without the field (old client / non-enriched backend) → None.
        let json = r#"{
            "id": 42,
            "org_id": 7,
            "device_id": "d",
            "predefined_device_name": "temp_sensor",
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
        // Minimal contract payload (docs/contracts/device.http).
        let payload: CreateDevice = serde_json::from_str(
            r#"{ "device_id": "4-chan-dev-shan", "predefined_device_name": "relay_1ch" }"#,
        )
        .unwrap();
        assert!(payload.board_id.is_none());
    }

    #[test]
    fn predefined_device_capabilities_are_names() {
        // Capabilities = list of strings.
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
        // No id in the contract.
        let back = serde_json::to_value(&pd).unwrap();
        assert!(back.get("id").is_none());
    }
}
