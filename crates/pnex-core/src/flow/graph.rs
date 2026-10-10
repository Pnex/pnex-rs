//! Graph model: node kinds, wiring, node/graph root types, violations and
//! the deployed-tab identifier.
//!
//! (Extracted verbatim from the former single-file `flow.rs` — comments
//! below are kept as written at the time of the move.)

use super::*;
use crate::functions::FunctionNodeConfig;
use serde::{Deserialize, Serialize};

/// Id du « tab » Node-RED projeté — unique par flow : le runtime EdgeLinkd
/// exécute un seul `flows.json` multi-tabs, la projection concatène donc
/// tous les flows déployés de l'instance.
pub fn flow_tab_id(flow_id: i64) -> String {
    format!("pnexflow{flow_id}")
}

/// Statuts possibles d'un flow (`flows.status` — VARCHAR(32) applicatif,
/// pas un enum PG : `draft` jamais déployé · `deployed` en exécution ·
/// `stopped` arrêté volontairement, version déployée conservée pour la
/// reprise · `error` réservé).
pub const FLOW_STATUS_DRAFT: &str = "draft";
pub const FLOW_STATUS_DEPLOYED: &str = "deployed";
pub const FLOW_STATUS_STOPPED: &str = "stopped";

/// Types de nœuds modélisés côté PNEX. Le tag serde est `"kind"` et la config
/// de chaque variante est portée par un champ `config` (pas de `flatten`
/// d'enum taggé, non supporté par serde) : `{"id": "n1", "kind": "inject",
/// "config": {"repeat_secs": 5.0}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowNodeKind {
    /// Déclencheur intervalle/cron (nœud builtin `inject`).
    Inject {
        #[serde(default)]
        config: InjectConfig,
    },
    /// Nœud custom PNEX : lecture des dernières valeurs des pins d'un seul
    /// device via OpenObserve (même série que l'ingestion) — un port de
    /// sortie par pin + un port « nom du device » (payload = slug).
    DeviceRead { config: DeviceReadConfig },
    /// Nœud custom PNEX : écriture des pins output d'un seul device
    /// (digital 1/0, pwm duty 0..=100) — payload map `{pin: valeur}`.
    DeviceWrite { config: DeviceWriteConfig },
    /// Nœud custom PNEX : calcul sur les clés du payload device.
    Calc { config: CalcConfig },
    /// Nœud custom PNEX : payload fixe ou aléatoire (bornes min/max).
    Value { config: ValueConfig },
    /// Nœud custom PNEX : écriture d'une métrique OpenObserve
    /// (remote-write, préfixe `etl_`, device virtuel `flow_{id}`).
    Metric { config: MetricConfig },
    /// Nœud custom PNEX : propriétés thermophysiques in-process
    /// (`pnex-coolprop`) — spec fluide/mélange + 2 entrées + sorties.
    CoolProp { config: CoolPropConfig },
    /// Nœud custom PNEX : notification multi-canaux (`websocket`/`webhook`
    /// v1) rendue depuis un template minijinja de l'org (D49–D54).
    PnexNotify { config: NotifyNodeConfig },
    /// Nœud custom PNEX : requête HTTP client configurable (C1a, amendement
    /// edge-model 2026-09-15) — la réponse remplace le payload, proxy
    /// configurable (direct ou provider de scraping).
    HttpFetch { config: HttpFetchNodeConfig },
    /// Capture de sortie (nœud builtin `debug`).
    Debug {
        #[serde(default)]
        config: DebugConfig,
    },
    /// Nœud custom PNEX : sonde passthrough — publie la valeur au panneau de
    /// debug et l'affiche en badge live sous le nœud (éditeur).
    Display {
        #[serde(default)]
        config: DisplayConfig,
    },
    /// Carte de régulation mixte tout-ou-rien **chauffage** : le device cible
    /// lit son propre capteur et pilote sa propre sortie, régulation
    /// embarquée (D13/D17) — le serveur caste la config, il ne régule pas.
    RegTtHeat {
        #[serde(default)]
        config: RegTtConfig,
    },
    /// Carte de régulation mixte tout-ou-rien **clim** (sens inversé).
    RegTtCool {
        #[serde(default)]
        config: RegTtConfig,
    },
    /// Carte de régulation mixte **PID**, sortie relais time-proportional.
    RegPid {
        #[serde(default)]
        config: RegPidConfig,
    },
    /// Nœud custom PNEX : fonction utilisateur versionnée du registre
    /// « Fonctions » (js | starlark). Le nœud ne porte que des **références**
    /// + le snapshot d'interface (inputs/outputs déclarés) — le CODE n'est
    ///   jamais dans le graphe : la projection l'inline au deploy depuis la
    ///   version épinglée (artefact self-contained ; éditer une fonction
    ///   n'affecte pas les flows déployés tant qu'ils ne sont pas re-déployés).
    PnexFunction { config: FunctionNodeConfig },
    /// Nœud PNEX : découpe du payload JSON — objet → un msg par clé
    /// (`payload` = valeur, `topic` = clé), array → un msg par élément
    /// (`topic` = index) ; autre payload → pass-through.
    JsonSplit { config: JsonSplitConfig },
    /// Nœud PNEX : compaction — objet fusionné par clés (shallow), scalaire
    /// rangé sous `msg.topic` sinon sous la clé par défaut ; l'objet accumulé
    /// est émis à chaque msg (état = contexte du nœud, reset au redeploy).
    JsonMerge { config: JsonMergeConfig },
    /// Event-driven source: one message per camera frame (Valkey frame bus,
    /// camera-video.md D78) — payload = frame reference, never the bytes.
    CameraSource { config: CameraSourceConfig },
    /// MJPEG-AVI segment recorder (duration/size/gap flush) → MediaStore
    /// through the backend (D78/D79).
    VideoRecord {
        #[serde(default)]
        config: VideoRecordConfig,
    },
    /// JSON event log → OpenObserve logs stream `ev_…` (D84).
    EventLog {
        #[serde(default)]
        config: crate::events::EventLogConfig,
    },
    /// Object detection on camera frames with a registry model (D83).
    VisionDetect {
        config: crate::vision::VisionDetectConfig,
    },
    /// Stores `msg.payload` in the org shared memory (Valkey) under a key,
    /// with a lifetime; passes the message through.
    MemoryWrite {
        #[serde(default)]
        config: crate::memory::MemoryWriteConfig,
    },
    /// Reads keys of the org shared memory (freshness-checked): object port
    /// then one port per key.
    MemoryRead {
        #[serde(default)]
        config: crate::memory::MemoryReadConfig,
    },
    /// Event source fed by org controls written from the surfaces
    /// (dashboards, annotations): one output port per control (D127).
    ControlSource {
        #[serde(default)]
        config: crate::ui_control::ControlSourceConfig,
    },
    /// Event source of transcribed media segments (media-ingest.md D163):
    /// one message per segment (or sentence) of the listed streams.
    MediaSource {
        config: crate::media_ingest::MediaSourceConfig,
    },
    /// Timed weather source (D140): no input, three outputs (current,
    /// daily, hourly) normalized from an allowlisted provider.
    Weather {
        #[serde(default)]
        config: crate::weather::WeatherConfig,
    },
    /// Anomaly scoring on a numeric series (robust z, forecast band,
    /// changepoint) — port 0 = detail, port 1 = boolean state.
    Anomaly {
        #[serde(default)]
        config: crate::predictive::AnomalyConfig,
    },
    /// Forecast of a numeric series (ETS/MSTL or linear trend) with an
    /// optional threshold breach ETA — ports: detail, breach, ETA seconds.
    Forecast {
        #[serde(default)]
        config: crate::predictive::ForecastConfig,
    },
    /// Échappement : nœud builtin EdgeLinkd non modélisé, config opaque.
    Red {
        /// Nom du type Node-RED cible (ex. `"change"`, `"json"`).
        type_name: String,
        /// Config Node-RED brute du nœud.
        #[serde(default)]
        config: serde_json::Value,
    },
}

