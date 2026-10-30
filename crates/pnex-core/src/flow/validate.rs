//! Graph validation: whole-graph structural checks plus per-kind node
//! contracts. Collects **all** violations at once (API returns them as a
//! 400 with a `violations` field).

use super::*;
use crate::calc::{calc_variables, validate_calc};
use crate::functions::FunctionNodeConfig;
use crate::naming::{device_payload_key, valid_device_label};

// ─────────────────────────────── Validation ───────────────────────────────

/// Validates the graph structure + the per-node contracts. Returns **all**
/// violations (the API returns them as 400, `violations` field).
pub fn validate_graph(g: &FlowGraph) -> Vec<FlowViolation> {
    let mut v = Vec::new();
    if g.nodes.is_empty() {
        v.push(FlowViolation::new(
            None,
            "empty_graph",
            "the graph is empty",
        ));
    }

    let mut seen = std::collections::HashSet::new();
    for n in &g.nodes {
        if !seen.insert(n.id.clone()) {
            v.push(FlowViolation::with_args(
                Some(&n.id),
                "duplicate_node_id",
                "duplicated node id",
                serde_json::json!({ "node_id": n.id }),
            ));
        }
        match &n.kind {
            FlowNodeKind::Inject { config } => validate_inject(&n.id, config, &mut v),
            FlowNodeKind::DeviceRead { config } => validate_device_read(&n.id, config, &mut v),
            FlowNodeKind::DeviceWrite { config } => validate_device_write(&n.id, config, &mut v),
            FlowNodeKind::Calc { config } => {
                for e in validate_calc(&config.expression) {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "calc_bad_expression",
                        e.to_string(),
                    ));
                }
            }
            FlowNodeKind::Value { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::CameraSource { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::VideoRecord { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::EventLog { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::VisionDetect { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::TextDecide { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::VisionCaption { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::PagePdf { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::PageWrite { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::LlmText { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::MediaFuse { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::VisionTrack { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::VisionZone { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::Anomaly { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::Forecast { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::MemoryWrite { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::MemoryRead { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::ControlSource { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::ActionTrigger { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            // An empty node saves as a draft; the mailboxes are checked at deploy.
            FlowNodeKind::MailReceived { config } => {
                if let Some((code, message)) = config.check_deployable() {
                    if code != crate::err_codes::MAIL_NO_MAILBOX {
                        v.push(FlowViolation::new(Some(&n.id), code, message));
                    }
                }
            }
            FlowNodeKind::MailSend { config } => {
                if let Some((code, message)) = config.check_deployable() {
                    if code != crate::err_codes::MAIL_NO_MAILBOX {
                        v.push(FlowViolation::new(Some(&n.id), code, message));
                    }
                }
            }
            FlowNodeKind::MediaSource { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::TopicClassify { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::RangeUpsert { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::OntologyQuery { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::ObjectUpsert { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::LinkUpsert { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::SiteRecipe { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::WebSearch { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::OrgResolve { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            // Nothing to check: one bool.
            FlowNodeKind::WebVisit { .. } => {}
            // The payload is checked by the server at each run (D426).
            FlowNodeKind::CheckRecord { .. } => {}
            // No setting: the message is checked by the server (D416, D420).
            FlowNodeKind::DocumentText { .. } | FlowNodeKind::SheetWrite { .. } => {}
            FlowNodeKind::SiteCrawl { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::Weather { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::Metric { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
            }
            FlowNodeKind::CoolProp { config } => validate_coolprop(&n.id, config, &mut v),
            FlowNodeKind::PnexNotify { config } => {
                // Structural only: the existence/state of the channels and of the
                // template is checked at deploy (snapshot + stale refs,
                // non-blocking toast — never a save violation for an
                // org object deleted afterwards).
                if config.channel_ids.is_empty() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "notify_no_channels",
                        "select at least one notification channel",
                    ));
                }
                if config.template_id.is_nil() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "notify_no_template",
                        "select a notification template",
                    ));
                }
                if config.to_field.trim().is_empty() && !config.cc_field.trim().is_empty() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "notify_cc_without_to",
                        "a copy address needs a recipient field",
                    ));
                }
                // The trigger gate is MANDATORY: without it the node would
                // send on every completing message (spam on fast sources).
                let trigger_wired = n.inputs.iter().any(|w| w.pin == crate::NOTIFY_TRIGGER_PIN);
                if !trigger_wired {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "notify_trigger_required",
                        "wire the trigger input (a logic function output deciding whether the notification is sent) — without it the node would spam on every message",
                    ));
                }
                // Topic-routing collision: with the trigger input wired, a
                // template var literally named `trigger` would share the
                // `topic = "trigger"` tagger with the gate — reserve the name.
                if trigger_wired
                    && config
                        .template_vars
                        .iter()
                        .any(|v| v == crate::NOTIFY_TRIGGER_PIN)
                {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "notify_trigger_conflict",
                        "a template variable named \"trigger\" collides with the trigger input — rename the variable in the template",
                    ));
                }
                if let Some(anti) = &config.anti_spam {
                    if anti.max_msgs == 0 {
                        v.push(FlowViolation::new(
                            Some(&n.id),
                            "notify_anti_spam_max",
                            "anti-spam requires at least 1 message per window",
                        ));
                    }
                    if anti.window_secs == 0 {
                        v.push(FlowViolation::new(
                            Some(&n.id),
                            "notify_anti_spam_window",
                            "anti-spam requires a window of at least 1 second",
                        ));
                    }
                }
            }
            FlowNodeKind::HttpFetch { config } => validate_http_fetch(&n.id, config, &mut v),
            FlowNodeKind::PnexFunction { config } => validate_function_node(&n.id, config, &mut v),
            FlowNodeKind::JsonMerge { config } => {
                if let Some((code, message)) = config.check() {
                    v.push(FlowViolation::new(Some(&n.id), code, message));
                }
                // Named inputs: a non-empty, unique name; every
                // wiring annotation must target a declared input
                // (catches orphans after a row is deleted).
                let mut declared: std::collections::BTreeSet<&str> =
                    std::collections::BTreeSet::new();
                for name in &config.inputs {
                    if name.trim().is_empty() {
                        v.push(FlowViolation::new(
                            Some(&n.id),
                            "merge_input_name_missing",
                            "each merge input must have a name",
                        ));
                        continue;
                    }
                    if !declared.insert(name.as_str()) {
                        v.push(FlowViolation::with_args(
                            Some(&n.id),
                            "merge_input_duplicate",
                            "duplicated input name",
                            serde_json::json!({ "name": name }),
                        ));
                    }
                }
                for w in &n.inputs {
                    if !declared.contains(w.pin.as_str()) {
                        v.push(FlowViolation::with_args(
                            Some(&n.id),
                            "merge_input_annotation_unknown",
                            "a wire targets a removed input — rewire or delete the line",
                            serde_json::json!({ "name": w.pin }),
                        ));
                    }
                }
                // Bare wiring (no row) while named inputs are
                // declared: the payload would go under the default key at
                // runtime, not under an input — rewiring enforced rather
                // than a wire that "picks" its row at render.
                if !config.inputs.is_empty() {
                    let unbound = g.nodes.iter().any(|s| {
                        s.outputs.iter().any(|w| {
                            w.targets.contains(&n.id)
                                && !n
                                    .inputs
                                    .iter()
                                    .any(|a| a.from == s.id && a.from_port == w.port)
                        })
                    });
                    if unbound {
                        v.push(FlowViolation::new(
                            Some(&n.id),
                            "merge_wire_unbound",
                            "a wire arrives without a named input — drag from a merge row to the source (or delete the row then rewire)",
                        ));
                    }
                }
            }
            FlowNodeKind::JsonSplit { config } => {
                // Ports wired outside the declared keys: dead wire at
                // runtime (silent port) — device_read_port_out_of_range school.
                let limit = config.keys.len().max(1);
                for w in &n.outputs {
                    if w.port >= limit {
                        v.push(FlowViolation::with_args(
                            Some(&n.id),
                            "split_port_out_of_range",
                            "json-split port out of wiring — rename or delete the keys in the inspector",
                            serde_json::json!({
                                "port": w.port.to_string(),
                                "keys": limit.to_string(),
                            }),
                        ));
                    }
                }
            }
            FlowNodeKind::Debug { .. } => {}
            FlowNodeKind::Display { .. } => {}
            FlowNodeKind::RegTtHeat { config } => validate_reg_tt(&n.id, config, &mut v),
            FlowNodeKind::RegTtCool { config } => validate_reg_tt(&n.id, config, &mut v),
            FlowNodeKind::RegPid { config } => validate_reg_pid(&n.id, config, &mut v),
        }
    }

    // Wiring: every target must exist.
    for n in &g.nodes {
        for w in &n.outputs {
            for t in &w.targets {
                if !seen.contains(t) {
                    v.push(FlowViolation::with_args(
                        Some(&n.id),
                        "dangling_target",
                        "wiring port targets an unknown node",
                        serde_json::json!({
                            "port": w.port.to_string(),
                            "node": n.id,
                            "target": t,
                        }),
                    ));
                }
            }
        }
    }

    // Device-read: the wired ports exist — one port per pin + the
    // "device name" port. Out of bounds = dead wire at runtime (fan_out out of
    // range) and node truncated on display: rejected on save rather than
    // silently.
    for n in &g.nodes {
        if let FlowNodeKind::DeviceRead { config } = &n.kind {
            let limit = config.pins.len() + 1;
            for w in &n.outputs {
                if w.port >= limit {
                    v.push(FlowViolation::with_args(
                        Some(&n.id),
                        "device_read_port_out_of_range",
                        "device-read port out of wiring — uncheck/recheck the pins in the inspector",
                        serde_json::json!({
                            "port": w.port.to_string(),
                            "pins": config.pins.len().to_string(),
                            "ports": limit.to_string(),
                        }),
                    ));
                }
            }
        }
    }

    // Control cards: **a single control per output** — two
    // cards on the same (device, actuator pin) would fight over the
    // relay. Whole-graph passes (cross-flow uniqueness stays guaranteed at
    // the backend sync, "smallest flow_id wins").
    let mut actuators: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    // A `device-write` node and a control card on the same output would
    // fight over the pin — same uniqueness as between cards.
    for n in &g.nodes {
        if let FlowNodeKind::DeviceWrite { config } = &n.kind {
            for pin in &config.pins {
                let key = format!("{}|{}", config.device_id.trim(), pin_key(pin));
                match actuators.get(&key) {
                    Some(first) => v.push(FlowViolation::with_args(
                        Some(&n.id),
                        "reg_actuator_conflict",
                        "pin already driven by another node (one write source per output)",
                        serde_json::json!({
                            "pin": pin.trim(),
                            "device": config.device_id.trim(),
                            "node": *first,
                        }),
                    )),
                    None => {
                        actuators.insert(key, n.id.as_str());
                    }
                }
            }
        }
    }
    for n in &g.nodes {
        if let Some(cfg) = reg_config_of(&n.kind) {
            let key = cfg.actuator_key();
            match actuators.get(&key) {
                Some(first) => v.push(FlowViolation::with_args(
                    Some(&n.id),
                    "reg_actuator_conflict",
                    "output already regulated by another node (one card per output)",
                    serde_json::json!({
                        "pin": cfg.actuator_pin().trim(),
                        "device": cfg.device_id(),
                        "node": *first,
                    }),
                )),
                None => {
                    actuators.insert(key, n.id.as_str());
                }
            }
        }
    }

    // Case of calc variables: a device key (`device_payload_key`) keeps
    // the pin's case ("proud_puffin_A0") and calc is case-sensitive
    // — a variable that differs from a graph device key only by
    // case is almost certainly an error (`proud_puffin_a0`), which otherwise
    // only died silently at runtime (calc rejected → downstream display/metric
    // no longer receive anything). The other variables (SQL columns,
    // constants) stay out of scope: only the near-match is flagged.
    let device_keys: Vec<String> = g
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            FlowNodeKind::DeviceRead { config } => Some(
                config
                    .pins
                    .iter()
                    .map(|pin| device_payload_key(&config.device_id, pin)),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    for n in &g.nodes {
        if let FlowNodeKind::Calc { config } = &n.kind {
            for var in calc_variables(&config.expression) {
                if device_keys.iter().any(|k| k == &var) {
                    continue;
                }
                let Some(expected) = device_keys.iter().find(|k| k.eq_ignore_ascii_case(&var))
                else {
                    continue;
                };
                v.push(FlowViolation::with_args(
                    Some(&n.id),
                    "calc_case_mismatch",
                    "unknown variable — the device key is case-sensitive",
                    serde_json::json!({
                        "variable": var,
                        "expected": expected,
                    }),
                ));
            }
        }
    }
    // One recorder per camera: a second one would write the same frames
    // twice into the camera's timeline.
    let mut recorded = std::collections::HashSet::new();
    for claim in video_record_claims_of(g) {
        if !recorded.insert(claim.device_id.clone()) {
            v.push(FlowViolation::with_args(
                Some(&claim.node_id),
                "video_record_duplicate",
                "another video-record node of this flow already records this camera",
                serde_json::json!({ "camera": claim.device_id }),
            ));
        }
    }
    v
}

fn validate_inject(id: &str, c: &InjectConfig, v: &mut Vec<FlowViolation>) {
    if let Some(r) = c.repeat_secs {
        if !(r.is_finite() && r > 0.0) {
            v.push(FlowViolation::new(
                Some(id),
                "bad_repeat",
                "repeat_secs must be > 0",
            ));
        }
    }
    if c.repeat_secs.is_none() && c.cron.trim().is_empty() && c.once_delay_secs.is_none() {
        v.push(FlowViolation::new(
            Some(id),
            "no_trigger",
            "the inject node has no trigger (repeat_secs, cron or once_delay_secs required)",
        ));
    }
}

/// http(s) scheme only (rejects `file://` and the like) — manual
/// check, pnex-core has no URL dep (wasm-safe, zero dep).
fn valid_http_scheme(url: &str) -> bool {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let s = scheme.to_ascii_lowercase();
            (s == "http" || s == "https") && !rest.trim().is_empty()
        }
        None => false,
    }
}

/// Structural validation of a function node ("Functions" registry):
/// references set. The actual existence of the function/version is checked
/// at deploy (`function_unresolved`) — never a save violation for an
/// org object deleted afterwards (notify school).
fn validate_function_node(id: &str, c: &FunctionNodeConfig, v: &mut Vec<FlowViolation>) {
    if c.function_id == 0 || c.function_name.trim().is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "fn_not_selected",
            "select a function from the registry",
        ));
        return;
    }
    if c.version_number <= 0 {
        v.push(FlowViolation::new(
            Some(id),
            "fn_no_version",
            "pin a version of the function",
        ));
    }
}

/// Rules of the `pnex-http-fetch` node: http(s) URL required, named headers,
/// auth sub-fields complete for the mode, proxy url required if enabled,
/// timeout bounded 1..=300 s.
fn validate_http_fetch(id: &str, c: &HttpFetchNodeConfig, v: &mut Vec<FlowViolation>) {
    if c.url.trim().is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "http_fetch_url_missing",
            "the request URL is required",
        ));
    } else if !valid_http_scheme(c.url.trim()) {
        v.push(FlowViolation::new(
            Some(id),
            "http_fetch_url_scheme",
            "the URL must start with http:// or https://",
        ));
    }
    for h in &c.headers {
        if h.name.trim().is_empty() {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_header_empty",
                "each header must have a name",
            ));
        }
    }
    match &c.auth {
        HttpFetchAuth::Basic { username, .. } if username.trim().is_empty() => {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_auth_incomplete",
                "the username (basic auth) is required",
            ));
        }
        HttpFetchAuth::Bearer { token } if token.is_unset() => {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_auth_incomplete",
                "the bearer token is required",
            ));
        }
        HttpFetchAuth::Header { name, .. } if name.trim().is_empty() => {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_auth_incomplete",
                "the auth header name is required",
            ));
        }
        _ => {}
    }
    if let HttpFetchProxy::Custom { url, .. } = &c.proxy {
        if url.trim().is_empty() {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_proxy_missing",
                "the proxy URL is required",
            ));
        } else if !valid_http_scheme(url.trim()) {
            v.push(FlowViolation::new(
                Some(id),
                "http_fetch_proxy_missing",
                "the proxy URL must start with http:// or https://",
            ));
        }
    }
    if c.timeout_secs == 0 || c.timeout_secs > 300 {
        v.push(FlowViolation::new(
            Some(id),
            "http_fetch_timeout_range",
            "the timeout must be between 1 and 300 s",
        ));
    }
}

