//! État et réducteurs purs de l'éditeur de flows — toutes les mutations du
//! graphe passent par ces fonctions pures (testées), jamais in-line dans le
//! RSX.

use pnex_core::{
    CalcConfig, CameraSourceConfig, CoolPropConfig, DebugConfig, DeviceReadConfig,
    DeviceWriteConfig, DisplayConfig, FlowGraph, FlowNode, FlowNodeKind, FlowWiring,
    FunctionNodeConfig, HttpFetchNodeConfig, InjectConfig, JsonMergeConfig, JsonSplitConfig,
    MetricConfig, NotifyNodeConfig, Position, RegPidConfig, RegTtConfig, ValueConfig,
    VideoRecordConfig,
};

/// Entrée de palette : kind + libellés i18n + couleur d'icône.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteKind {
    Inject,
    Value,
    DeviceRead,
    DeviceWrite,
    Calc,
    Metric,
    CoolProp,
    Display,
    RegTtHeat,
    RegTtCool,
    RegPid,
    PnexNotify,
    HttpFetch,
    Function,
    JsonSplit,
    JsonMerge,
    CameraSource,
    VideoRecord,
    VisionDetect,
    EventLog,
    MemoryWrite,
    MemoryRead,
    ControlSource,
    MediaSource,
    TopicClassify,
    RangeUpsert,
    Weather,
    Anomaly,
    Forecast,
    Debug,
    Red,
}

