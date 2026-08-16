//! DTO du domaine devices — parité des contrats Django `devices/serializers.py`
//! (Phase 4), avec le scoping org (D2) à la place du `user` Django.
//!
//! Trois familles :
//! - le **catalogue global** (lecture seule, partagé entre orgs) :
//!   `predefined-devices`, `device-capabilities` ;
//! - le **registre scopé org** : `devices` (CRUD + réactivation + quotas) ;
//! - les **canaux actionneurs** : `actuator-channels` (machine à états par
//!   canal — la distribution vers les devices est le chantier M2M D13, ici on
//!   ne fait que stocker/éditer la config).
//!
//! Durcissements assumés vs Django POC (convention : contrats fonctionnels
//! conservés, pas la cosmétique) :
//! - validation des canaux sur le mode **effectif** (après défaut `binary`) —
//!   Django sautait la validation si `mode` était absent de la requête ;
//! - doublon `(device, canal)` vérifié aussi en update (Django → 500 IntegrityError).
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
    #[serde(default)]
    pub device_token: Option<DeviceTokenInfo>,
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

// ─────────────────────── Canaux actionneurs ───────────────────────

/// Mode d'un canal : tout-ou-rien, PWM ou suivi de capteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelMode {
    Binary,
    Pwm,
    Follow,
}

/// Agrégation multi-capteurs d'un canal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AggregationMethod {
    Mean,
    Max,
    Min,
    Single,
}

/// Sens de comparaison binaire : « lt » (<) / « gt » (>).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Comparison {
    Lt,
    Gt,
}

/// Comportement attendu quand la donnée capteur est périmée/absente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeMode {
    Off,
    On,
    Keep,
}

/// Canal actionneur — `GET /api/v1/actuator-channels` (liste et détail,
/// parité `ActuatorChannelConfigSerializer` + `to_representation` qui ajoute
/// `actuator_device_id` en sortie).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActuatorChannel {
    pub id: i64,
    /// `device_id` (string) du device actionneur portant ce canal.
    pub actuator_device_id: String,
    /// Numéro du canal physique (1, 2, 3, 4…).
    pub channel_number: i32,
    pub enabled: bool,
    /// Nom(s) de capteur d'entrée (séparés par des virgules si plusieurs).
    pub sensor_input_name: String,
    pub aggregation_method: AggregationMethod,
    pub mode: ChannelMode,
    // ── mode binary ──
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub comparison: Option<Comparison>,
    #[serde(default)]
    pub invert_logic: bool,
    #[serde(default)]
    pub hysteresis_seconds: i32,
    #[serde(default)]
    pub hysteresis_value: f64,
    // ── mode pwm ──
    #[serde(default)]
    pub min_sensor_value: Option<f64>,
    #[serde(default)]
    pub max_sensor_value: Option<f64>,
    #[serde(default)]
    pub min_pwm: i32,
    #[serde(default)]
    pub max_pwm: i32,
    // ── sécurité ──
    pub safe_mode: SafeMode,
    // ── horodatages (RFC 3339) ──
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

fn default_enabled() -> bool {
    true
}
fn default_hysteresis_seconds() -> i32 {
    60
}
fn default_max_pwm() -> i32 {
    255
}
fn default_single() -> AggregationMethod {
    AggregationMethod::Single
}
fn default_off() -> SafeMode {
    SafeMode::Off
}
fn default_binary() -> ChannelMode {
    ChannelMode::Binary
}

/// Corps du `POST /api/v1/actuator-channels`.
///
/// Défauts du modèle Django appliqués ici : enabled=true, aggregation=single,
/// mode=binary, hysteresis_seconds=60, hysteresis_value=0, min_pwm=0,
/// max_pwm=255, safe_mode=off. La validation mode (binary → threshold +
/// comparison, pwm → min/max sensor avec min < max) porte sur le mode
/// **effectif** — voir durcissements en tête de module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateActuatorChannel {
    /// `device_id` d'un device de type actuator/mixed dans l'org.
    pub actuator_device_id: String,
    pub channel_number: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub sensor_input_name: String,
    #[serde(default = "default_single")]
    pub aggregation_method: AggregationMethod,
    #[serde(default = "default_binary")]
    pub mode: ChannelMode,
    #[serde(default)]
    pub threshold: Option<f64>,
    #[serde(default)]
    pub comparison: Option<Comparison>,
    #[serde(default)]
    pub invert_logic: bool,
    #[serde(default = "default_hysteresis_seconds")]
    pub hysteresis_seconds: i32,
    #[serde(default)]
    pub hysteresis_value: f64,
    #[serde(default)]
    pub min_sensor_value: Option<f64>,
    #[serde(default)]
    pub max_sensor_value: Option<f64>,
    #[serde(default)]
    pub min_pwm: i32,
    #[serde(default = "default_max_pwm")]
    pub max_pwm: i32,
    #[serde(default = "default_off")]
    pub safe_mode: SafeMode,
}

/// Corps du `PUT/PATCH /api/v1/actuator-channels/{id}` — champs optionnels ;
/// absents = inchangés. `actuator_device_id` rebranche le canal sur un autre
/// device (actuator/mixed de l'org uniquement).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateActuatorChannel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actuator_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensor_input_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregation_method: Option<AggregationMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ChannelMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invert_logic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_seconds: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hysteresis_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_sensor_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sensor_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_pwm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pwm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_mode: Option<SafeMode>,
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
            "device_token": {
                "token": "tok",
                "encryption_key": "key",
                "is_active": true,
                "created": "2026-08-16T10:00:00+00:00"
            },
            "allow_dynamic_measurements": false,
            "discovered_measurements": [],
            "max_unique_measurements": 100
        }"#;
        let device: Device = serde_json::from_str(json).unwrap();
        assert_eq!(device.org_id, 7);
        assert_eq!(device.device_token.as_ref().unwrap().token, "tok");
        let back = serde_json::to_value(&device).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(json).unwrap());
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

    #[test]
    fn channel_enums_wire_format() {
        // Minuscules sur le wire, comme les choices Django.
        assert_eq!(
            serde_json::to_string(&ChannelMode::Pwm).unwrap(),
            r#""pwm""#
        );
        let agg: AggregationMethod = serde_json::from_str(r#""single""#).unwrap();
        assert_eq!(agg, AggregationMethod::Single);
        // Variante inconnue → erreur de désérialisation (400 côté API).
        assert!(serde_json::from_str::<ChannelMode>(r#""proportional""#).is_err());
    }

    #[test]
    fn create_channel_applies_model_defaults() {
        let payload: CreateActuatorChannel = serde_json::from_str(
            r#"{ "actuator_device_id": "4-chan-dev-shan", "channel_number": 1,
                "sensor_input_name": "d1:soil", "mode": "binary",
                "threshold": 40, "comparison": "lt" }"#,
        )
        .unwrap();
        assert!(payload.enabled);
        assert_eq!(payload.aggregation_method, AggregationMethod::Single);
        assert_eq!(payload.hysteresis_seconds, 60);
        assert_eq!(payload.max_pwm, 255);
        assert_eq!(payload.safe_mode, SafeMode::Off);
    }

    #[test]
    fn update_channel_empty_serializes_to_empty_object() {
        // PATCH partiel : aucun champ → {} sur le wire.
        let patch = UpdateActuatorChannel::default();
        assert_eq!(serde_json::to_string(&patch).unwrap(), "{}");
        let patch: UpdateActuatorChannel =
            serde_json::from_str(r#"{ "threshold": 12.5 }"#).unwrap();
        assert_eq!(patch.threshold, Some(12.5));
    }
}
