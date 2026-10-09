//! Per-kind node configuration types: default configs, pin/reg extractors
//! and the notify/HTTP-fetch config family.
//!
//! (Extracted verbatim from the former single-file `flow.rs` — comments
//! below are kept as written at the time of the move.)

use super::validate::pin_key;
use super::*;
use crate::proto::SafeState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Position 2D d'un nœud sur le canevas de l'éditeur (métadonnée, ignorée du runtime).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// Configuration du nœud `inject` (déclencheur intervalle/cron EdgeLinkd).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InjectConfig {
    /// Intervalle de répétition en secondes (Node-RED `repeat`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_secs: Option<f64>,
    /// Expression cron (Node-RED `crontab`, 5 ou 6 champs).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron: String,
    /// Injection unique après délai (Node-RED `once` + `onceDelay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once_delay_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Payload injecté (valeur JSON quelconque ; `null` = timestamp).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
}

/// Configuration du nœud custom `pnex-device-read` — lecture des **dernières
/// valeurs** des pins d'**un seul device** (cohérence physique : 1 nœud =
/// 1 device). La lecture passe par OpenObserve (PromQL `last_over_time` sur
/// la même série que l'ingestion) : une seule source de vérité, cohérente
/// avec le dashboard/Visualisation. Aucune coordonnée d'accès ici — l'org O2
/// réelle est estampillée dans l'artefact au deploy (`pnex_o2_org`) et les
/// creds viennent de l'env du runtime. Ports de sortie : un par pin (payload
/// = valeur brute du pin, `null` si la fenêtre est vide) + un port final
/// « nom du device » (payload = slug du device).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceReadConfig {
    /// Slug du device (dimension `device_id` des séries O2).
    pub device_id: String,
    /// Labels des pins lues, dans l'ordre des ports (le port « nom du
    /// device » est ajouté après le dernier). La série O2 est le nom
    /// normalisé, calculé par le runtime via `normalize_measurement_name`.
    #[serde(default)]
    pub pins: Vec<String>,
    /// Fenêtre de fraîcheur (secondes) : la dernière valeur dans la fenêtre
    /// est renvoyée, au-delà la clé est omise du payload. 1..=3600.
    #[serde(default = "default_window_secs")]
    pub window_secs: f64,
}

fn default_window_secs() -> f64 {
    60.0
}

/// Configuration for the `pnex-device-write` custom node — writes the
/// **output** pins of a **single device** (digital 1/0, pwm duty 0..=100).
/// Incoming payload = `{pin: value}` map (batch path) or a scalar routed by
/// `msg.topic` (wire drawn on the pin's input anchor). Downlink goes through
/// the backend (`device-write` internal route, dedicated token): the runtime
/// never touches the device WS directly.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceWriteConfig {
    /// Slug du device cible.
    pub device_id: String,
    /// Labels of the written pins, in visual-anchor order. Two write paths:
    /// an object payload is the batch path (its keys are matched against
    /// these labels); a scalar payload is written to the pin named by
    /// `msg.topic` when the wire was drawn on this pin's input anchor (the
    /// deploy projection stamps the topic — see `routed_wires`), or to the
    /// sole label on a single-pin node.
    #[serde(default)]
    pub pins: Vec<String>,
    /// Commands announced by a custom firmware (`pnex.onCommand`, D88/D146),
    /// in visual-anchor order after the pins. Same two paths as the pins
    /// (object payload key, or `msg.topic` stamped by a wire on the
    /// command's anchor); the device receives `args = {"value": payload}`.
    /// Commands are not pins: they never take part in the one-write-source
    /// rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

impl DeviceWriteConfig {
    /// Input anchors of the node: pins first, then commands.
    pub fn anchors(&self) -> impl Iterator<Item = &String> {
        self.pins.iter().chain(self.commands.iter())
    }
}

/// Configuration du nœud custom `calc` — expression sur les clés du payload
/// device (variables identifiants, évaluateur [`crate::calc`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalcConfig {
    pub expression: String,
}

/// Configuration of the custom `pnex-value` node — replaces `msg.payload`
/// with a fixed JSON document (the editor ships an object template; the
/// palette name is "Json Values"). Trigger stays upstream (inject): the
/// node is a transformer, not an autonomous source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueConfig {
    /// Payload (free JSON; `null` = not set).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub value: serde_json::Value,
}

impl ValueConfig {
    /// Structural check shared by save validation and the runtime node build
    /// — `Some((code, message))` = config the runtime would reject. Single
    /// source of truth: what the editor flags is exactly what fails at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        self.value.is_null().then(|| {
            (
                "value_static_missing",
                "saisissez une valeur (champ vide)".into(),
            )
        })
    }
}