/// Rules of the `device` node: at least one read, device_id as a slug
/// (`valid_device_label` — interpolated in a PromQL selector), non-empty pin,
/// unique payload keys, window bounded 1..=3600 s.
/// Structural validation of the `pnex-coolprop` node (pnex-core, without
/// CoolProp): required fields present. CoolProp validity proper
/// (known pairs/parameters) is checked when the node is built on the
/// runtime side — fail-fast at the `--check` pre-flight — and when
/// mixtures are saved on the backend side.
fn validate_coolprop(id: &str, c: &CoolPropConfig, v: &mut Vec<FlowViolation>) {
    if c.fluid_spec.trim().is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "coolprop_fluid_missing",
            "the fluid/mixture spec is required (e.g. Water, R410A, Propane[0.5]&Ethane[0.5])",
        ));
    }
    for (field, value) in [
        ("input1", &c.input1),
        ("input2", &c.input2),
        ("v1_key", &c.v1_key),
        ("v2_key", &c.v2_key),
    ] {
        if value.trim().is_empty() {
            v.push(FlowViolation::with_args(
                Some(id),
                "coolprop_input_missing",
                "required field missing",
                serde_json::json!({ "field": field }),
            ));
        }
    }
    if c.outputs.is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "coolprop_outputs_missing",
            "at least one output property is required (e.g. Dmolar, Hmolar, Smolar)",
        ));
    }
    if !c.input1.trim().is_empty() && c.input1 == c.input2 {
        v.push(FlowViolation::new(
            Some(id),
            "coolprop_inputs_identical",
            "the two inputs must be different quantities",
        ));
    }
    if !c.v1_key.trim().is_empty() && c.v1_key == c.v2_key {
        v.push(FlowViolation::new(
            Some(id),
            "coolprop_input_keys_identical",
            "the two inputs must use different keys",
        ));
    }
    // A unit must belong to its quantity's dimension (catalogue quantities
    // only — an unknown CoolProp name stays SI).
    let units = [(&c.input1, &c.unit1), (&c.input2, &c.unit2)]
        .into_iter()
        .map(|(q, u)| (q.as_str(), u.as_str()))
        .chain(c.outputs.iter().map(|o| (o.as_str(), c.output_unit(o))));
    for (quantity, unit) in units {
        if unit.is_empty() {
            continue;
        }
        if crate::thermo_quantity(quantity).is_some()
            && crate::thermo_unit(quantity, unit).is_none()
        {
            v.push(FlowViolation::with_args(
                Some(id),
                "coolprop_unit_invalid",
                "unit not valid for this quantity",
                serde_json::json!({ "quantity": quantity, "unit": unit }),
            ));
        }
    }
}