/// Crée un nœud neuf d'un kind de palette, avec sa config par défaut :
/// inject se déclenche **en continu** (`repeat_secs` 30 s — peaufiner un flow
/// demande un trafic permanent, le feed debug ne vit sinon que 5 min ;
/// violation `no_trigger` évitée d'office), device/calc/metric des configs **typées
/// mais incomplètes** (violations
/// `device_no_reads`/`calc_bad_expression`/`metric_name_missing`
/// jusqu'à la saisie — le bandeau guide l'utilisateur), red un type vide.
pub fn make_node(id: &str, kind: PaletteKind, pos: Position) -> FlowNode {
    FlowNode {
        id: id.to_string(),
        name: None,
        position: Some(pos),
        outputs: vec![],
        inputs: vec![],
        kind: match kind {
            PaletteKind::Inject => FlowNodeKind::Inject {
                config: InjectConfig {
                    repeat_secs: Some(30.0),
                    ..Default::default()
                },
            },
            PaletteKind::DeviceRead => FlowNodeKind::DeviceRead {
                config: DeviceReadConfig {
                    device_id: String::new(),
                    pins: vec![],
                    window_secs: 60.0,
                },
            },
            PaletteKind::DeviceWrite => FlowNodeKind::DeviceWrite {
                config: DeviceWriteConfig {
                    device_id: String::new(),
                    pins: vec![],
                    commands: vec![],
                },
            },
            PaletteKind::Calc => FlowNodeKind::Calc {
                config: CalcConfig {
                    expression: String::new(),
                },
            },
            // JSON-document node: ships with a usable object template so the
            // node reads as "a JSON payload source", never as a bare scalar
            // variable (user decision 2026-09-24 — the old null default was
            // too often mistaken for a variable holder).
            PaletteKind::Value => FlowNodeKind::Value {
                config: ValueConfig {
                    value: serde_json::json!({"key": "value"}),
                },
            },
            PaletteKind::Metric => FlowNodeKind::Metric {
                config: MetricConfig::default(),
            },
            // Config typée mais volontairement incomplète : les violations
            // `coolprop_*` du bandeau guident la saisie (école device/calc).
            PaletteKind::CoolProp => FlowNodeKind::CoolProp {
                config: CoolPropConfig {
                    fluid_spec: "Water".into(),
                    fluid_label: "Water".into(),
                    input1: "T".into(),
                    input2: "P".into(),
                    v1_key: "in1".into(),
                    v2_key: "in2".into(),
                    unit1: pnex_core::thermo_default_unit("T").into(),
                    unit2: pnex_core::thermo_default_unit("P").into(),
                    outputs: pnex_core::THERMO_DEFAULT_OUTPUTS
                        .iter()
                        .map(|o| o.to_string())
                        .collect(),
                    output_units: pnex_core::THERMO_DEFAULT_OUTPUTS
                        .iter()
                        .map(|o| (o.to_string(), pnex_core::thermo_default_unit(o).to_string()))
                        .collect(),
                    include_phase: true,
                },
            },
            PaletteKind::Display => FlowNodeKind::Display {
                config: DisplayConfig,
            },
            // Cartes de régulation mixtes : config typée mais volontairement
            // incomplète (device/pins vides, deadband 0) — les violations
            // `reg_*` du bandeau guident la saisie.
            PaletteKind::RegTtHeat => FlowNodeKind::RegTtHeat {
                config: RegTtConfig::default(),
            },
            PaletteKind::RegTtCool => FlowNodeKind::RegTtCool {
                config: RegTtConfig::default(),
            },
            PaletteKind::RegPid => FlowNodeKind::RegPid {
                config: RegPidConfig::default(),
            },
            // Config typée mais volontairement incomplète : les violations
            // `notify_no_*` du bandeau guident la saisie (école device/calc).
            PaletteKind::PnexNotify => FlowNodeKind::PnexNotify {
                config: NotifyNodeConfig::default(),
            },
            // Config typée mais volontairement incomplète : la violation
            // `http_fetch_url_missing` du bandeau guide la saisie (école
            // device/calc/notify).
            PaletteKind::HttpFetch => FlowNodeKind::HttpFetch {
                config: HttpFetchNodeConfig::default(),
            },
            // Config typée mais volontairement incomplète : la violation
            // `fn_not_selected` du bandeau guide la saisie (école device/calc).
            PaletteKind::Function => FlowNodeKind::PnexFunction {
                config: FunctionNodeConfig::default(),
            },
            // Découpe structurelle : aucune config, le nœud est utile d'office.
            PaletteKind::JsonSplit => FlowNodeKind::JsonSplit {
                config: JsonSplitConfig::default(),
            },
            // Clé par défaut posée : le nœud est utile d'office (la saisie
            // n'est requise que pour un payload scalaire sans topic).
            PaletteKind::JsonMerge => FlowNodeKind::JsonMerge {
                config: JsonMergeConfig {
                    default_key: "value".into(),
                    inputs: Vec::new(),
                },
            },
            // Camera left empty on purpose: the `camera_device_missing`
            // violation in the banner guides the selection.
            PaletteKind::CameraSource => FlowNodeKind::CameraSource {
                config: CameraSourceConfig::default(),
            },
            // Usable as dropped: 60 s segments, 7-day retention.
            PaletteKind::VideoRecord => FlowNodeKind::VideoRecord {
                config: VideoRecordConfig::default(),
            },
            // Model left empty on purpose: the `vision_model_missing`
            // violation in the banner guides the pick; 1 fps inference.
            PaletteKind::VisionDetect => FlowNodeKind::VisionDetect {
                config: pnex_core::vision::VisionDetectConfig {
                    model_id: String::new(),
                    labels: Vec::new(),
                    min_score: 0.0,
                    emit: pnex_core::vision::VisionEmit::OnDetection,
                    max_fps: 1.0,
                    record_layer: true,
                },
            },
            // Usable as dropped: `ev_events` stream, info level.
            PaletteKind::EventLog => FlowNodeKind::EventLog {
                config: pnex_core::events::EventLogConfig::default(),
            },
            // Key left empty on purpose: the violation banner guides the user.
            PaletteKind::MemoryWrite => FlowNodeKind::MemoryWrite {
                config: pnex_core::memory::MemoryWriteConfig::default(),
            },
            PaletteKind::MemoryRead => FlowNodeKind::MemoryRead {
                config: pnex_core::memory::MemoryReadConfig::default(),
            },
            // No control picked yet: the violation banner guides the user.
            PaletteKind::ControlSource => FlowNodeKind::ControlSource {
                config: pnex_core::ui_control::ControlSourceConfig::default(),
            },
            // No stream picked yet: the violation banner guides the user.
            PaletteKind::MediaSource => FlowNodeKind::MediaSource {
                config: pnex_core::media_ingest::MediaSourceConfig::default(),
            },
            // No taxonomy picked yet: the violation banner guides the user.
            PaletteKind::TopicClassify => FlowNodeKind::TopicClassify {
                config: pnex_core::taxonomy::TopicClassifyConfig::default(),
            },
            // No stream picked yet: the violation banner guides the user.
            PaletteKind::RangeUpsert => FlowNodeKind::RangeUpsert {
                config: pnex_core::time_range::RangeUpsertConfig::default(),
            },
            // Usable as dropped: Paris, MET Norway, every 30 min.
            PaletteKind::Weather => FlowNodeKind::Weather {
                config: pnex_core::weather::WeatherConfig::default(),
            },
            // Usable as dropped: robust z-score, 200-sample window.
            PaletteKind::Anomaly => FlowNodeKind::Anomaly {
                config: pnex_core::predictive::AnomalyConfig::default(),
            },
            // Usable as dropped: ETS, 48-step horizon, no threshold yet.
            PaletteKind::Forecast => FlowNodeKind::Forecast {
                config: pnex_core::predictive::ForecastConfig::default(),
            },
            PaletteKind::Debug => FlowNodeKind::Debug {
                config: DebugConfig::default(),
            },
            PaletteKind::Red => FlowNodeKind::Red {
                type_name: String::new(),
                config: Default::default(),
            },
        },
    }
}