/// Configuration du nœud PNEX `json-split` — découpe structurelle du
/// payload portée par les clés déclarées :
/// - `keys` non vide : un port de sortie **nommé par clé** — `payload[key]`
///   sort sur ce port (`payload` = valeur, `topic` = clé) ; clé absente du
///   payload = port muet, clé sans port = ignorée ; array → éléments sur le
///   port 0 (`topic` = index) ; autre payload → passthrough port 0.
/// - `keys` vide (single-port mode) : un seul port, un msg par clé (`topic` = clé),
///   array → un msg par élément ; tout autre payload passe inchangé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonSplitConfig {
    /// Clés déclarées = ports de sortie nommés, dans l'ordre déclaré.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
    /// Suivi automatique de l'amont (Value statique-objet ou Merge) : les
    /// clés — donc le **nombre de ports** — sont régénérées par l'éditeur à
    /// chaque édition du graphe. `true` par défaut.
    #[serde(default = "default_split_auto")]
    pub auto: bool,
}

impl Default for JsonSplitConfig {
    fn default() -> Self {
        Self {
            keys: Vec::new(),
            auto: true,
        }
    }
}

fn default_split_auto() -> bool {
    true
}

/// Configuration du nœud PNEX `json-merge` (canvas : « Merge to JSON ») —
/// compaction : chaque payload est rangé sous `msg.topic` (si présent et non
/// vide) sinon sous la clé par défaut ; l'objet accumulé est émis à chaque
/// msg. Les entrées nommées (`inputs`) sont des lignes du canvas : un fil
/// qui y arrive est estampé `topic = nom` au deploy (tagger) — une entrée
/// porte **une variable**. État = contexte du nœud runtime (vit tant que le
/// flow tourne, reset au redeploy/restart).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonMergeConfig {
    /// Clé du payload scalaire sans `msg.topic`.
    #[serde(default = "default_merge_key")]
    pub default_key: String,
    /// Entrées nommées = lignes d'entrée du canvas, dans l'ordre déclaré.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
}

fn default_merge_key() -> String {
    "value".to_string()
}

impl JsonMergeConfig {
    /// Structural check shared by save validation and the runtime node build
    /// — `Some((code, message))` = config the runtime would reject. Single
    /// source of truth: what the editor flags is exactly what fails at deploy.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if self.default_key.trim().is_empty() {
            return Some((
                "merge_default_key_missing",
                "saisissez la clé par défaut (payload scalaire sans topic)".into(),
            ));
        }
        None
    }
}

/// Configuration du nœud custom `metric` — écriture d'une métrique
/// OpenObserve (remote-write) : nom auto-préfixé `etl_`, labels
/// `device_id="flow_{id}"`, `pred_dev="virtual_device"`, `source_type="etl"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricConfig {
    /// Nom saisi par l'utilisateur — préfixé `etl_` et sanitisé à l'écriture
    /// (`etl_metric_name`), prévisualisé à l'identique dans l'éditeur.
    pub metric_name: String,
}

/// Configuration du nœud custom `pnex-coolprop` — propriétés
/// thermophysiques in-process (`pnex-coolprop`, CoolProp v8) : spec
/// fluide/mélange (syntaxe PropsSI : nom simple, mélange prédéfini ou
/// inline `Propane[0.5]&Ethane[0.5]`), deux propriétés d'entrée (noms
/// CoolProp : `T`, `P`, `Hmass`…) lues dans `msg.payload` (objet
/// `clé → numérique`, sortie des nœuds device/calc), et sorties calculées.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CoolPropConfig {
    /// Spec fluide/mélange (validate + figée à l'édition — D6).
    pub fluid_spec: String,
    /// Propriété d'entrée 1 (nom CoolProp, ex. `"T"`).
    pub input1: String,
    /// Propriété d'entrée 2 (nom CoolProp, ex. `"P"`).
    pub input2: String,
    /// Clé de `msg.payload` pour la valeur d'entrée 1 (ex. `"t"`).
    pub v1_key: String,
    /// Clé de `msg.payload` pour la valeur d'entrée 2 (ex. `"p"`).
    pub v2_key: String,
    /// Sorties CoolProp (`Dmolar`, `Hmolar`, `Smolar`, `T`…) — au moins une.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Ajouter `"phase"` (nom textuel PhaseSI) au payload sortant.
    #[serde(default)]
    pub include_phase: bool,
    /// Display name of the picked fluid (org mixture name or CoolProp
    /// fluid); `fluid_spec` stays the frozen PropsSI spec.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fluid_label: String,
    /// Unit of input 1 (catalogue id, see `thermo_quantities`); empty = SI.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit1: String,
    /// Unit of input 2; empty = SI.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit2: String,
    /// Unit per output id; a missing entry = SI.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub output_units: std::collections::BTreeMap<String, String>,
}

impl CoolPropConfig {
    /// Output port count: port 0 = every output as one object, then one
    /// port per output, then the phase port when enabled.
    pub fn port_count(&self) -> usize {
        1 + self.outputs.len() + usize::from(self.include_phase)
    }