/// Common read/write validation: device slug required, non-empty and
/// unique pins (trim + ASCII case normalization — the payload is case-sensitive
/// but "D1" vs "d1" is almost certainly an error).
fn validate_device_common(
    id: &str,
    device_id: &str,
    pins: &[String],
    require_pins: bool,
    v: &mut Vec<FlowViolation>,
) -> bool {
    let mut ok = true;
    if !valid_device_label(device_id) {
        v.push(FlowViolation::with_args(
            Some(id),
            "device_bad_slug",
            "invalid device (slug required: letters, digits, . _ -)",
            serde_json::json!({ "device": device_id }),
        ));
        ok = false;
    }
    if require_pins && pins.is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "device_no_pins",
            "no pin selected (at least one pin is required)",
        ));
        ok = false;
    }
    let mut seen = std::collections::HashSet::new();
    for pin in pins {
        if pin.trim().is_empty() {
            v.push(FlowViolation::with_args(
                Some(id),
                "device_bad_pin",
                "empty pin for the device",
                serde_json::json!({ "device": device_id }),
            ));
            ok = false;
            continue;
        }
        if !seen.insert(pin_key(pin)) {
            v.push(FlowViolation::with_args(
                Some(id),
                "device_duplicate_pin",
                "duplicated pin (labels must be unique)",
                serde_json::json!({ "pin": pin.trim() }),
            ));
            ok = false;
        }
    }
    ok
}