/// Nombre de ports de sortie **canvas** d'un nœud : `device-read` expose un
/// port par pin + le port « nom du device » (payload = slug), la fonction un
/// port par sortie déclarée (min 1, parité runtime). (Le moteur n'a qu'une
/// entrée par nœud : les ancres d'entrée device-write/fonction sont
/// cosmétiques, hors de ce compteur.)
/// After a save, adopts the vault references the server gave to the
/// secret values typed in `current` (secrets.md S5): the editor no longer
/// holds the plaintext, and the graph matches the saved one again. Only
/// `Value` slots whose node/field the server turned into a reference
/// change; anything edited meanwhile is kept.
pub fn adopt_stored_secrets(current: &mut FlowGraph, stored: &FlowGraph) {
    for node in &mut current.nodes {
        let FlowNodeKind::HttpFetch { config } = &mut node.kind else {
            continue;
        };
        let Some(FlowNodeKind::HttpFetch { config: saved }) = stored
            .nodes
            .iter()
            .find(|n| n.id == node.id)
            .map(|n| &n.kind)
        else {
            continue;
        };
        let saved_slots = saved.secret_slots();
        for (field, slot) in config.secret_slots_mut() {
            if !matches!(slot, pnex_core::SecretSlot::Value(_)) {
                continue;
            }
            if let Some((_, pnex_core::SecretSlot::Ref(id))) =
                saved_slots.iter().find(|(f, _)| *f == field)
            {
                *slot = pnex_core::SecretSlot::Ref(*id);
            }
        }
    }
}

pub fn output_count_of(node: &FlowNode) -> usize {
    super::geometry::port_counts_of(node).0
}

/// Opaque rewire label of the device-name port (last port of device-read /
/// device-write). Internal only — never displayed; both sides of a pin
/// change append it so a wire on the last port always survives.
const DEVICE_NAME_PORT_LABEL: &str = "device-name";