    /// Unit id of an output (empty = SI).
    pub fn output_unit(&self, id: &str) -> &str {
        self.output_units.get(id).map(String::as_str).unwrap_or("")
    }
}

/// Configuration du nœud `debug` (capture de la sortie d'un pipeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugConfig {
    /// Node-RED `active` — capture activée (défaut : oui).
    #[serde(default = "default_true")]
    pub active: bool,
    /// Propriété capturée (`"payload"` par défaut, `"true"` = message entier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<String>,
    /// Recopie également sur la console du runtime.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub console: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            active: true,
            complete: None,
            console: false,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Configuration du nœud custom `pnex-display` (sonde) — passthrough +
/// publication au panneau de debug. **Aucun champ saisi** : l'identité
/// (`pnex_node_id`, flow, version) est estampillée par la projection au
/// deploy — jamais lue d'une config client (anti-forgery d'attribution).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig;

/// Configuration d'une carte de régulation **tout-ou-rien** — partagée par
/// les kinds `reg_tt_heat` (chauffage) et `reg_tt_cool` (clim, inversé) : le
/// kind encode le sens de l'action, la config ne le répète pas (faute de
/// config impossible).
///
/// Carte **mixte** : le capteur et la sortie appartiennent au **même
/// device** — la régulation tourne embarquée sur l'ESP (D13/D17 : aucune
/// boucle serveur), le serveur se limite à caster cette config (docs/
/// architecture/control-cards.md). Les pins sont des **labels** (authoring)
/// résolus en gpio par le backend au sync (parité `DeviceRead`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegTtConfig {
    /// Slug du device cible (la carte mixte régulée).
    pub device_id: String,
    /// Label du pin capteur (mode `digital_in`/`adc_in` — déduit du pin
    /// résolu, jamais saisi).
    pub sensor_pin: String,
    /// Label du pin actionneur (mode `digital_out`).
    pub actuator_pin: String,
    /// Consigne (unité du capteur).
    pub setpoint: f64,
    /// Demi-bande d'hystérésis valeur : ON au franchissement de
    /// `consigne ∓ deadband`, OFF au retour à la consigne. > 0.
    pub deadband: f64,
    /// Anti court-cycle : durée minimale ON (hystérésis temporelle).
    #[serde(default = "default_reg_min_on_secs")]
    pub min_on_secs: u32,
    /// Anti court-cycle : durée minimale OFF.
    #[serde(default = "default_reg_min_off_secs")]
    pub min_off_secs: u32,
    /// Période d'échantillonnage du capteur par l'ESP (200..=60_000 ms).
    #[serde(default = "default_reg_sample_ms")]
    pub sample_ms: u32,
    /// Capteur muet/illisible au-delà de cette durée → sortie safe.
    #[serde(default = "default_reg_data_timeout_secs")]
    pub data_timeout_secs: u32,
    /// État de repos de la sortie quand la régulation est arrêtée
    /// (dé-déploiement, capteur muet, boot sans config).
    #[serde(default)]
    pub safe_state: SafeState,
}

fn default_reg_min_on_secs() -> u32 {
    5
}
fn default_reg_min_off_secs() -> u32 {
    5
}
fn default_reg_sample_ms() -> u32 {
    5_000
}
fn default_reg_data_timeout_secs() -> u32 {
    30
}

impl Default for RegTtConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            sensor_pin: String::new(),
            actuator_pin: String::new(),
            setpoint: 0.0,
            deadband: 0.0,
            min_on_secs: default_reg_min_on_secs(),
            min_off_secs: default_reg_min_off_secs(),
            sample_ms: default_reg_sample_ms(),
            data_timeout_secs: default_reg_data_timeout_secs(),
            safe_state: SafeState::Low,
        }
    }
}

/// Configuration d'une carte de régulation **PID** — sortie relais
/// time-proportional (le duty % est réparti sur `cycle_time_secs`, contrat
/// mathématique : `pnex_core::control`). Même modèle mixte que
/// [`RegTtConfig`] : un seul device, régulation embarquée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegPidConfig {
    pub device_id: String,
    pub sensor_pin: String,
    pub actuator_pin: String,
    /// Consigne (unité du capteur).
    pub setpoint: f64,
    /// Gains ≥ 0 finis (l'action dérivée est calculée sur la mesure).
    pub kp: f64,
    pub ki: f64,
    pub kd: f64,
    /// Période du cycle relais time-proportional (1..=60 s).
    #[serde(default = "default_reg_cycle_time_secs")]
    pub cycle_time_secs: u32,
    #[serde(default = "default_reg_sample_ms")]
    pub sample_ms: u32,
    #[serde(default = "default_reg_data_timeout_secs")]
    pub data_timeout_secs: u32,
    #[serde(default)]
    pub safe_state: SafeState,
}