fn validate_device_read(id: &str, c: &DeviceReadConfig, v: &mut Vec<FlowViolation>) {
    validate_device_common(id, &c.device_id, &c.pins, true, v);
    if !(c.window_secs.is_finite() && (1.0..=3600.0).contains(&c.window_secs)) {
        v.push(FlowViolation::new(
            Some(id),
            "device_window_range",
            "window_secs must be between 1 and 3600",
        ));
    }
}

fn validate_device_write(id: &str, c: &DeviceWriteConfig, v: &mut Vec<FlowViolation>) {
    // A custom firmware may be driven by commands only: pins are then
    // optional, but the node still needs at least one anchor.
    validate_device_common(id, &c.device_id, &c.pins, c.commands.is_empty(), v);
    let mut seen: std::collections::HashSet<_> = c.pins.iter().map(|p| pin_key(p)).collect();
    for (pin, value) in &c.values {
        if !is_fixed_write_value(value) || !c.pins.contains(pin) {
            v.push(FlowViolation::with_args(
                Some(id),
                "device_bad_fixed_value",
                "declared value: a selected output pin, with on/off or a duty from 0 to 100",
                serde_json::json!({ "pin": pin }),
            ));
        }
    }
    for cmd in &c.commands {
        if !valid_command_name(cmd) {
            v.push(FlowViolation::with_args(
                Some(id),
                "device_bad_command",
                "invalid firmware command name (letters, digits, _ . - ; 1 to 32 characters)",
                serde_json::json!({ "command": cmd }),
            ));
            continue;
        }
        if !seen.insert(pin_key(cmd)) {
            v.push(FlowViolation::with_args(
                Some(id),
                "device_duplicate_pin",
                "duplicated pin or command (anchor names must be unique)",
                serde_json::json!({ "pin": cmd.trim() }),
            ));
        }
    }
}