/// Rewires the output ports of a `device-read` after a pin change: a wire
/// follows **its port label** (pin or device-name port), never its index. A
/// removed pin takes its wire with it — the graph breaks visibly, never a
/// silent rebinding onto the neighboring port — while surviving pins keep
/// their wire even if their index shifted (the device-name port stays
/// wired, last port).
pub fn rewire_ports_by_label(
    graph: &mut FlowGraph,
    id: &str,
    old_pins: &[String],
    new_pins: &[String],
) {
    // Labels before/after: pins in natural order + the device-name port
    // appended — same convention as the canvas (port i = pins[i], last =
    // device name) and the runtime (fan-out port i = pins[i], last = slug).
    // The label itself is opaque here: both sides append it, a wire on the
    // last port survives every pin/device change.
    let labels = |pins: &[String]| {
        let mut labels = super::geometry::sorted_pins(pins);
        labels.push(DEVICE_NAME_PORT_LABEL.to_string());
        labels
    };
    rewire_by_label(graph, id, &labels(old_pins), &labels(new_pins));
}

/// Rewires the output ports of a `PnexFunction` after a pick/rebase changed
/// the declared outputs: a wire follows **its output name**, never its
/// index. A removed output takes its wire with it (the graph breaks
/// visibly, never a silent rebinding onto the neighboring port) while
/// surviving outputs keep their wire even if their index shifted. Empty
/// declarations → single unnamed port (`[""]` labels, see
/// `geometry::function_output_labels`) — port 0 keeps its wire.
pub fn rewire_function_outputs(
    graph: &mut FlowGraph,
    id: &str,
    old_outputs: &[pnex_core::FunctionOutput],
    new_outputs: &[pnex_core::FunctionOutput],
) {
    let labels = |outs: &[pnex_core::FunctionOutput]| super::geometry::function_output_labels(outs);
    rewire_by_label(graph, id, &labels(old_outputs), &labels(new_outputs));
}

/// Keys an upstream statically exposes to a `json-split`: a Value with a
/// static object payload (its keys) or a Merge (its declared non-empty
/// inputs — the accumulated object's keys). `None` = nothing derivable, the
/// split's keys stay untouched.
pub fn upstream_split_keys(graph: &FlowGraph, split_id: &str) -> Option<Vec<String>> {
    let upstream = graph.nodes.iter().find(|n| {
        n.outputs
            .iter()
            .any(|w| w.targets.iter().any(|t| t == split_id))
            && match &n.kind {
                FlowNodeKind::Value { config } => config.value.as_object().is_some(),
                FlowNodeKind::JsonMerge { config } => {
                    config.inputs.iter().any(|i| !i.trim().is_empty())
                }
                _ => false,
            }
    });
    match upstream.map(|n| &n.kind) {
        Some(FlowNodeKind::Value { config }) => config
            .value
            .as_object()
            .map(|o| o.keys().cloned().collect()),
        Some(FlowNodeKind::JsonMerge { config }) => Some(
            config
                .inputs
                .iter()
                .filter(|i| !i.trim().is_empty())
                .cloned()
                .collect(),
        ),
        _ => None,
    }
}

/// Auto-follow pass, run after **every** graph mutation (update_graph
/// funnel): each `json-split` with `auto` regenerates its keys — hence its
/// port count — from its upstream (see `upstream_split_keys`). Wires follow
/// labels (`rewire_split_keys`): a key that disappears takes its wire, a
/// surviving key keeps it even when indexes shift. Manual splits
/// (`auto = false`) are untouched.
pub fn sync_split_keys_auto(graph: &mut FlowGraph) {
    let updates: Vec<(String, Vec<String>)> = graph
        .nodes
        .iter()
        .filter_map(|n| {
            let FlowNodeKind::JsonSplit { config } = &n.kind else {
                return None;
            };
            if !config.auto {
                return None;
            }
            upstream_split_keys(graph, &n.id)
                .filter(|keys| *keys != config.keys)
                .map(|keys| (n.id.clone(), keys))
        })
        .collect();
    for (id, keys) in updates {
        let old = keys_in(graph, &id).to_vec();
        rewire_split_keys(graph, &id, &old, &keys);
        if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
            if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                config.keys = keys;
            }
        }
    }
}