fn default_reg_cycle_time_secs() -> u32 {
    10
}

impl Default for RegPidConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            sensor_pin: String::new(),
            actuator_pin: String::new(),
            setpoint: 0.0,
            kp: 0.0,
            ki: 0.0,
            kd: 0.0,
            cycle_time_secs: default_reg_cycle_time_secs(),
            sample_ms: default_reg_sample_ms(),
            data_timeout_secs: default_reg_data_timeout_secs(),
            safe_state: SafeState::Low,
        }
    }
}

/// Carte de régulation extraite d'un graphe — forme unifiée consommée par le
/// backend (matérialisation `regulator_config` + cast `control_config`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegConfig {
    TtHeat(RegTtConfig),
    TtCool(RegTtConfig),
    Pid(RegPidConfig),
}

// Manual `Deserialize`: same school as `FlowNodeKind` — internally-tagged
// enums buffer numeric fields through serde's `Content` deserializer and
// `arbitrary_precision` (unified workspace-wide by the starlark dependency)
// turns those buffered numbers into private magic maps, breaking typed
// numeric fields. Parsing through a `serde_json::Value` keeps numbers as
// real Numbers. Runtime parse site: `regulator.rs` (card materialization —
// a parse failure would silently drop the card). Internally-tagged newtype
// variants flatten the inner struct onto the same map as the tag, so the
// config deserializes from the whole map (its derive ignores `kind`).
impl<'de> Deserialize<'de> for RegConfig {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(d)?;
        let parsed: Result<Self, String> = (|| {
            let kind = v
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "missing field `kind`".to_string())?;
            match kind {
                "tt_heat" => Ok(Self::TtHeat(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                "tt_cool" => Ok(Self::TtCool(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                "pid" => Ok(Self::Pid(
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?,
                )),
                other => Err(format!("unknown regulation card: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

impl RegConfig {
    /// Discriminant de la carte (`tt_heat` | `tt_cool` | `pid`) — la même
    /// chaîne que le champ `kind` du `ControlSpec` fil.
    pub fn kind_str(&self) -> &'static str {
        match self {
            RegConfig::TtHeat(_) => "tt_heat",
            RegConfig::TtCool(_) => "tt_cool",
            RegConfig::Pid(_) => "pid",
        }
    }

    pub fn device_id(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.device_id,
            RegConfig::Pid(c) => &c.device_id,
        }
    }

    pub fn sensor_pin(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.sensor_pin,
            RegConfig::Pid(c) => &c.sensor_pin,
        }
    }

    pub fn actuator_pin(&self) -> &str {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => &c.actuator_pin,
            RegConfig::Pid(c) => &c.actuator_pin,
        }
    }

    pub fn setpoint(&self) -> f64 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.setpoint,
            RegConfig::Pid(c) => c.setpoint,
        }
    }

    pub fn sample_ms(&self) -> u32 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.sample_ms,
            RegConfig::Pid(c) => c.sample_ms,
        }
    }

    pub fn data_timeout_secs(&self) -> u32 {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.data_timeout_secs,
            RegConfig::Pid(c) => c.data_timeout_secs,
        }
    }

    pub fn safe_state(&self) -> SafeState {
        match self {
            RegConfig::TtHeat(c) | RegConfig::TtCool(c) => c.safe_state,
            RegConfig::Pid(c) => c.safe_state,
        }
    }

    /// Clé d'unicité d'une régulation par sortie : (device, pin actionneur),
    /// labels normalisés (trim + casse ASCII ignorée) — utilisée par
    /// `validate_graph` (graphe) puis par le sync backend (tous les flows
    /// déployés).
    pub fn actuator_key(&self) -> String {
        format!(
            "{}|{}",
            self.device_id().trim(),
            self.actuator_pin().trim().to_ascii_lowercase()
        )
    }
}

/// Extrait les cartes de régulation d'un graphe : `(id nœud, config)` dans
/// l'ordre du graphe. Une seule définition, consommée par le sync backend
/// (matérialisation + cast) et par les tests.
pub fn regulator_configs_of(g: &FlowGraph) -> Vec<(String, RegConfig)> {
    g.nodes
        .iter()
        .filter_map(|n| reg_config_of(&n.kind).map(|c| (n.id.clone(), c)))
        .collect()
}

/// La carte portée par ce kind, s'il en porte une.
pub fn reg_config_of(kind: &FlowNodeKind) -> Option<RegConfig> {
    match kind {
        FlowNodeKind::RegTtHeat { config } => Some(RegConfig::TtHeat(config.clone())),
        FlowNodeKind::RegTtCool { config } => Some(RegConfig::TtCool(config.clone())),
        FlowNodeKind::RegPid { config } => Some(RegConfig::Pid(config.clone())),
        _ => None,
    }
}