/// A declared pin value: a boolean, or a number from 0 to 100 (0/1 for a
/// digital output, a duty for PWM — the pin mode is checked at write time).
fn is_fixed_write_value(v: &serde_json::Value) -> bool {
    v.is_boolean() || v.as_f64().is_some_and(|n| (0.0..=100.0).contains(&n))
}

/// Name of a custom-firmware command (`pnex.onCommand`): the announced cap
/// id, kept short and free of separators used by topics and anchors.
pub fn valid_command_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Pin label equality, normalized (trim + ASCII case ignored) —
/// wasm-safe (no `normalize_measurement_name`, `naming` feature absent
/// from the front).
pub(super) fn pin_key(label: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(label.trim().to_ascii_lowercase())
}

/// Rules common to control cards (device binding + pins + timing).
/// `sensor_mode_hint` only serves the message; the real mode is derived from the
/// resolved pin on the backend side (`caps::validate`, single point — never an entered mode).
fn validate_reg_common(
    id: &str,
    device_id: &str,
    sensor_pin: &str,
    actuator_pin: &str,
    sample_ms: u32,
    data_timeout_secs: u32,
    v: &mut Vec<FlowViolation>,
) {
    if !valid_device_label(device_id) {
        v.push(FlowViolation::with_args(
            Some(id),
            "reg_bad_device",
            "invalid device (slug required: letters, digits, . _ -)",
            serde_json::json!({ "device": device_id }),
        ));
    }
    if sensor_pin.trim().is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "reg_pin_missing",
            "missing sensor pin (an input of the same device)",
        ));
    }
    if actuator_pin.trim().is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "reg_pin_missing",
            "missing actuator pin (an output of the same device)",
        ));
    }
    if !sensor_pin.trim().is_empty()
        && !actuator_pin.trim().is_empty()
        && pin_key(sensor_pin) == pin_key(actuator_pin)
    {
        v.push(FlowViolation::new(
            Some(id),
            "reg_pins_equal",
            "mixed card: the sensor pin and the actuator pin must be different",
        ));
    }
    if !(200..=60_000).contains(&sample_ms) {
        v.push(FlowViolation::new(
            Some(id),
            "reg_sample_range",
            "sample_ms must be between 200 and 60000",
        ));
    }
    let sample_floor_secs = (sample_ms as f64 / 1000.0).ceil() as u32;
    if data_timeout_secs < sample_floor_secs.max(1) || data_timeout_secs > 3_600 {
        v.push(FlowViolation::with_args(
            Some(id),
            "reg_data_timeout_range",
            "data_timeout_secs out of range (minimum follows sample_ms, maximum 3600)",
            serde_json::json!({ "min_secs": sample_floor_secs.to_string() }),
        ));
    }
}