/// Snapshot of a node's declared split keys (helper for the sync pass).
fn keys_in<'a>(graph: &'a FlowGraph, id: &str) -> &'a [String] {
    match graph.nodes.iter().find(|n| n.id == id).map(|n| &n.kind) {
        Some(FlowNodeKind::JsonSplit { config }) => &config.keys,
        _ => &[],
    }
}

/// Rewires the output ports of a `json-split` after a key change: a wire
/// follows **its key label**, never its index. A removed key takes its wire
/// with it (the graph breaks visibly, never a silent rebinding onto the
/// neighboring port) while surviving keys keep their wire even if their
/// index shifted. No declared key → single unnamed port (`[""]` labels, see
/// `geometry::split_output_labels`) — port 0 keeps its wire.
pub fn rewire_split_keys(
    graph: &mut FlowGraph,
    id: &str,
    old_keys: &[String],
    new_keys: &[String],
) {
    let labels = |keys: &[String]| super::geometry::split_output_labels(keys);
    rewire_by_label(graph, id, &labels(old_keys), &labels(new_keys));
}

/// Rewires the output ports of a CoolProp node after its outputs or phase
/// flag changed: a wire follows its output id, never its index (port 0 =
/// the all-outputs object always keeps its wire). Also renames the input
/// annotations when an input key changed.
pub fn rewire_coolprop(
    graph: &mut FlowGraph,
    id: &str,
    old: &pnex_core::CoolPropConfig,
    new: &pnex_core::CoolPropConfig,
) {
    let old_ids = super::geometry::coolprop_port_ids(old);
    let new_ids = super::geometry::coolprop_port_ids(new);
    if old_ids != new_ids {
        rewire_by_label(graph, id, &old_ids, &new_ids);
    }
    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
        for annotation in &mut node.inputs {
            if annotation.pin == old.v1_key {
                annotation.pin = new.v1_key.clone();
            } else if annotation.pin == old.v2_key {
                annotation.pin = new.v2_key.clone();
            }
        }
    }
}

/// Rewires the output ports of a `control-source` after its control list
/// changed: a wire follows its control id, never its index.
pub fn rewire_control_source(
    graph: &mut FlowGraph,
    id: &str,
    old: &[uuid::Uuid],
    new: &[uuid::Uuid],
) {
    let labels = |ids: &[uuid::Uuid]| ids.iter().map(|u| u.to_string()).collect::<Vec<_>>();
    rewire_by_label(graph, id, &labels(old), &labels(new));
}

/// Rewires the output ports of a memory-read node after its key list
/// changed: a wire follows its key, never its index (port 0 = the object
/// port always keeps its wire).
pub fn rewire_memory_read(
    graph: &mut FlowGraph,
    id: &str,
    old_keys: &[String],
    new_keys: &[String],
) {
    let ids = |keys: &[String]| super::geometry::memory_read_port_ids(keys);
    let (old_ids, new_ids) = (ids(old_keys), ids(new_keys));
    if old_ids != new_ids {
        rewire_by_label(graph, id, &old_ids, &new_ids);
    }
}