/// Usage d'une pin de device par un nœud de graphe — sert au backend pour
/// qualifier l'impact d'un changement de config device (garde 409 / stop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePinUsage {
    /// Lecture (nœud `device-read` ou pin capteur d'une carte régulation).
    Read,
    /// Write (a `device-write` pin, or a regulator card's actuator pin —
    /// both drive the physical output).
    Write,
}

/// Référence (nœud, pin, usage) d'un device dans un graphe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePinRef {
    pub node_id: String,
    pub pin: String,
    pub usage: DevicePinUsage,
}

/// All pins of a device (exact slug) referenced by a graph — `device-read`/
/// `device-write` nodes (labels normalized trim+case) and BOTH pins of
/// regulator cards (sensor = Read, actuator = Write). Consumed by the
/// backend for the "device used by a deployed flow" guard (set_mode → 409
/// / stop) and for write-ownership scans (one write source per output).
pub fn device_pin_refs_of(g: &FlowGraph, device_id: &str) -> Vec<DevicePinRef> {
    let target = device_id.trim();
    let mut refs = Vec::new();
    for n in &g.nodes {
        match &n.kind {
            FlowNodeKind::DeviceRead { config } if config.device_id.trim() == target => {
                for pin in &config.pins {
                    refs.push(DevicePinRef {
                        node_id: n.id.clone(),
                        pin: pin_key(pin).into_owned(),
                        usage: DevicePinUsage::Read,
                    });
                }
            }
            FlowNodeKind::DeviceWrite { config } if config.device_id.trim() == target => {
                for pin in &config.pins {
                    refs.push(DevicePinRef {
                        node_id: n.id.clone(),
                        pin: pin_key(pin).into_owned(),
                        usage: DevicePinUsage::Write,
                    });
                }
            }
            kind => {
                if let Some(cfg) = reg_config_of(kind) {
                    if cfg.device_id().trim() == target {
                        refs.push(DevicePinRef {
                            node_id: n.id.clone(),
                            pin: pin_key(cfg.sensor_pin()).into_owned(),
                            usage: DevicePinUsage::Read,
                        });
                        refs.push(DevicePinRef {
                            node_id: n.id.clone(),
                            pin: pin_key(cfg.actuator_pin()).into_owned(),
                            usage: DevicePinUsage::Write,
                        });
                    }
                }
            }
        }
    }
    refs
}

/// A single output-pin WRITE claim of a graph — `device-write` pins and
/// regulator-card actuator pins both drive the physical output. Device slug
/// trimmed, pin label normalized (`pin_key`: trim + ASCII-lowercase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceWritePinRef {
    pub node_id: String,
    pub device_id: String,
    pub pin: String,
}

/// All output-pin write claims of a graph (no device filter) — the pure
/// primitive behind the cross-flow "one write source per output" rule
/// (deploy gate, manual write guard, pinout reservations).
pub fn device_write_pin_refs_of(g: &FlowGraph) -> Vec<DeviceWritePinRef> {
    let mut refs = Vec::new();
    for n in &g.nodes {
        match &n.kind {
            FlowNodeKind::DeviceWrite { config } => {
                for pin in &config.pins {
                    refs.push(DeviceWritePinRef {
                        node_id: n.id.clone(),
                        device_id: config.device_id.trim().to_string(),
                        pin: pin_key(pin).into_owned(),
                    });
                }
            }
            kind => {
                if let Some(cfg) = reg_config_of(kind) {
                    refs.push(DeviceWritePinRef {
                        node_id: n.id.clone(),
                        device_id: cfg.device_id().trim().to_string(),
                        pin: pin_key(cfg.actuator_pin()).into_owned(),
                    });
                }
            }
        }
    }
    refs
}

/// Org controls listened to by a graph (`control-source` nodes), deduplicated
/// and sorted: "no effect" badge, deploy existence check, delete guard.
pub fn control_refs_of(g: &FlowGraph) -> Vec<Uuid> {
    let mut ids: Vec<Uuid> = g
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            FlowNodeKind::ControlSource { config } => Some(config.controls.iter().copied()),
            _ => None,
        })
        .flatten()
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Reserved name of the notify node's permanent boolean gate input: a wire
/// annotated with this pin feeds the trigger gate (`topic = "trigger"` at
/// runtime), never a template var.
pub const NOTIFY_TRIGGER_PIN: &str = "trigger";