// Manual `Deserialize`: internally-tagged enums buffer fields through
// serde's `Content` deserializer, which is incompatible with serde_json's
// `arbitrary_precision` feature — enabled by the starlark dependency and
// unified across every unit of a `cargo --workspace` build (resolver 2),
// so workspace-wide test builds hit it even though `-p` builds do not.
// Buffered numbers arrive as private magic maps and typed numeric fields
// fail with "invalid type: map". Parsing through a `serde_json::Value`
// keeps numbers as real Numbers, so node configs survive any feature
// unification. Same school as `AnnotationGeometry` in `pnex-core`.
impl<'de> Deserialize<'de> for FlowNodeKind {
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
            let config = v.get("config");
            match kind {
                "inject" => Ok(Self::Inject {
                    config: opt_config(config)?,
                }),
                // The `pnex_sql` node was removed: saved graphs that still
                // carry it fail to parse with an explicit message instead of
                // the generic unknown-kind error.
                "pnex_sql" => Err(
                    "node kind `pnex_sql` has been removed; delete this node from the flow"
                        .to_string(),
                ),
                "device_read" => Ok(Self::DeviceRead {
                    config: req_config(config)?,
                }),
                "device_write" => Ok(Self::DeviceWrite {
                    config: req_config(config)?,
                }),
                "calc" => Ok(Self::Calc {
                    config: req_config(config)?,
                }),
                "value" => Ok(Self::Value {
                    config: req_config(config)?,
                }),
                "metric" => Ok(Self::Metric {
                    config: req_config(config)?,
                }),
                "cool_prop" => Ok(Self::CoolProp {
                    config: req_config(config)?,
                }),
                "pnex_notify" => Ok(Self::PnexNotify {
                    config: req_config(config)?,
                }),
                "http_fetch" => Ok(Self::HttpFetch {
                    config: req_config(config)?,
                }),
                "debug" => Ok(Self::Debug {
                    config: opt_config(config)?,
                }),
                "display" => Ok(Self::Display {
                    config: opt_config(config)?,
                }),
                "reg_tt_heat" => Ok(Self::RegTtHeat {
                    config: opt_config(config)?,
                }),
                "reg_tt_cool" => Ok(Self::RegTtCool {
                    config: opt_config(config)?,
                }),
                "reg_pid" => Ok(Self::RegPid {
                    config: opt_config(config)?,
                }),
                "pnex_function" => Ok(Self::PnexFunction {
                    config: req_config(config)?,
                }),
                "json_split" => Ok(Self::JsonSplit {
                    config: req_config(config)?,
                }),
                "json_merge" => Ok(Self::JsonMerge {
                    config: req_config(config)?,
                }),
                "camera_source" => Ok(Self::CameraSource {
                    config: req_config(config)?,
                }),
                "video_record" => Ok(Self::VideoRecord {
                    config: opt_config(config)?,
                }),
                "event_log" => Ok(Self::EventLog {
                    config: opt_config(config)?,
                }),
                "vision_detect" => Ok(Self::VisionDetect {
                    config: req_config(config)?,
                }),
                "memory_write" => Ok(Self::MemoryWrite {
                    config: opt_config(config)?,
                }),
                "memory_read" => Ok(Self::MemoryRead {
                    config: opt_config(config)?,
                }),
                "control_source" => Ok(Self::ControlSource {
                    config: opt_config(config)?,
                }),
                "media_source" => Ok(Self::MediaSource {
                    config: req_config(config)?,
                }),
                "weather" => Ok(Self::Weather {
                    config: opt_config(config)?,
                }),
                "anomaly" => Ok(Self::Anomaly {
                    config: opt_config(config)?,
                }),
                "forecast" => Ok(Self::Forecast {
                    config: opt_config(config)?,
                }),
                "red" => Ok(Self::Red {
                    type_name: v
                        .get("type_name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| "missing field `type_name`".to_string())?,
                    config: config.cloned().unwrap_or(serde_json::Value::Null),
                }),
                other => Err(format!("unknown node kind: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

/// Deserialize an optional config struct; absent maps to `Default`
/// (mirrors the derived `#[serde(default)]` on the variant field).
fn opt_config<T>(raw: Option<&serde_json::Value>) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Default,
{
    match raw {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
        None => Ok(T::default()),
    }
}

/// Deserialize a required config struct (mirrors the derived behavior:
/// absence is a missing-field error).
fn req_config<T>(raw: Option<&serde_json::Value>) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    match raw {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
        None => Err("missing field `config`".to_string()),
    }
}

/// Câblage d'un port de sortie : `targets` = ids des nœuds destinataires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowWiring {
    pub port: usize,
    pub targets: Vec<String>,
}

/// Target-side wiring: which input row of a device-write (pin name), function
/// node (declared input name), notify node (template var) or json-merge node
/// (declared input name) the wire coming from `from` (port `from_port`) lands
/// on. Editor rendering: the canvas draws one wire per targeted row. Deploy:
/// an annotation whose pin matches a declared input of the target drives the
/// input routing — the projection inserts a tagger node stamping `msg.topic`
/// with that name (see `routed_wires`); the device-write runtime unit-writes
/// the pin named by the topic, the other kinds feed `inputs.<name>` from the
/// arriving payload. Kinds without declared inputs ignore these annotations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowInputWiring {
    /// Targeted row label (device-write pin, function input, notify template
    /// var or json-merge input).
    pub pin: String,
    /// Source node id.
    pub from: String,
    /// Output port index on the source node.
    pub from_port: usize,
}

/// Nœud d'un graphe de flow PNEX.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Sorties : une entrée par port câblé.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<FlowWiring>,
    /// Input-row annotations (device-write pins, function inputs, notify
    /// template vars, json-merge inputs). Read by the deploy projection
    /// (`routed_wires`) to synthesize topic taggers on wired anchors;
    /// `skip_serializing_if` keeps them out of the artifact itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<FlowInputWiring>,
    #[serde(flatten)]
    pub kind: FlowNodeKind,
}

/// Violation de validation d'un graphe (rejetée en 400 par l'API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowViolation {
    /// Nœud fautif, si la violation est localisée.
    pub node_id: Option<String>,
    /// Code machine (`duplicate_node_id`, `value_static_missing`…).
    pub code: String,
    /// Canonical English message, display fallback when the code is not
    /// registered in `err_codes::ALL` or the translation fails.
    pub message: String,
    /// Interpolation data (JSON object of strings: `device`, `pin`…)
    /// consumed by the client-side `err-<kebab>` resolution. Absent until
    /// the construction site is migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
}