/// Prunes the input annotations of a `PnexFunction` whose declared input
/// disappeared (pick/rebase). A pruned `(from, from_port)` drops the
/// runtime wire too **only** when that source port no longer feeds any
/// other input of the node (`cut_input_wire` semantics — fan-out keeps
/// sharing it).
pub fn prune_function_inputs(
    graph: &mut FlowGraph,
    id: &str,
    new_inputs: &[pnex_core::FunctionInput],
) {
    let pruned: Vec<(String, usize)> = {
        let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        let kept: std::collections::BTreeSet<&str> =
            new_inputs.iter().map(|i| i.name.as_str()).collect();
        let mut pruned = Vec::new();
        node.inputs.retain(|w| {
            if kept.contains(w.pin.as_str()) {
                true
            } else {
                pruned.push((w.from.clone(), w.from_port));
                false
            }
        });
        pruned
    };
    for (from, from_port) in pruned {
        // Runtime wire dropped only if this source port feeds no other
        // annotation of the target (mirror of cut_input_wire's keep-rule).
        let still_fed = graph.nodes.iter().any(|n| {
            n.id == id
                && n.inputs
                    .iter()
                    .any(|w| w.from == from && w.from_port == from_port)
        });
        if !still_fed {
            remove_target(graph, &from, from_port, id);
        }
    }
}

/// Prunes the input annotations of a `PnexNotify` whose template var
/// disappeared (re-pick after a template edit). A pruned `(from, from_port)`
/// drops the runtime wire too **only** when that source port no longer feeds
/// any other var of the node (`cut_input_wire` semantics — fan-out keeps
/// sharing it).
pub fn prune_notify_var_anchors(graph: &mut FlowGraph, id: &str, new_vars: &[String]) {
    let pruned: Vec<(String, usize)> = {
        let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        let kept: std::collections::BTreeSet<&str> = new_vars.iter().map(String::as_str).collect();
        let mut pruned = Vec::new();
        node.inputs.retain(|w| {
            // The permanent `trigger` gate row survives template re-picks.
            if kept.contains(w.pin.as_str()) || w.pin == pnex_core::NOTIFY_TRIGGER_PIN {
                true
            } else {
                pruned.push((w.from.clone(), w.from_port));
                false
            }
        });
        pruned
    };
    for (from, from_port) in pruned {
        // Runtime wire dropped only if this source port feeds no other
        // annotation of the target (mirror of cut_input_wire's keep-rule).
        let still_fed = graph.nodes.iter().any(|n| {
            n.id == id
                && n.inputs
                    .iter()
                    .any(|w| w.from == from && w.from_port == from_port)
        });
        if !still_fed {
            remove_target(graph, &from, from_port, id);
        }
    }
}

/// Prunes the input annotations of a `json-merge` whose named row
/// disappeared (row removal). A pruned `(from, from_port)` drops the
/// runtime wire too **only** when that source port no longer feeds any
/// other input of the node (`cut_input_wire` semantics — fan-out keeps
/// sharing it).
pub fn prune_merge_inputs(graph: &mut FlowGraph, id: &str, new_inputs: &[String]) {
    let pruned: Vec<(String, usize)> = {
        let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) else {
            return;
        };
        let kept: std::collections::BTreeSet<&str> =
            new_inputs.iter().map(String::as_str).collect();
        let mut pruned = Vec::new();
        node.inputs.retain(|w| {
            if kept.contains(w.pin.as_str()) {
                true
            } else {
                pruned.push((w.from.clone(), w.from_port));
                false
            }
        });
        pruned
    };
    for (from, from_port) in pruned {
        // Runtime wire dropped only if this source port feeds no other
        // annotation of the target (mirror of cut_input_wire's keep-rule).
        let still_fed = graph.nodes.iter().any(|n| {
            n.id == id
                && n.inputs
                    .iter()
                    .any(|w| w.from == from && w.from_port == from_port)
        });
        if !still_fed {
            remove_target(graph, &from, from_port, id);
        }
    }
}