/// Configuration du nœud custom `pnex-notify` — rendu d'un template
/// minijinja de l'org puis envoi vers un ou plusieurs canaux (D49–D54).
/// Le nœud ne porte que des **références** : au deploy, le backend résout
/// et estampe le snapshot (`pnex_notify_channels` / `pnex_notify_template`)
/// dans l'artefact — éditer un canal/template ne mute pas les flows déjà
/// déployés (re-déployer pour propager, école « save ≠ déployé »).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NotifyNodeConfig {
    /// Canaux cibles (`notify_channels.id`) — au moins un (validate_graph).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_ids: Vec<Uuid>,
    /// Template de rendu (`notify_templates.id`) — nil = non posé.
    /// Tolérant au JSON `null`/`""` (l'éditeur émet null pour « non posé »).
    #[serde(default, deserialize_with = "deserialize_nil_uuid")]
    pub template_id: Uuid,
    /// `true` : un échec d'envoi tombe le nœud (flow_error isolé, jamais de
    /// crash-loop) ; défaut lenient : warn + passthrough (D53).
    #[serde(default)]
    pub strict: bool,
    /// Overrides du nœud — littéral **ou** expression `{{ msg.x }}`
    /// pré-rendue contre `{msg, meta}` avant le rendu du template
    /// (composition 2 niveaux, D52).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub vars: std::collections::BTreeMap<String, String>,
    /// Noms des vars du template stampés au pick (ordre du template) —
    /// source des ancres d'entrée nommées canvas et du routage deploy
    /// (tagger `topic = var`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_vars: Vec<String>,
    /// Anti-spam — fenêtre fixe « max N envois par fenêtre de D secondes » :
    /// le premier envoi ouvre la fenêtre, le surplus est bloqué jusqu'au
    /// reset (l'envoi est différé, pas perdu — le set rempli reste en attente
    /// côté runtime). `None` = pas de limite. Le passthrough n'est pas affecté.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anti_spam: Option<AntiSpamConfig>,
}

/// Règle anti-spam d'un nœud notify (compteur × durée, une seule règle
/// lisible — l'esprit low-code, pas la school Alertmanager).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct AntiSpamConfig {
    /// Envois autorisés par fenêtre — ≥ 1 (validate_graph).
    pub max_msgs: u32,
    /// Durée de la fenêtre en secondes — ≥ 1 (validate_graph).
    pub window_secs: u64,
}

/// Configuration du nœud custom `pnex-http-fetch` (C1a) — requête HTTP
/// client « type curl » : méthode GET/POST, headers, auth (basic/bearer/
/// clé dans un header), proxy configurable (direct ou provider de scraping).
/// La réponse remplace le payload ; le moteur ETL standard prend le relais.
/// Secret fields (basic/proxy passwords, bearer token, key header value)
/// are [`crate::SecretSlot`] vault references once saved (secrets.md S5).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HttpFetchHeader {
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// Méthode HTTP v1 — GET/POST couvrent le cas collecte (API JSON en
/// lecture, webhook d'ingestion d'un tiers).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpFetchMethod {
    #[default]
    Get,
    Post,
}

/// Auth de la requête — taggée `mode` (l'éditeur émet `{"mode": "basic", …}`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HttpFetchAuth {
    #[default]
    None,
    Basic {
        username: String,
        #[serde(default)]
        password: crate::SecretSlot,
    },
    Bearer {
        #[serde(default)]
        token: crate::SecretSlot,
    },
    /// Clé d'API dans un header arbitraire (`X-Api-Key: …`).
    Header {
        name: String,
        #[serde(default)]
        value: crate::SecretSlot,
    },
}

/// Proxy sortant — `custom` couvre les providers de scraping en mode proxy
/// (HTTP CONNECT + user/pass) ; l'idiome « clé dans l'URL cible »
/// (ScraperAPI/ZenRows) n'a pas besoin de proxy du tout.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum HttpFetchProxy {
    #[default]
    None,
    Custom {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<String>,
        #[serde(default, skip_serializing_if = "crate::SecretSlot::is_unset")]
        password: crate::SecretSlot,
    },
}

/// Sémantique d'erreur : `reject` → message rejeté (`flow.handle_error`,
/// flow_error isolé, le flow survit — école notify strict) ; `passthrough`
/// → payload null + `http_error` + statusCode, le graphe décide (switch…).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpFetchOnError {
    #[default]
    Reject,
    Passthrough,
}

/// `Default` **manuel** : aligné sur le défaut serde (`timeout_secs: 30`),
/// sinon le constructeur Rust produirait 0 (violation parasite côté éditeur).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpFetchNodeConfig {
    /// URL complète (query inclus) — pas de templating v1 (calcul amont
    /// par un nœud calc).
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub method: HttpFetchMethod,
    /// Headers statiques additionnels.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HttpFetchHeader>,
    #[serde(default)]
    pub auth: HttpFetchAuth,
    #[serde(default)]
    pub proxy: HttpFetchProxy,
    /// Timeout total de la requête (s) — borné 1..=300 par validate_graph.
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
    /// Body littéral (POST) — absent ⇒ payload du msg sérialisé JSON
    /// (+ `content-type: application/json`, surchargeable par header explicite).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub on_error: HttpFetchOnError,
}

