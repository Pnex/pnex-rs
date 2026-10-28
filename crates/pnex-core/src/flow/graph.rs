//! Graph model: node kinds, wiring, node/graph root types, violations and
//! the deployed-tab identifier.
//!
//! (Extracted verbatim from the former single-file `flow.rs` — comments
//! below are kept as written at the time of the move.)

use super::*;
use crate::functions::FunctionNodeConfig;
use serde::{Deserialize, Serialize};

/// Id of the projected Node-RED "tab" — unique per flow: the EdgeLinkd runtime
/// runs a single multi-tab `flows.json`, so the projection concatenates
/// all the deployed flows of the instance.
pub fn flow_tab_id(flow_id: i64) -> String {
    format!("pnexflow{flow_id}")
}

/// Possible statuses of a flow (`flows.status` — application VARCHAR(32),
/// not a PG enum: `draft` never deployed · `deployed` running ·
/// `stopped` deliberately stopped, deployed version kept for
/// resuming · `error` reserved).
pub const FLOW_STATUS_DRAFT: &str = "draft";
pub const FLOW_STATUS_DEPLOYED: &str = "deployed";
pub const FLOW_STATUS_STOPPED: &str = "stopped";

/// Node types modeled on the PNEX side. The serde tag is `"kind"` and the config
/// of each variant is carried by a `config` field (no `flatten`
/// of a tagged enum, unsupported by serde): `{"id": "n1", "kind": "inject",
/// "config": {"repeat_secs": 5.0}}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowNodeKind {
    /// Interval/cron trigger (builtin `inject` node).
    Inject {
        #[serde(default)]
        config: InjectConfig,
    },
    /// PNEX custom node: reads the latest values of the pins of a single
    /// device via OpenObserve (same series as ingestion) — one output port
    /// per pin + a "device name" port (payload = slug).
    DeviceRead { config: DeviceReadConfig },
    /// PNEX custom node: writes the output pins of a single device
    /// (digital 1/0, pwm duty 0..=100) — payload map `{pin: value}`.
    DeviceWrite { config: DeviceWriteConfig },
    /// PNEX custom node: computation over the device payload keys.
    Calc { config: CalcConfig },
    /// PNEX custom node: fixed or random payload (min/max bounds).
    Value { config: ValueConfig },
    /// PNEX custom node: writes an OpenObserve metric
    /// (remote-write, `etl_` prefix, virtual device `flow_{id}`).
    Metric { config: MetricConfig },
    /// PNEX custom node: in-process thermophysical properties
    /// (`pnex-coolprop`) — fluid/mixture spec + 2 inputs + outputs.
    CoolProp { config: CoolPropConfig },
    /// PNEX custom node: multi-channel notification (`websocket`/`webhook`
    /// v1) rendered from an org minijinja template (D49–D54).
    PnexNotify { config: NotifyNodeConfig },
    /// PNEX custom node: configurable HTTP client request (C1a, edge-model
    /// amendment 2026-09-15) — the response replaces the payload, configurable
    /// proxy (direct or scraping provider).
    HttpFetch { config: HttpFetchNodeConfig },
    /// Output capture (builtin `debug` node).
    Debug {
        #[serde(default)]
        config: DebugConfig,
    },
    /// PNEX custom node: passthrough probe — publishes the value to the debug
    /// panel and shows it as a live badge under the node (editor).
    Display {
        #[serde(default)]
        config: DisplayConfig,
    },
    /// Mixed on/off **heating** control card: the target device
    /// reads its own sensor and drives its own output, embedded
    /// control (D13/D17) — the server casts the config, it does not regulate.
    RegTtHeat {
        #[serde(default)]
        config: RegTtConfig,
    },
    /// Mixed on/off **cooling** control card (inverted direction).
    RegTtCool {
        #[serde(default)]
        config: RegTtConfig,
    },
    /// Mixed **PID** control card, time-proportional relay output.
    RegPid {
        #[serde(default)]
        config: RegPidConfig,
    },
    /// PNEX custom node: versioned user function from the "Functions"
    /// registry (js | starlark). The node only carries **references**
    /// + the interface snapshot (declared inputs/outputs) — the CODE is
    ///   never in the graph: the projection inlines it at deploy from the
    ///   pinned version (self-contained artifact; editing a function
    ///   does not affect deployed flows until they are redeployed).
    PnexFunction { config: FunctionNodeConfig },
    /// PNEX node: JSON payload split — object → one msg per key
    /// (`payload` = value, `topic` = key), array → one msg per element
    /// (`topic` = index); other payload → pass-through.
    JsonSplit { config: JsonSplitConfig },
    /// PNEX node: compaction — object merged by keys (shallow), scalar
    /// stored under `msg.topic`, otherwise under the default key; the accumulated object
    /// is emitted on every msg (state = node context, reset on redeploy).
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
    /// Typed questions about a text → calibrated probabilities (Laya, D197).
    TextDecide {
        #[serde(default)]
        config: crate::decide::TextDecideConfig,
    },
    /// Frame or detection → caption by the org vision-language model (D195).
    VisionCaption {
        #[serde(default)]
        config: crate::vision_ai::VisionCaptionConfig,
    },
    /// Window of transcripts, events and captions → sourced description by
    /// the org LLM (D196).
    MediaFuse {
        #[serde(default)]
        config: crate::vision_ai::MediaFuseConfig,
    },
    /// A page version → PDF in the media library; AI versions wait for a
    /// person's approval (D209, D210). Port 0 ready, port 1 approval needed.
    PagePdf {
        #[serde(default)]
        config: crate::pages::PagePdfConfig,
    },
    /// Markdown of the message → a page of the org + named version (D209).
    PageWrite {
        #[serde(default)]
        config: crate::pages::PageWriteConfig,
    },
    /// Instruction + incoming data → text of the org's LLM (D208).
    LlmText {
        #[serde(default)]
        config: crate::vision_ai::LlmTextConfig,
    },
    /// Per-source object tracking over detections (D192).
    VisionTrack {
        #[serde(default)]
        config: crate::vision::VisionTrackConfig,
    },
    /// Enter / exit events of tracked objects in image zones (D193).
    VisionZone {
        #[serde(default)]
        config: crate::vision::VisionZoneConfig,
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
    /// Event source fed by the invocations of one ontology action (D183).
    ActionTrigger {
        #[serde(default)]
        config: crate::ontology::action::ActionTriggerConfig,
    },
    /// Event source fed by the new messages of mailboxes (mail.md D456).
    MailReceived {
        #[serde(default)]
        config: crate::mail::MailReceivedConfig,
    },
    /// Objects of one type matching filters (partner-watch.md D312).
    OntologyQuery {
        #[serde(default)]
        config: crate::ontology::flow::OntologyQueryConfig,
    },
    /// Creates or updates an object on its natural key (D302, D312).
    ObjectUpsert {
        #[serde(default)]
        config: crate::ontology::flow::ObjectUpsertConfig,
    },
    /// Opens, updates or closes a typed link (D312).
    LinkUpsert {
        #[serde(default)]
        config: crate::ontology::flow::LinkUpsertConfig,
    },
    /// Plays a saved site navigation rule on the server browser
    /// (site-recipes.md D327).
    SiteRecipe {
        #[serde(default)]
        config: crate::site_recipe::SiteRecipeNodeConfig,
    },
    /// Candidates of a web search on an org provider; writes nothing
    /// (web-presence.md D361).
    WebSearch {
        #[serde(default)]
        config: crate::web_search::WebSearchConfig,
    },
    /// Company name → SIREN candidates from the public register; writes
    /// nothing (account-mapping.md D332).
    OrgResolve {
        #[serde(default)]
        config: crate::org_resolve::OrgResolveConfig,
    },
    /// One look at a candidate website (page and legal notice) to identify
    /// it, never a crawl; writes nothing (web-presence.md D364).
    WebVisit {
        #[serde(default)]
        config: crate::web_visit::WebVisitConfig,
    },
    /// The server crawl of a site started by the flow, relevance rules from
    /// the message; one message per relevant page (network-check.md D407).
    SiteCrawl {
        #[serde(default)]
        config: crate::site_crawl::SiteCrawlConfig,
    },
    /// Event source of transcribed media segments (media-ingest.md D163):
    /// one message per segment (or sentence) of the listed streams.
    MediaSource {
        config: crate::media_ingest::MediaSourceConfig,
    },
    /// Topic classifier (media-ingest.md D168): tags the text of each
    /// message with the topics of one pinned taxonomy version.
    TopicClassify {
        config: crate::taxonomy::TopicClassifyConfig,
    },
    /// Time range writer (media-ingest.md D169): upserts `msg.payload` as a
    /// range of the configured scope, keyed by `external_id`.
    RangeUpsert {
        config: crate::time_range::RangeUpsertConfig,
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
                "text_decide" => Ok(Self::TextDecide {
                    config: opt_config(config)?,
                }),
                "vision_caption" => Ok(Self::VisionCaption {
                    config: opt_config(config)?,
                }),
                "media_fuse" => Ok(Self::MediaFuse {
                    config: opt_config(config)?,
                }),
                "page_pdf" => Ok(Self::PagePdf {
                    config: opt_config(config)?,
                }),
                "page_write" => Ok(Self::PageWrite {
                    config: opt_config(config)?,
                }),
                "llm_text" => Ok(Self::LlmText {
                    config: opt_config(config)?,
                }),
                "vision_track" => Ok(Self::VisionTrack {
                    config: opt_config(config)?,
                }),
                "vision_zone" => Ok(Self::VisionZone {
                    config: opt_config(config)?,
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
                "action_trigger" => Ok(Self::ActionTrigger {
                    config: opt_config(config)?,
                }),
                "mail_received" => Ok(Self::MailReceived {
                    config: opt_config(config)?,
                }),
                "ontology_query" => Ok(Self::OntologyQuery {
                    config: opt_config(config)?,
                }),
                "object_upsert" => Ok(Self::ObjectUpsert {
                    config: opt_config(config)?,
                }),
                "link_upsert" => Ok(Self::LinkUpsert {
                    config: opt_config(config)?,
                }),
                "site_recipe" => Ok(Self::SiteRecipe {
                    config: opt_config(config)?,
                }),
                "web_search" => Ok(Self::WebSearch {
                    config: opt_config(config)?,
                }),
                "org_resolve" => Ok(Self::OrgResolve {
                    config: opt_config(config)?,
                }),
                "web_visit" => Ok(Self::WebVisit {
                    config: opt_config(config)?,
                }),
                "site_crawl" => Ok(Self::SiteCrawl {
                    config: opt_config(config)?,
                }),
                "media_source" => Ok(Self::MediaSource {
                    config: req_config(config)?,
                }),
                "topic_classify" => Ok(Self::TopicClassify {
                    config: req_config(config)?,
                }),
                "range_upsert" => Ok(Self::RangeUpsert {
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
                other => Err(format!("unknown node kind: {other}")),
            }
        })();
        parsed.map_err(serde::de::Error::custom)
    }
}

/// Node kinds retired, with what replaces them (web-presence.md D359): a
/// graph holding one no longer reads, its deploy is refused with
/// `flow-node-retired`.
pub const RETIRED_KINDS: &[(&str, &str)] = &[("ingest", "import_steps")];

/// The nodes of a stored graph whose kind is retired, as deploy violations.
pub fn retired_nodes(graph: &serde_json::Value) -> Vec<FlowViolation> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let kind = n["kind"].as_str()?;
            let (_, instead) = RETIRED_KINDS.iter().find(|(k, _)| *k == kind)?;
            let id = n["id"].as_str().unwrap_or_default();
            Some(FlowViolation::with_args(
                Some(id),
                crate::err_codes::FLOW_NODE_RETIRED,
                format!("the node kind {kind} is retired: use {instead}"),
                serde_json::json!({ "node": id, "kind": kind, "instead": instead }),
            ))
        })
        .collect()
}