/// Row a forward drop should claim on `target_id`: the nearest row to
/// `cursor_y` that is **free or already owned by `from_id`** — two sources
/// dropped one after the other land on two different rows (predictable
/// wiring, no silent steal). A full board falls back to the plain nearest
/// row (explicit takeover).
pub fn claim_input_row(
    graph: &FlowGraph,
    target_id: &str,
    from_id: &str,
    cursor_y: f64,
) -> Option<String> {
    let node = graph.nodes.iter().find(|n| n.id == target_id)?;
    let labels = super::geometry::input_row_labels(node);
    if labels.is_empty() {
        return None;
    }
    let pos = node.position?;
    let rows = super::geometry::input_anchor_rows(node, super::geometry::node_height_for(node));
    let owned_by_other = |label: &str| {
        node.inputs
            .iter()
            .any(|a| a.pin == label && a.from != from_id)
    };
    let nearest = |candidates: &[usize]| {
        candidates
            .iter()
            .copied()
            .min_by(|a, b| {
                (pos.y + rows[*a] - cursor_y)
                    .abs()
                    .total_cmp(&(pos.y + rows[*b] - cursor_y).abs())
            })
            .map(|i| labels[i].clone())
    };
    let free: Vec<usize> = (0..labels.len())
        .filter(|i| {
            // Une ligne sans nom n'est jamais réclamable (le nom EST la clé
            // de routage — l'inspecteur ne crée plus de lignes vides).
            !labels[*i].trim().is_empty() && !owned_by_other(&labels[*i])
        })
        .collect();
    let wired: Vec<usize> = (0..labels.len())
        .filter(|i| !labels[*i].trim().is_empty())
        .collect();
    nearest(&free).or_else(|| nearest(&wired))
}

/// Renames the input row of a `json-merge` (`old_name` → `new_name`): the
/// wire annotation on that row follows the rename — same wire, same source,
/// new `pin`. Per-row sync (positional, length unchanged): the inspector
/// calls this for `on_change`, `prune_merge_inputs` for `on_remove`.
pub fn rename_merge_input(graph: &mut FlowGraph, id: &str, old_name: &str, new_name: &str) {
    if old_name == new_name {
        return;
    }
    let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) else {
        return;
    };
    for w in &mut node.inputs {
        if w.pin == old_name {
            w.pin = new_name.to_string();
        }
    }
}

/// Shared label-following rewiring of the output ports of node `id` — the
/// wire on old port `i` moves to the new port carrying the same label; a
/// label that disappeared takes its wire with it. Device-read (pins +
/// device-name port) and function nodes (output names) share this
/// convention: the canvas and the runtime index ports identically.
fn rewire_by_label(graph: &mut FlowGraph, id: &str, old_labels: &[String], new_labels: &[String]) {
    let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) else {
        return;
    };
    let new_outputs: Vec<FlowWiring> = new_labels
        .iter()
        .enumerate()
        .filter_map(|(port, label)| {
            let targets = old_labels
                .iter()
                .position(|l| l == label)
                .and_then(|old_port| node.outputs.iter().find(|w| w.port == old_port))
                .map(|w| w.targets.clone())
                .unwrap_or_default();
            (!targets.is_empty()).then_some(FlowWiring { port, targets })
        })
        .collect();
    node.outputs = new_outputs;
}

/// Prochain id libre : `n{max(suffixes numériques)+1}` — jamais de collision
/// même après plusieurs ajouts/suppressions.
pub fn next_node_id(graph: &FlowGraph) -> String {
    let mut max = 0u32;
    for node in &graph.nodes {
        if let Some(n) = node.id.strip_prefix('n') {
            if let Ok(value) = n.parse::<u32>() {
                max = max.max(value);
            }
        }
    }
    format!("n{}", max + 1)
}

fn wiring_mut(outputs: &mut Vec<FlowWiring>, port: usize) -> &mut FlowWiring {
    match outputs.iter().position(|w| w.port == port) {
        Some(index) => &mut outputs[index],
        None => {
            outputs.push(FlowWiring {
                port,
                targets: vec![],
            });
            outputs.last_mut().expect("wiring poussé")
        }
    }
}

/// Déplace un nœud (drag) — no-op si l'id est inconnu.
pub fn move_node(graph: &mut FlowGraph, id: &str, pos: Position) {
    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
        node.position = Some(pos);
    }
}