fn default_http_timeout_secs() -> u64 {
    30
}

impl HttpFetchNodeConfig {
    /// Destination key of a secret field (R9): the request URL's origin
    /// for `auth.*`, the proxy's for `proxy.password`.
    pub fn secret_destination(&self, field: &str) -> String {
        match (&self.proxy, field) {
            (HttpFetchProxy::Custom { url, .. }, "proxy.password") => crate::destination_key(url),
            _ => crate::destination_key(&self.url),
        }
    }

    /// The secret fields of the node, by stable field name
    /// (`auth.password`, `auth.token`, `auth.value`, `proxy.password`).
    pub fn secret_slots(&self) -> Vec<(&'static str, &crate::SecretSlot)> {
        let mut out = Vec::new();
        match &self.auth {
            HttpFetchAuth::Basic { password, .. } => out.push(("auth.password", password)),
            HttpFetchAuth::Bearer { token } => out.push(("auth.token", token)),
            HttpFetchAuth::Header { value, .. } => out.push(("auth.value", value)),
            HttpFetchAuth::None => {}
        }
        if let HttpFetchProxy::Custom { password, .. } = &self.proxy {
            out.push(("proxy.password", password));
        }
        out
    }

    /// Mutable twin of [`Self::secret_slots`].
    pub fn secret_slots_mut(&mut self) -> Vec<(&'static str, &mut crate::SecretSlot)> {
        let mut out = Vec::new();
        match &mut self.auth {
            HttpFetchAuth::Basic { password, .. } => out.push(("auth.password", password)),
            HttpFetchAuth::Bearer { token } => out.push(("auth.token", token)),
            HttpFetchAuth::Header { value, .. } => out.push(("auth.value", value)),
            HttpFetchAuth::None => {}
        }
        if let HttpFetchProxy::Custom { password, .. } = &mut self.proxy {
            out.push(("proxy.password", password));
        }
        out
    }
}

impl Default for HttpFetchNodeConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: HttpFetchMethod::default(),
            headers: Vec::new(),
            auth: HttpFetchAuth::default(),
            proxy: HttpFetchProxy::default(),
            timeout_secs: default_http_timeout_secs(),
            body: None,
            on_error: HttpFetchOnError::default(),
        }
    }
}

/// UUID tolérant au JSON `null`/`""` (vaut nil) — l'éditeur front manipule
/// des chaînes et représente « non posé » par null.
fn deserialize_nil_uuid<'de, D>(d: D) -> Result<Uuid, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<String> = Option::deserialize(d)?;
    match v.as_deref() {
        None | Some("") => Ok(Uuid::nil()),
        Some(s) => Uuid::parse_str(s).map_err(serde::de::Error::custom),
    }
}

// ───────────────────── Camera & video nodes (D78) ─────────────────────

/// Configuration of the `camera-source` node — the first event-driven
/// source node: subscribes to the camera frame bus of one device (Valkey
/// pub/sub, fed by the backend CameraHub) and emits one message per frame.
/// Messages carry a **reference** to the JPEG (`payload.frame_key`, TTL
/// 15 s), never the bytes (camera-video.md D74).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraSourceConfig {
    /// Camera device slug.
    pub device_id: String,
    /// Sampling: at most this many messages per second (0 = every frame).
    #[serde(default)]
    pub max_fps: f64,
}

/// Upper bound of `max_fps` on camera nodes (the camera itself caps at 25).
pub const CAMERA_NODE_MAX_FPS: f64 = 25.0;

impl CameraSourceConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        if !crate::naming::valid_device_label(&self.device_id) {
            return Some(("camera_device_missing", "select a camera".into()));
        }
        if !self.max_fps.is_finite() || self.max_fps < 0.0 || self.max_fps > CAMERA_NODE_MAX_FPS {
            return Some((
                "camera_fps_invalid",
                format!("max fps must be between 0 (every frame) and {CAMERA_NODE_MAX_FPS}"),
            ));
        }
        None
    }
}

/// Configuration of the `video-record` node — accumulates the frames of
/// incoming `camera-source` messages into MJPEG-AVI segments, one buffer
/// per camera, and stores each segment through the backend MediaStore
/// (fs | s3). Flush on duration, size or frame gap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoRecordConfig {
    /// Segment duration in seconds.
    #[serde(default = "default_segment_secs")]
    pub segment_secs: u32,
    /// Segment size cap in MB (flushes early when reached).
    #[serde(default = "default_segment_mb")]
    pub max_segment_mb: u32,
    /// Flush when no frame arrived for this many seconds (camera stopped).
    #[serde(default = "default_gap_secs")]
    pub gap_secs: u32,
    /// Recording rate cap (0 = every incoming frame).
    #[serde(default)]
    pub max_fps: f64,
    /// Days before the segments are pruned (0 = keep forever).
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Logical stream name (empty = the camera slug).
    #[serde(default)]
    pub stream: String,
}