impl FlowViolation {
    pub fn new(node_id: Option<&str>, code: &str, message: impl Into<String>) -> Self {
        Self {
            node_id: node_id.map(str::to_owned),
            code: code.to_owned(),
            message: message.into(),
            args: None,
        }
    }

    /// Variant carrying structured interpolation data (i18n) — `args` must
    /// be a JSON object of strings.
    pub fn with_args(
        node_id: Option<&str>,
        code: &str,
        message: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self {
            node_id: node_id.map(str::to_owned),
            code: code.to_owned(),
            message: message.into(),
            args: Some(args),
        }
    }
}

/// Graphe d'un flow — c'est ce qui est stocké (JSONB) dans `flow_versions`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlowGraph {
    pub nodes: Vec<FlowNode>,
}

/// Métadonnées embarquées dans l'artefact `flows.json` projeté (traçabilité
/// de la version réellement en exécution). `org_id` identifie l'org PNEX ;
/// `o2_org` est l'identifiant **réel** de l'org OpenObserve (généré par le
/// provisioning, cf. `openobserve_orgs.o2_org`) — les nœuds custom Phase 6
/// (device/metric) l'utilisent directement, sans schéma de nommage déduit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowArtifactMeta {
    pub flow_id: i64,
    pub version_number: i64,
    pub org_id: i64,
    /// Vide si l'org n'a pas d'org O2 provisionnée au moment de la
    /// projection — les nœuds device/metric dégradent alors sans crasher
    /// (warn + lecture/écriture sautée), et la reprojection qui suit le
    /// provisioning comble le champ.
    pub o2_org: String,
}

impl FlowArtifactMeta {
    /// Meta « à vide » (aucun flow déployé) — artefact réduit au silence.
    pub fn empty() -> Self {
        Self {
            flow_id: 0,
            version_number: 0,
            org_id: 0,
            o2_org: String::new(),
        }
    }
}
