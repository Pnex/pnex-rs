//! Node-RED projection: renders the typed graph as the `flows.json` entries
//! consumed by the headless EdgeLinkd runtime (one tab per flow, custom
//! nodes carry version metadata through unknown-key preservation).

use super::*;
use crate::functions::{build_js_wrapper, FunctionLanguage, FunctionResolver};
use crate::proto::SafeState;

/// Projette le graphe typé PNEX vers les entrées Node-RED du `flows.json`
/// (un tab + ses nœuds) consommées par le runtime EdgeLinkd headless.
/// L'artefact complet = concaténation des projections de tous les flows
/// déployés. Les métadonnées de version sont embarquées sur le tab et sur
/// les nœuds custom (clés inconnues préservées par le désérialiseur).
pub fn to_red_flows_json(g: &FlowGraph, meta: &FlowArtifactMeta) -> serde_json::Value {
    to_red_flows_json_with(g, meta, &FunctionResolver::new())
}

/// Projection avec résolveur de fonctions : les nœuds `PnexFunction` sont
/// inline avec le CODE de la version épinglée (wrapper JS généré pour le
/// nœud builtin `function` du vendor, code brut pour `pnex-starlark`).
/// Référence manquante → entrée `comment` (no-op runtime) — le backend
/// bloque en amont avec `function_unresolved` quand la ref n'est pas
/// résoluble (la fonction reste totale, jamais de panic).
pub fn to_red_flows_json_with(
    g: &FlowGraph,
    meta: &FlowArtifactMeta,
    fns: &FunctionResolver,
) -> serde_json::Value {
    let tab_id = flow_tab_id(meta.flow_id);
    let mut entries = vec![serde_json::json!({
        "id": tab_id,
        "type": "tab",
        "label": format!("Flow #{} v{}", meta.flow_id, meta.version_number),
        "pnex_flow_id": meta.flow_id,
        "pnex_version": meta.version_number,
        "pnex_org_id": meta.org_id,
        "pnex_o2_org": meta.o2_org,
    })];

    for n in &g.nodes {
        let mut padded_ports: Option<usize> = None;
        let (wires, taggers) = routed_wires(g, n, &tab_id, meta);
        let mut e = match &n.kind {
            FlowNodeKind::Inject { config } => inject_entry(config),
            FlowNodeKind::DeviceRead { config } => {
                // Un port par pin + le port final « nom du device » : pad les
                // wires au nombre de ports (école fonction multi-sorties).
                padded_ports = Some(config.pins.len() + 1);
                serde_json::json!({
                    "type": "pnex-device-read",
                    "device_id": config.device_id,
                    "pins": config.pins,
                    "window_secs": config.window_secs,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                    // Identifiant O2 réel (pas un schéma déduit de l'id DB) :
                    // consommé par le nœud runtime pour ses lectures.
                    "pnex_o2_org": meta.o2_org,
                })
            }
            FlowNodeKind::DeviceWrite { config } => serde_json::json!({
                "type": "pnex-device-write",
                "device_id": config.device_id,
                "pins": config.pins,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::Calc { config } => serde_json::json!({
                "type": "pnex-calc",
                "expression": config.expression,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
            }),
            FlowNodeKind::Value { config } => serde_json::json!({
                "type": "pnex-value",
                "mode": config.mode,
                "value": config.value,
                "min": config.min,
                "max": config.max,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
            }),
            FlowNodeKind::CameraSource { config } => serde_json::json!({
                "type": "pnex-camera-source",
                "device_id": config.device_id,
                "max_fps": config.max_fps,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::VideoRecord { config } => serde_json::json!({
                "type": "pnex-video-record",
                "segment_secs": config.segment_secs,
                "max_segment_mb": config.max_segment_mb,
                "gap_secs": config.gap_secs,
                "max_fps": config.max_fps,
                "retention_days": config.retention_days,
                "stream": config.stream,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::EventLog { config } => serde_json::json!({
                "type": "pnex-event-log",
                "stream": config.stream,
                "level": config.level,
                "message": config.message,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::VisionDetect { config } => serde_json::json!({
                "type": "pnex-vision-detect",
                "model_id": config.model_id,
                "labels": config.labels,
                "min_score": config.min_score,
                "emit": config.emit,
                "max_fps": config.max_fps,
                "record_layer": config.record_layer,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::MemoryWrite { config } => serde_json::json!({
                "type": "pnex-memory-write",
                "key": config.key,
                "ttl_secs": config.ttl_secs,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::MemoryRead { config } => {
                // Object port + one port per key.
                padded_ports = Some(config.port_count());
                serde_json::json!({
                    "type": "pnex-memory-read",
                    "keys": config.keys,
                    "max_age_secs": config.max_age_secs,
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::Anomaly { config } => {
                padded_ports = Some(crate::predictive::ANOMALY_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-anomaly",
                    "method": config.method,
                    "key": config.key,
                    "window": config.window,
                    "min_samples": config.min_samples,
                    "threshold": config.threshold,
                    "level": config.level,
                    "hazard": config.hazard,
                    "season_length": config.season_length,
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::Forecast { config } => {
                padded_ports = Some(crate::predictive::FORECAST_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-forecast",
                    "model": config.model,
                    "key": config.key,
                    "window": config.window,
                    "min_samples": config.min_samples,
                    "horizon": config.horizon,
                    "season_length": config.season_length,
                    "level": config.level,
                    "threshold": config.threshold,
                    "direction": config.direction,
                    "every": config.every,
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::Metric { config } => serde_json::json!({
                "type": "pnex-metric",
                "metric_name": config.metric_name,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
                "pnex_o2_org": meta.o2_org,
            }),
            FlowNodeKind::CoolProp { config } => {
                // Port 0 = all outputs, one port per output, phase port.
                padded_ports = Some(config.port_count());
                serde_json::json!({
                    "type": "pnex-coolprop",
                    "fluid_spec": config.fluid_spec,
                    "input1": config.input1,
                    "input2": config.input2,
                    "v1_key": config.v1_key,
                    "v2_key": config.v2_key,
                    "unit1": config.unit1,
                    "unit2": config.unit2,
                    "outputs": config.outputs,
                    "output_units": config.output_units,
                    "include_phase": config.include_phase,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                })
            }
            FlowNodeKind::PnexNotify { config } => serde_json::json!({
                "type": "pnex-notify",
                // Références brutes : le snapshot (canaux + template
                // résolus) est estampé au deploy par le backend
                // (`pnex_notify_channels` / `pnex_notify_template`), pas ici.
                "channel_ids": config.channel_ids,
                "template_id": config.template_id,
                "strict": config.strict,
                "vars": config.vars,
                // Vars stampées au pick — trace de câblage côté deploy
                // (drift template_changed) ; le runtime, lui, lit les vars
                // du snapshot `pnex_notify_template`.
                "template_vars": config.template_vars,
                "anti_spam": config.anti_spam,
                // Deploy-derived: true iff a wire is annotated on the
                // `trigger` gate row — the runtime gates sends on the
                // boolean arriving with `topic = "trigger"`. Derived here
                // (no UI state) so old graphs stay valid and self-heal on
                // the next deploy.
                "pnex_notify_trigger": n
                    .inputs
                    .iter()
                    .any(|w| w.pin == crate::NOTIFY_TRIGGER_PIN),
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::HttpFetch { config } => serde_json::json!({
                "type": "pnex-http-fetch",
                "url": config.url,
                "method": config.method,
                "headers": config.headers,
                "auth": config.auth,
                "proxy": config.proxy,
                "timeout_secs": config.timeout_secs,
                "body": config.body,
                "on_error": config.on_error,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::PnexFunction { config } => {
                let snap = fns.get(&(config.function_id, config.version_number));
                let out_count = snap
                    .map(|s| FunctionLanguage::output_count(&s.outputs))
                    .unwrap_or(1);
                // Multi-sorties partiellement câblé : le vendor indexe le
                // retour par port — pad les wires au nombre de ports.
                padded_ports = Some(out_count);
                match snap {
                    Some(s) => match s.language {
                        FunctionLanguage::Js => serde_json::json!({
                            "type": "function",
                            "func": build_js_wrapper(&s.code, &s.inputs, &s.outputs),
                            "outputs": out_count,
                            "pnex_node_id": n.id,
                            "pnex_flow_id": meta.flow_id,
                            "pnex_version": meta.version_number,
                            "pnex_org_id": meta.org_id,
                            "pnex_function_id": config.function_id,
                            "pnex_function_version": config.version_number,
                        }),
                        FunctionLanguage::Starlark => serde_json::json!({
                            "type": "pnex-starlark",
                            "code": s.code,
                            "inputs": s.inputs,
                            "pnex_node_id": n.id,
                            "pnex_flow_id": meta.flow_id,
                            "pnex_version": meta.version_number,
                            "pnex_org_id": meta.org_id,
                            "pnex_function_id": config.function_id,
                            "pnex_function_version": config.version_number,
                        }),
                    },
                    // Snapshot absent (fonction/version supprimée malgré le
                    // delete-guard) : le backend bloque le deploy en amont ;
                    // ici on émet un no-op pour rester total (jamais de panic).
                    None => serde_json::json!({
                        "type": "comment",
                        "name": format!(
                            "fonction introuvable : #{} v{}",
                            config.function_id, config.version_number
                        ),
                    }),
                }
            }
            FlowNodeKind::Debug { config } => serde_json::json!({
                "type": "debug",
                "active": config.active,
                "tosidebar": true,
                "console": config.console,
                "complete": config.complete.clone().unwrap_or_else(|| "payload".to_string()),
            }),
            FlowNodeKind::Display { .. } => serde_json::json!({
                "type": "pnex-display",
                // Identité estampillée par la projection : l'id canvas brut
                // ("n3") est la clé de rattachement panneau/badge, et la
                // traçabilité estampille le nœud comme les autres customs.
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::JsonSplit { config } => {
                // Un port par clé déclarée (legacy zéro clé = 1 port) : pad
                // les wires au nombre de ports (école multi-sorties).
                padded_ports = Some(config.keys.len().max(1));
                serde_json::json!({
                    "type": "function",
                    "outputs": config.keys.len().max(1),
                    "func": crate::functions::build_json_split_func(&config.keys),
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::JsonMerge { config } => serde_json::json!({
                "type": "function",
                "outputs": 1,
                "func": crate::functions::build_json_merge_func(&config.default_key),
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::RegTtHeat { config } => reg_tt_entry("pnex-reg-tt-heat", n, meta, config),
            FlowNodeKind::RegTtCool { config } => reg_tt_entry("pnex-reg-tt-cool", n, meta, config),
            FlowNodeKind::RegPid { config } => reg_pid_entry("pnex-reg-pid", n, meta, config),
            FlowNodeKind::Red { type_name, config } => {
                let mut obj = config.as_object().cloned().unwrap_or_default();
                obj.insert("type".into(), serde_json::Value::String(type_name.clone()));
                serde_json::Value::Object(obj)
            }
        };

        // Ids uniques dans l'artefact multi-flows : l'éditeur génère des ids
        // courts par flow ("n1"…) et deux flows déployés peuvent entrer en
        // collision (deux "n1" → le runtime ne retrouve plus ses nœuds et
        // refuse l'artefact entier). Le préfixe par tab rend l'id globalement
        // unique ; `pnex_node_id` garde l'id canvas brut (rattachement du
        // panneau debug) et l'attribution runtime le dé-préfixe.
        let node_id = format!("{tab_id}_{}", n.id);
        let wires = prefixed_wires(&wires, &tab_id);
        {
            let obj = e.as_object_mut().expect("entrée flows.json");
            obj.insert("id".into(), serde_json::Value::String(node_id));
            obj.insert("z".into(), serde_json::Value::String(tab_id.clone()));
            if let Some(name) = &n.name {
                obj.insert("name".into(), serde_json::Value::String(name.clone()));
            }
            if let Some(p) = n.position {
                obj.insert("x".into(), serde_json::json!(p.x));
                obj.insert("y".into(), serde_json::json!(p.y));
            }
            obj.insert("wires".into(), wires);
        }
        // Fonction multi-sorties partiellement câblée : complète le tableau
        // `wires` au nombre de ports déclarés (le vendor indexe le retour
        // par port — un tableau court ferait perdre les ports de queue).
        if let Some(min_ports) = padded_ports {
            let obj = e.as_object_mut().expect("entrée flows.json");
            let current = obj
                .get("wires")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            obj.insert("wires".into(), pad_wires(current, min_ports));
        }
        entries.push(e);
        entries.extend(taggers);
    }

    serde_json::Value::Array(entries)
}

/// Deploy-time input routing of the labeled ports of function **and notify**
/// nodes: for every wire landing on a named input anchor (a
/// `FlowInputWiring` annotation whose pin matches a declared input for a
/// `PnexFunction`, or a picked template var for a `PnexNotify`), the
/// projection retargets the wire to a generated **tagger** node that stamps
/// `msg.topic` with the anchor name and forwards to the target node — the
/// wrapper/exécuteur then feeds `inputs.<name>` from the arriving payload
/// (topic match, see `build_inputs`; the notify node fills the matching
/// template var the same way). Wires without a matching annotation
/// keep the direct wiring (legacy `payload.<name>` resolution).
///
/// Tagger ids are `t{source}p{port}_{pin}` **unprefixed** inside the wires
/// arrays (`prefixed_wires` adds the tab prefix like any node id) and
/// already prefixed in the emitted tagger entry.
fn routed_wires(
    g: &FlowGraph,
    n: &FlowNode,
    tab_id: &str,
    meta: &FlowArtifactMeta,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    // Routing table: (source id, target node id, source port) → anchor
    // name, for annotations whose pin is a declared input of the target
    // (function input, notify template var or device-write pin — same
    // tagger semantics).
    let mut routers: std::collections::BTreeMap<(String, String, usize), String> =
        std::collections::BTreeMap::new();
    for f in &g.nodes {
        let declared: Vec<&str> = match &f.kind {
            FlowNodeKind::PnexFunction { config } => {
                config.inputs.iter().map(|i| i.name.as_str()).collect()
            }
            FlowNodeKind::PnexNotify { config } => {
                let mut declared: Vec<&str> =
                    config.template_vars.iter().map(String::as_str).collect();
                // The permanent `trigger` gate row routes like a var: a wire
                // annotated `pin = "trigger"` gets a tagger stamping
                // `topic = "trigger"`.
                declared.push(crate::NOTIFY_TRIGGER_PIN);
                declared
            }
            FlowNodeKind::JsonMerge { config } => {
                config.inputs.iter().map(String::as_str).collect()
            }
            FlowNodeKind::DeviceWrite { config } => {
                config.pins.iter().map(String::as_str).collect()
            }
            // The two input rows are named after the payload keys: a wire on
            // a row is stamped `topic = key`, the node latches the value.
            FlowNodeKind::CoolProp { config } => {
                vec![config.v1_key.as_str(), config.v2_key.as_str()]
            }
            _ => continue,
        };
        for w in &f.inputs {
            if declared.contains(&w.pin.as_str()) {
                routers.insert((w.from.clone(), f.id.clone(), w.from_port), w.pin.clone());
            }
        }
    }
    let mut wires = wires_of(n);
    let mut taggers = Vec::new();
    let Some(ports) = wires.as_array_mut() else {
        return (wires, taggers);
    };
    for (port, port_targets) in ports.iter_mut().enumerate() {
        let Some(targets) = port_targets.as_array_mut() else {
            continue;
        };
        for target in targets.iter_mut() {
            let Some(target_id) = target.as_str() else {
                continue;
            };
            let Some(pin) = routers.get(&(n.id.clone(), target_id.to_string(), port)) else {
                continue;
            };
            let tagger_id = format!("t{}p{}_{}", n.id, port, pin);
            taggers.push(serde_json::json!({
                "id": format!("{tab_id}_{tagger_id}"),
                "z": tab_id,
                "type": "function",
                "func": crate::functions::build_input_tag_func(pin),
                "outputs": 1,
                // Le tagger redresse vers le nœud fonction (id d'artefact
                // préfixé, cf. préfixage des ids de nœuds plus bas).
                "wires": [[format!("{tab_id}_{target_id}")]],
                "pnex_node_id": target_id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }));
            *target = serde_json::Value::String(tagger_id);
        }
    }
    (wires, taggers)
}

/// Préfixe chaque id cible d'un tableau `wires` Node-RED avec l'id du tab
/// (miroir du préfixage des `id` de nœuds — les deux doivent rester
/// cohérents).
fn prefixed_wires(wires: &serde_json::Value, tab_id: &str) -> serde_json::Value {
    match wires {
        serde_json::Value::Array(ports) => serde_json::Value::Array(
            ports
                .iter()
                .map(|port| match port {
                    serde_json::Value::Array(targets) => serde_json::Value::Array(
                        targets
                            .iter()
                            .map(|t| match t {
                                serde_json::Value::String(s) => {
                                    serde_json::Value::String(format!("{tab_id}_{s}"))
                                }
                                other => other.clone(),
                            })
                            .collect(),
                    ),
                    other => other.clone(),
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Complète un tableau `wires` Node-RED jusqu'à `min_ports` entrées avec
/// des ports vides (`[]`) — nœud multi-sorties partiellement câblé.
fn pad_wires(v: serde_json::Value, min_ports: usize) -> serde_json::Value {
    match v {
        serde_json::Value::Array(mut ports) => {
            while ports.len() < min_ports {
                ports.push(serde_json::json!([]));
            }
            serde_json::Value::Array(ports)
        }
        other => other,
    }
}

/// Construit le tableau `wires` Node-RED : `wires[port] = [ids cibles]`.
fn wires_of(n: &FlowNode) -> serde_json::Value {
    let max_port = n
        .outputs
        .iter()
        .map(|w| w.port)
        .max()
        .map(|p| p + 1)
        .unwrap_or(0);
    let mut wires: Vec<Vec<String>> = vec![Vec::new(); max_port];
    for w in &n.outputs {
        if w.port < max_port {
            wires[w.port] = w.targets.clone();
        }
    }
    serde_json::json!(wires)
}

fn inject_entry(c: &InjectConfig) -> serde_json::Value {
    let mut e = serde_json::json!({
        "type": "inject",
        "props": [{"p": "payload"}],
    });
    let obj = e.as_object_mut().expect("inject entry");
    // EdgeLinkd hérite les props legacy (`props[].v` = valeur chaîne) : la clé
    // `payload` doit toujours exister — une chaîne vide suffit au mode `date`.
    match c.payload {
        serde_json::Value::Null => {
            obj.insert("payloadType".into(), serde_json::json!("date"));
            obj.insert("payload".into(), serde_json::json!(""));
        }
        ref p => {
            // EdgeLinkd évalue `props[].v` comme chaîne : le payload JSON est
            // encodé en chaîne (vt "json") pour être re-parsé à l'exécution.
            obj.insert("payloadType".into(), serde_json::json!("json"));
            obj.insert("payload".into(), serde_json::json!(p.to_string()));
        }
    }
    if let Some(t) = &c.topic {
        // La prop `topic` n'est émise que si un topic existe : sans `v`, le
        // désérialiseur EdgeLinkd rejetterait la valeur nulle.
        if let Some(props) = obj.get_mut("props").and_then(|p| p.as_array_mut()) {
            props.push(serde_json::json!({"p": "topic", "vt": "str", "v": t}));
        }
        obj.insert("topic".into(), serde_json::json!(t));
    }
    if let Some(r) = c.repeat_secs {
        obj.insert("repeat".into(), serde_json::json!(r));
    }
    if !c.cron.is_empty() {
        obj.insert("crontab".into(), serde_json::json!(c.cron));
    }
    if let Some(d) = c.once_delay_secs {
        obj.insert("once".into(), serde_json::json!(true));
        obj.insert("onceDelay".into(), serde_json::json!(d));
    }
    e
}

/// Champs communs d'une entrée de carte de régulation projetée : type custom
/// (`pnex-reg-*`), liaison (device + labels de pins), paramètres temporels,
/// estampillage de traçabilité (`pnex_node_id` = id canvas brut, clé du
/// `ControlSpec` fil).
#[allow(clippy::too_many_arguments)]
fn reg_common_entry(
    type_name: &str,
    n: &FlowNode,
    meta: &FlowArtifactMeta,
    device_id: &str,
    sensor_pin: &str,
    actuator_pin: &str,
    setpoint: f64,
    sample_ms: u32,
    data_timeout_secs: u32,
    safe_state: SafeState,
) -> serde_json::Value {
    serde_json::json!({
        "type": type_name,
        "device_id": device_id,
        "sensor_pin": sensor_pin,
        "actuator_pin": actuator_pin,
        "setpoint": setpoint,
        "sample_ms": sample_ms,
        "data_timeout_secs": data_timeout_secs,
        "safe_state": safe_state,
        "pnex_node_id": n.id,
        "pnex_flow_id": meta.flow_id,
        "pnex_version": meta.version_number,
        "pnex_org_id": meta.org_id,
    })
}

/// Entrée projetée d'une carte tout-ou-rien (chauffage/clim — le type encode
/// le sens) : le deadband et les anti court-cycles complètent la base.
fn reg_tt_entry(
    type_name: &str,
    n: &FlowNode,
    meta: &FlowArtifactMeta,
    c: &RegTtConfig,
) -> serde_json::Value {
    let mut e = reg_common_entry(
        type_name,
        n,
        meta,
        &c.device_id,
        &c.sensor_pin,
        &c.actuator_pin,
        c.setpoint,
        c.sample_ms,
        c.data_timeout_secs,
        c.safe_state,
    );
    let obj = e.as_object_mut().expect("entrée flows.json");
    obj.insert("deadband".into(), serde_json::json!(c.deadband));
    obj.insert("min_on_secs".into(), serde_json::json!(c.min_on_secs));
    obj.insert("min_off_secs".into(), serde_json::json!(c.min_off_secs));
    e
}

/// Entrée projetée d'une carte PID : gains + cycle relais complètent la base.
fn reg_pid_entry(
    type_name: &str,
    n: &FlowNode,
    meta: &FlowArtifactMeta,
    c: &RegPidConfig,
) -> serde_json::Value {
    let mut e = reg_common_entry(
        type_name,
        n,
        meta,
        &c.device_id,
        &c.sensor_pin,
        &c.actuator_pin,
        c.setpoint,
        c.sample_ms,
        c.data_timeout_secs,
        c.safe_state,
    );
    let obj = e.as_object_mut().expect("entrée flows.json");
    obj.insert("kp".into(), serde_json::json!(c.kp));
    obj.insert("ki".into(), serde_json::json!(c.ki));
    obj.insert("kd".into(), serde_json::json!(c.kd));
    obj.insert(
        "cycle_time_secs".into(),
        serde_json::json!(c.cycle_time_secs),
    );
    e
}
