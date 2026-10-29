//! Node-RED projection: renders the typed graph as the `flows.json` entries
//! consumed by the headless EdgeLinkd runtime (one tab per flow, custom
//! nodes carry version metadata through unknown-key preservation).

use super::*;
use crate::functions::{build_js_wrapper, FunctionLanguage, FunctionResolver};
use crate::proto::SafeState;

/// Projects the typed PNEX graph to the Node-RED entries of `flows.json`
/// (one tab + its nodes) consumed by the headless EdgeLinkd runtime.
/// The full artifact = concatenation of the projections of all deployed
/// flows. Version metadata is embedded on the tab and on
/// the custom nodes (unknown keys preserved by the deserializer).
pub fn to_red_flows_json(g: &FlowGraph, meta: &FlowArtifactMeta) -> serde_json::Value {
    to_red_flows_json_with(g, meta, &FunctionResolver::new())
}

/// Projection with a function resolver: the `PnexFunction` nodes are
/// inlined with the CODE of the pinned version (generated JS wrapper for the
/// vendor builtin `function` node, raw code for `pnex-starlark`).
/// Missing reference → `comment` entry (runtime no-op) — the backend
/// blocks upstream with `function_unresolved` when the ref cannot be
/// resolved (the function stays total, never a panic).
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
                // One port per pin + the final "device name" port: pad the
                // wires to the port count (multi-output function school).
                padded_ports = Some(config.pins.len() + 1);
                serde_json::json!({
                    "type": "pnex-device-read",
                    "device_id": config.device_id,
                    "pins": config.pins,
                    "window_secs": config.window_secs,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                    // Real O2 identifier (not a schema derived from the DB id):
                    // consumed by the runtime node for its reads.
                    "pnex_o2_org": meta.o2_org,
                })
            }
            FlowNodeKind::DeviceWrite { config } => {
                // Ports: passthrough + "device name". Without padding, a write
                // whose name port is unwired fails `fan_out_one(1)` and the
                // passthrough never reaches port 0.
                padded_ports = Some(2);
                serde_json::json!({
                "type": "pnex-device-write",
                "device_id": config.device_id,
                "pins": config.pins,
                "commands": config.commands,
                "values": config.values,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::Calc { config } => serde_json::json!({
                "type": "pnex-calc",
                "expression": config.expression,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
            }),
            FlowNodeKind::Value { config } => serde_json::json!({
                "type": "pnex-value",
                "value": config.value,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
            }),
            FlowNodeKind::CameraSource { config } => serde_json::json!({
                "type": "pnex-camera-source",
                "source": config.source,
                // Only the selected source is projected: a stream node never
                // creates device demand (D76), a device node subscribes to
                // no stream.
                "device_id": if config.source.is_device() { config.device_id.as_str() } else { "" },
                "stream": if config.source.is_device() { "" } else { config.stream.as_str() },
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
                "review_project": config.review_project,
                "review_min": config.review_min,
                "review_max": config.review_max,
                "review_every_secs": config.review_every_secs,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::TextDecide { config } => serde_json::json!({
                "type": "pnex-text-decide",
                "questions": config.questions,
                "text_field": config.effective_field(),
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::VisionCaption { config } => serde_json::json!({
                "type": "pnex-vision-caption",
                "prompt": config.effective_prompt(),
                "min_interval_secs": config.min_interval_secs,
                "crop_best": config.crop_best,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::PagePdf { config } => serde_json::json!({
                "type": "pnex-page-pdf",
                "source": config.source,
                "page_id": config.page_id,
                "require_approval": config.require_approval,
                "language": config.language,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::PageWrite { config } => serde_json::json!({
                "type": "pnex-page-write",
                "mode": config.mode,
                "title": config.title,
                "page_id": config.page_id,
                "text_field": config.text_field,
                "version_label": config.version_label,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::LlmText { config } => serde_json::json!({
                "type": "pnex-llm-text",
                "prompt": config.prompt,
                "data_field": config.data_field,
                "language": config.language,
                "max_chars": config.max_chars,
                "pnex_node_id": n.id,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::MediaFuse { config } => serde_json::json!({
                "type": "pnex-media-fuse",
                "window_secs": config.window_secs,
                "language": config.language,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::VisionTrack { config } => serde_json::json!({
                "type": "pnex-vision-track",
                "high_score": config.high_score,
                "min_iou": config.min_iou,
                "min_hits": config.min_hits,
                "lost_after_secs": config.lost_after_secs,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::VisionZone { config } => serde_json::json!({
                "type": "pnex-vision-zone",
                "zones": config.zones,
                "labels": config.labels,
                "anchor": config.anchor,
                "min_dwell_secs": config.min_dwell_secs,
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
            FlowNodeKind::ControlSource { config } => {
                // One port per listened control, in config order.
                padded_ports = Some(config.port_count());
                serde_json::json!({
                    "type": "pnex-control-source",
                    "controls": config.controls,
                    "emit_on_start": config.emit_on_start,
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            // The org stamped here is the only channel the node may read.
            FlowNodeKind::MailReceived { config } => serde_json::json!({
                "type": "pnex-mail-received",
                "mailboxes": config.mailboxes,
                "folder": config.folder,
                "from_contains": config.from_contains,
                "subject_contains": config.subject_contains,
                "skip_automatic": config.skip_automatic,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::ActionTrigger { config } => serde_json::json!({
                "type": "pnex-action-trigger",
                "object_type": config.object_type,
                "action": config.action,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::MediaSource { config } => serde_json::json!({
                "type": "pnex-media-source",
                "streams": config.streams,
                "emit": config.emit,
                "min_confidence": config.min_confidence,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            // `topics` and `taxonomy_version` are stamped by the backend
            // after projection (pinned version of the org, D168): the
            // runtime never reads the database.
            FlowNodeKind::TopicClassify { config } => serde_json::json!({
                "type": "pnex-topic-classify",
                "taxonomy_id": config.taxonomy_id,
                "version": config.version,
                "text_field": config.text_field,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            // The backend re-checks the scope against the stamped org at each
            // write (`/internal/flow/time-range`).
            FlowNodeKind::RangeUpsert { config } => serde_json::json!({
                "type": "pnex-range-upsert",
                "scope_kind": config.scope_kind,
                "scope_id": config.scope_id.trim(),
                "origin": config.origin,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            // The backend reads these configs from the deployed graph (D312):
            // the artifact only names the node.
            FlowNodeKind::OntologyQuery { config } => serde_json::json!({
                "type": "pnex-ontology-query",
                "filters": config.filters,
                "output": config.output,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::ObjectUpsert { config } => serde_json::json!({
                "type": "pnex-object-upsert",
                "key": config.key,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::LinkUpsert { config } => serde_json::json!({
                "type": "pnex-link-upsert",
                "close": config.close,
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::DocumentText { .. } => serde_json::json!({
                "type": "pnex-document-text",
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::SheetWrite { .. } => serde_json::json!({
                "type": "pnex-sheet-write",
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::CheckRecord { .. } => serde_json::json!({
                "type": "pnex-check-record",
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            // The backend reads the recipe from the deployed graph (D327).
            FlowNodeKind::SiteRecipe { .. } => {
                padded_ports = Some(crate::site_recipe::NODE_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-site-recipe",
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            // The backend reads the provider and options from the deployed graph (D361).
            FlowNodeKind::WebSearch { .. } => {
                padded_ports = Some(crate::web_search::NODE_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-web-search",
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::WebVisit { .. } => {
                padded_ports = Some(crate::web_visit::NODE_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-web-visit",
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            // The backend reads the crawl options from the deployed graph (D407).
            FlowNodeKind::SiteCrawl { .. } => {
                padded_ports = Some(crate::site_crawl::NODE_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-site-crawl",
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            // The backend reads the thresholds from the deployed graph (D332).
            FlowNodeKind::OrgResolve { .. } => {
                padded_ports = Some(crate::org_resolve::NODE_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-org-resolve",
                    "pnex_node_id": n.id,
                    "pnex_flow_id": meta.flow_id,
                    "pnex_version": meta.version_number,
                    "pnex_org_id": meta.org_id,
                })
            }
            FlowNodeKind::Weather { config } => {
                // Ports: current, daily, hourly.
                padded_ports = Some(crate::weather::WEATHER_PORT_COUNT);
                serde_json::json!({
                    "type": "pnex-weather",
                    "provider": config.provider,
                    "latitude": config.latitude,
                    "longitude": config.longitude,
                    "interval_min": config.interval_min,
                    "emit_on_start": config.emit_on_start,
                    "pnex_node_id": n.id,
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
                "labels": config.labels,
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
                // Raw references: the snapshot (resolved channels + template)
                // is stamped at deploy by the backend
                // (`pnex_notify_channels` / `pnex_notify_template`), not here.
                "channel_ids": config.channel_ids,
                "template_id": config.template_id,
                "strict": config.strict,
                "vars": config.vars,
                // Vars stamped at pick time — wiring trace on the deploy side
                // (template_changed drift); the runtime itself reads the vars
                // of the `pnex_notify_template` snapshot.
                "template_vars": config.template_vars,
                "anti_spam": config.anti_spam,
                "attach_field": config.attach_field,
                "to_field": config.to_field,
                "cc_field": config.cc_field,
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
                // Partially wired multi-output: the vendor indexes the
                // return value by port — pad the wires to the port count.
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
                            "outputs": s.outputs,
                            "pnex_node_id": n.id,
                            "pnex_flow_id": meta.flow_id,
                            "pnex_version": meta.version_number,
                            "pnex_org_id": meta.org_id,
                            "pnex_function_id": config.function_id,
                            "pnex_function_version": config.version_number,
                        }),
                    },
                    // Snapshot missing (function/version deleted despite the
                    // delete-guard): the backend blocks the deploy upstream;
                    // here we emit a no-op to stay total (never a panic).
                    None => serde_json::json!({
                        "type": "comment",
                        "name": format!(
                            "function not found: #{} v{}",
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
                // Identity stamped by the projection: the raw canvas id
                // ("n3") is the panel/badge attachment key, and
                // traceability stamps the node like the other customs.
                "pnex_node_id": n.id,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
            FlowNodeKind::JsonSplit { config } => {
                // One port per declared key (legacy zero keys = 1 port): pad
                // the wires to the port count (multi-output school).
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
        };

        // Unique ids in the multi-flow artifact: the editor generates short
        // ids per flow ("n1"…) and two deployed flows can
        // collide (two "n1" → the runtime no longer finds its nodes and
        // rejects the whole artifact). The per-tab prefix makes the id globally
        // unique; `pnex_node_id` keeps the raw canvas id (debug panel
        // attachment) and runtime attribution un-prefixes it.
        let node_id = format!("{tab_id}_{}", n.id);
        let wires = prefixed_wires(&wires, &tab_id);
        {
            let obj = e.as_object_mut().expect("flows.json entry");
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
        // Partially wired multi-output function: completes the `wires`
        // array to the declared port count (the vendor indexes the return value
        // by port — a short array would lose the trailing ports).
        if let Some(min_ports) = padded_ports {
            let obj = e.as_object_mut().expect("flows.json entry");
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
/// wrapper/executor then feeds `inputs.<name>` from the arriving payload
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
            FlowNodeKind::DeviceWrite { config } => config.anchors().map(String::as_str).collect(),
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
                // The tagger points back to the function node (prefixed artifact
                // id, see the node id prefixing below).
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

/// Prefixes each target id of a Node-RED `wires` array with the tab id
/// (mirror of the node `id` prefixing — both must stay
/// consistent).
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

/// Pads a Node-RED `wires` array up to `min_ports` entries with
/// empty ports (`[]`) — partially wired multi-output node.
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

/// Builds the Node-RED `wires` array: `wires[port] = [target ids]`.
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
    // EdgeLinkd inherits the legacy props (`props[].v` = string value): the
    // `payload` key must always exist — an empty string is enough for `date` mode.
    match c.payload {
        serde_json::Value::Null => {
            obj.insert("payloadType".into(), serde_json::json!("date"));
            obj.insert("payload".into(), serde_json::json!(""));
        }
        ref p => {
            // EdgeLinkd evaluates `props[].v` as a string: the JSON payload is
            // encoded as a string (vt "json") to be re-parsed at execution.
            obj.insert("payloadType".into(), serde_json::json!("json"));
            obj.insert("payload".into(), serde_json::json!(p.to_string()));
        }
    }
    if let Some(t) = &c.topic {
        // The `topic` prop is only emitted if a topic exists: without `v`, the
        // EdgeLinkd deserializer would reject the null value.
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

/// Common fields of a projected control card entry: custom type
/// (`pnex-reg-*`), binding (device + pin labels), time parameters,
/// traceability stamping (`pnex_node_id` = raw canvas id, key of the
/// wire `ControlSpec`).
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

/// Projected entry of an on/off card (heating/cooling — the type encodes
/// the direction): the deadband and the anti short-cycling complete the base.
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
    let obj = e.as_object_mut().expect("flows.json entry");
    obj.insert("deadband".into(), serde_json::json!(c.deadband));
    obj.insert("min_on_secs".into(), serde_json::json!(c.min_on_secs));
    obj.insert("min_off_secs".into(), serde_json::json!(c.min_off_secs));
    e
}

/// Projected entry of a PID card: gains + relay cycle complete the base.
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
    let obj = e.as_object_mut().expect("flows.json entry");
    obj.insert("kp".into(), serde_json::json!(c.kp));
    obj.insert("ki".into(), serde_json::json!(c.ki));
    obj.insert("kd".into(), serde_json::json!(c.kd));
    obj.insert(
        "cycle_time_secs".into(),
        serde_json::json!(c.cycle_time_secs),
    );
    e
}