fn default_segment_secs() -> u32 {
    60
}
fn default_segment_mb() -> u32 {
    32
}
fn default_gap_secs() -> u32 {
    10
}
fn default_retention_days() -> u32 {
    7
}

impl Default for VideoRecordConfig {
    fn default() -> Self {
        Self {
            segment_secs: default_segment_secs(),
            max_segment_mb: default_segment_mb(),
            gap_secs: default_gap_secs(),
            max_fps: 0.0,
            retention_days: default_retention_days(),
            stream: String::new(),
        }
    }
}

pub const VIDEO_SEGMENT_SECS_RANGE: (u32, u32) = (5, 3600);
pub const VIDEO_SEGMENT_MB_RANGE: (u32, u32) = (1, 256);
pub const VIDEO_GAP_SECS_RANGE: (u32, u32) = (1, 600);
pub const VIDEO_RETENTION_DAYS_MAX: u32 = 3650;

impl VideoRecordConfig {
    /// Structural check shared by the save validation and the runtime build.
    pub fn check(&self) -> Option<(&'static str, String)> {
        let in_range = |v: u32, (lo, hi): (u32, u32)| (lo..=hi).contains(&v);
        if !in_range(self.segment_secs, VIDEO_SEGMENT_SECS_RANGE) {
            return Some((
                "video_segment_secs_invalid",
                format!(
                    "segment duration must be between {} and {} seconds",
                    VIDEO_SEGMENT_SECS_RANGE.0, VIDEO_SEGMENT_SECS_RANGE.1
                ),
            ));
        }
        if !in_range(self.max_segment_mb, VIDEO_SEGMENT_MB_RANGE) {
            return Some((
                "video_segment_mb_invalid",
                format!(
                    "segment size must be between {} and {} MB",
                    VIDEO_SEGMENT_MB_RANGE.0, VIDEO_SEGMENT_MB_RANGE.1
                ),
            ));
        }
        if !in_range(self.gap_secs, VIDEO_GAP_SECS_RANGE) {
            return Some((
                "video_gap_secs_invalid",
                format!(
                    "gap must be between {} and {} seconds",
                    VIDEO_GAP_SECS_RANGE.0, VIDEO_GAP_SECS_RANGE.1
                ),
            ));
        }
        if !self.max_fps.is_finite() || self.max_fps < 0.0 || self.max_fps > CAMERA_NODE_MAX_FPS {
            return Some((
                "camera_fps_invalid",
                format!("max fps must be between 0 (every frame) and {CAMERA_NODE_MAX_FPS}"),
            ));
        }
        if self.retention_days > VIDEO_RETENTION_DAYS_MAX {
            return Some((
                "video_retention_invalid",
                format!("retention must be at most {VIDEO_RETENTION_DAYS_MAX} days"),
            ));
        }
        if self.stream.chars().count() > 128 {
            return Some((
                "video_stream_too_long",
                "stream name is limited to 128 characters".into(),
            ));
        }
        None
    }
}

/// A camera recorded by a `video-record` node — one recorder per camera
/// across the org (two recorders would write the same frames twice into
/// one timeline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRecordClaim {
    pub node_id: String,
    pub device_id: String,
}

/// Recordings of a graph: for each `video-record` node, the cameras of the
/// `camera-source` nodes upstream of it (any path — through a vision node
/// for instance).
pub fn video_record_claims_of(g: &FlowGraph) -> Vec<VideoRecordClaim> {
    use std::collections::{HashMap, HashSet};
    // Reverse adjacency: target -> sources.
    let mut parents: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &g.nodes {
        for w in &n.outputs {
            for t in &w.targets {
                parents.entry(t.as_str()).or_default().push(n.id.as_str());
            }
        }
    }
    let by_id: HashMap<&str, &FlowNode> = g.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut claims = Vec::new();
    for n in &g.nodes {
        let FlowNodeKind::VideoRecord { .. } = &n.kind else {
            continue;
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![n.id.as_str()];
        let mut cameras: Vec<String> = Vec::new();
        while let Some(id) = stack.pop() {
            for &p in parents.get(id).map(Vec::as_slice).unwrap_or(&[]) {
                if !seen.insert(p) {
                    continue;
                }
                if let Some(FlowNodeKind::CameraSource { config: src }) =
                    by_id.get(p).map(|n| &n.kind)
                {
                    let cam = src.device_id.trim().to_string();
                    if !cam.is_empty() && !cameras.contains(&cam) {
                        cameras.push(cam);
                    }
                }
                stack.push(p);
            }
        }
        for cam in cameras {
            claims.push(VideoRecordClaim {
                node_id: n.id.clone(),
                device_id: cam,
            });
        }
    }
    claims
}