/// A stored graph read for display: nodes that no longer deserialize
/// (retired kind, unknown kind, broken config) are left out with the wires
/// to them, and reported as violations. Saving the result drops them.
pub fn read_graph(raw: &serde_json::Value) -> (FlowGraph, Vec<FlowViolation>) {
    let retired = retired_nodes(raw);
    let mut dropped = Vec::new();
    let mut violations = Vec::new();
    let mut nodes: Vec<FlowNode> = Vec::new();
    for n in raw["nodes"].as_array().into_iter().flatten() {
        match serde_json::from_value::<FlowNode>(n.clone()) {
            Ok(node) => nodes.push(node),
            Err(e) => {
                let id = n["id"].as_str().unwrap_or_default();
                dropped.push(id.to_owned());
                if retired.iter().all(|r| r.node_id.as_deref() != Some(id)) {
                    let kind = n["kind"].as_str().unwrap_or_default();
                    violations.push(FlowViolation::with_args(
                        Some(id),
                        crate::err_codes::FLOW_NODE_UNREADABLE,
                        format!("node {id} ({kind}) no longer reads: {e}"),
                        serde_json::json!({ "node": id, "kind": kind }),
                    ));
                }
            }
        }
    }
    for node in &mut nodes {
        for w in &mut node.outputs {
            w.targets.retain(|t| !dropped.contains(t));
        }
        node.outputs.retain(|w| !w.targets.is_empty());
        node.inputs.retain(|i| !dropped.contains(&i.from));
    }
    violations.splice(0..0, retired);
    (FlowGraph { nodes }, violations)
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

/// Wiring of an output port: `targets` = ids of the destination nodes.
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

/// Node of a PNEX flow graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// Outputs: one entry per wired port.
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

/// Graph validation violation (refused with a 400 by the API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowViolation {
    /// Offending node, when the violation is located.
    pub node_id: Option<String>,
    /// Machine code (`duplicate_node_id`, `value_static_missing`…).
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

/// Graph of a flow, as stored (JSONB) in `flow_versions`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FlowGraph {
    /// Nodes: `{id, name?, position?, outputs?, inputs?, kind, config?}`,
    /// `kind` + `config` per node type (see the flow node docs).
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<Object>))]
    pub nodes: Vec<FlowNode>,
}

/// Metadata embedded in the projected `flows.json` artifact (traceability
/// of the version actually running). `org_id` identifies the PNEX org;
/// `o2_org` is the **real** identifier of the OpenObserve org (generated by
/// provisioning, see `openobserve_orgs.o2_org`) — the Phase 6 custom nodes
/// (device/metric) use it directly, without a derived naming scheme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowArtifactMeta {
    pub flow_id: i64,
    pub version_number: i64,
    pub org_id: i64,
    /// Empty if the org has no O2 org provisioned at
    /// projection time — the device/metric nodes then degrade without crashing
    /// (warn + read/write skipped), and the reprojection that follows
    /// provisioning fills the field.
    pub o2_org: String,
}

impl FlowArtifactMeta {
    /// "Empty" meta (no deployed flow) — artifact silenced.
    pub fn empty() -> Self {
        Self {
            flow_id: 0,
            version_number: 0,
            org_id: 0,
            o2_org: String::new(),
        }
    }
}