/// Rules of an on/off card (`reg_tt_heat` / `reg_tt_cool`).
fn validate_reg_tt(id: &str, c: &RegTtConfig, v: &mut Vec<FlowViolation>) {
    validate_reg_common(
        id,
        &c.device_id,
        &c.sensor_pin,
        &c.actuator_pin,
        c.sample_ms,
        c.data_timeout_secs,
        v,
    );
    if !c.setpoint.is_finite() {
        v.push(FlowViolation::new(
            Some(id),
            "reg_setpoint_range",
            "the setpoint must be a finite number",
        ));
    }
    if !(c.deadband.is_finite() && c.deadband > 0.0) {
        v.push(FlowViolation::new(
            Some(id),
            "reg_deadband_range",
            "deadband must be > 0 (hysteresis half-band, same unit as the setpoint)",
        ));
    }
}

/// Rules of a PID card (`reg_pid`) — time-proportional relay output.
fn validate_reg_pid(id: &str, c: &RegPidConfig, v: &mut Vec<FlowViolation>) {
    validate_reg_common(
        id,
        &c.device_id,
        &c.sensor_pin,
        &c.actuator_pin,
        c.sample_ms,
        c.data_timeout_secs,
        v,
    );
    if !c.setpoint.is_finite() {
        v.push(FlowViolation::new(
            Some(id),
            "reg_setpoint_range",
            "the setpoint must be a finite number",
        ));
    }
    for (name, gain) in [("kp", c.kp), ("ki", c.ki), ("kd", c.kd)] {
        if !(gain.is_finite() && gain >= 0.0) {
            v.push(FlowViolation::with_args(
                Some(id),
                "reg_pid_gain_range",
                "gain must be a finite number ≥ 0",
                serde_json::json!({ "gain": name }),
            ));
        }
    }
    if !(1..=60).contains(&c.cycle_time_secs) {
        v.push(FlowViolation::new(
            Some(id),
            "reg_cycle_time_range",
            "cycle_time_secs must be between 1 and 60",
        ));
    }
}

// ─────────────────────────────── Projection ───────────────────────────────