/// Wires `(from, port) → to` — idempotent. Fan-in stays allowed: on a
/// device-write target the input rows are disambiguated by the
/// `FlowInputWiring` annotations (`set_input_target`), one per pin row.
pub fn add_target(graph: &mut FlowGraph, from: &str, port: usize, to: &str) {
    let Some(source) = graph.nodes.iter_mut().find(|n| n.id == from) else {
        return;
    };
    let targets = &mut wiring_mut(&mut source.outputs, port).targets;
    if !targets.iter().any(|t| t == to) {
        targets.push(to.to_string());
    }
}

/// Removes the wire `(from, port) → to` (if it exists).
pub fn remove_target(graph: &mut FlowGraph, from: &str, port: usize, to: &str) {
    if let Some(source) = graph.nodes.iter_mut().find(|n| n.id == from) {
        for w in &mut source.outputs {
            if w.port == port {
                w.targets.retain(|t| t != to);
            }
        }
        source.outputs.retain(|w| !w.targets.is_empty());
    }
}

/// Records the input row a wire lands on for a device-write/function/notify
/// target. A source may feed **several** rows (fan-out, e.g. one value → two
/// function inputs) but a row receives **one** wire: re-dropping on an
/// occupied row takes it over.
pub fn set_input_target(
    graph: &mut FlowGraph,
    from: &str,
    from_port: usize,
    to: &str,
    pin: String,
) {
    let Some(target) = graph.nodes.iter_mut().find(|n| n.id == to) else {
        return;
    };
    if !matches!(
        &target.kind,
        FlowNodeKind::DeviceWrite { .. }
            | FlowNodeKind::PnexFunction { .. }
            | FlowNodeKind::PnexNotify { .. }
            | FlowNodeKind::JsonMerge { .. }
    ) {
        return;
    }
    // Notify: only a stamped template var or the permanent `trigger` gate
    // row is a valid anchor.
    if let FlowNodeKind::PnexNotify { config } = &target.kind {
        if pin != pnex_core::NOTIFY_TRIGGER_PIN && !config.template_vars.iter().any(|v| *v == pin) {
            return;
        }
    }
    // Json-merge : seule une entrée déclarée est une ancre valide.
    if let FlowNodeKind::JsonMerge { config } = &target.kind {
        if !config.inputs.iter().any(|v| *v == pin) {
            return;
        }
    }
    let inputs = &mut target.inputs;
    // One wire per row: the dropped row is taken over by this source.
    inputs.retain(|w| w.pin != pin);
    inputs.push(pnex_core::FlowInputWiring {
        pin,
        from: from.to_string(),
        from_port,
    });
}

/// Cuts the annotated wire `(from, from_port) → to` on `pin`: drops the
/// annotation, and removes the runtime wire only when this source port no
/// longer feeds any other pin of the target (fan-out keeps sharing it).
pub fn cut_input_wire(graph: &mut FlowGraph, from: &str, from_port: usize, to: &str, pin: &str) {
    let keep_runtime_wire = {
        let Some(target) = graph.nodes.iter_mut().find(|n| n.id == to) else {
            return;
        };
        target.inputs.retain(|w| !(w.from == from && w.pin == pin));
        target
            .inputs
            .iter()
            .any(|w| w.from == from && w.from_port == from_port)
    };
    if !keep_runtime_wire {
        remove_target(graph, from, from_port, to);
    }
}

/// Deletes a node **and** every wire pointing at it (outputs targets and
/// input annotations alike).
pub fn remove_node(graph: &mut FlowGraph, id: &str) {
    graph.nodes.retain(|n| n.id != id);
    for node in &mut graph.nodes {
        for w in &mut node.outputs {
            w.targets.retain(|t| t != id);
        }
        node.outputs.retain(|w| !w.targets.is_empty());
        node.inputs.retain(|w| w.from != id);
    }
}

#[cfg(test)]
mod tests;
