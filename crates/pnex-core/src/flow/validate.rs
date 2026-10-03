//! Graph validation: whole-graph structural checks plus per-kind node
//! contracts. Collects **all** violations at once (API returns them as a
//! 400 with a `violations` field).

use super::*;
use crate::calc::{calc_variables, validate_calc};
use crate::functions::FunctionNodeConfig;
use crate::naming::{device_payload_key, valid_device_label};

// ─────────────────────────────── Validation ───────────────────────────────

/// Valide la structure du graphe + les contrats par nœud. Retourne **toutes**
/// les violations (l'API les renvoie en 400, champ `violations`).
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
            FlowNodeKind::Metric { config } => {
                if config.metric_name.trim().is_empty() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "metric_name_missing",
                        "the metric name is required",
                    ));
                }
            }
            FlowNodeKind::CoolProp { config } => validate_coolprop(&n.id, config, &mut v),
            FlowNodeKind::PnexNotify { config } => {
                // Structurel uniquement : l'existence/état des canaux et du
                // template est vérifiée au deploy (snapshot + stale refs,
                // toast non bloquant — jamais une violation de save pour un
                // objet de l'org supprimé après coup).
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
                // Entrées nommées : un nom non vide et unique ; toute
                // annotation de câblage doit viser une entrée déclarée
                // (rattrape les orphelines après suppression de ligne).
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
                // Câblage nu (sans ligne) alors que des entrées nommées sont
                // déclarées : le payload partirait sous la clé par défaut au
                // runtime, pas sous une entrée — re-câblage imposé plutôt
                // qu'un fil qui « choisit » sa rangée au rendu.
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
                // Ports câblés hors des clés déclarées : câble mort au
                // runtime (port muet) — école device_read_port_out_of_range.
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
            FlowNodeKind::Red { type_name, config } => {
                if type_name.trim().is_empty() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "bad_red_node",
                        "missing Node-RED type",
                    ));
                }
                if !config.is_null() && !config.is_object() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "bad_red_node",
                        "the Red node config must be a JSON object",
                    ));
                }
            }
        }
    }

    // Câblage : chaque cible doit exister.
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

    // Device-read : les ports câblés existent — un port par pin + le port
    // « nom du device ». Hors bornes = câble mort au runtime (fan_out hors
    // plage) et nœud tronqué à l'affichage : rejeté au save plutôt qu'en
    // silence.
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

    // Cartes de régulation : **une seule régulation par sortie** — deux
    // cartes sur le même (device, pin actionneur) se battraient pour le
    // relais. Passes graphe-entier (l'unicité cross-flows reste garantie au
    // sync backend, « plus petit flow_id gagne »).
    let mut actuators: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    // Un nœud `device-write` et une carte régulation sur la même sortie se
    // battraient pour la pin — même unicité qu'entre cartes.
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

    // Casse des variables calc : une clé device (`device_payload_key`) garde
    // la casse du pin (« proud_puffin_A0 ») et le calc est sensible à la
    // casse — une variable qui ne diffère d'une clé device du graphe que par
    // la casse est une erreur quasi certaine (`proud_puffin_a0`), qui ne
    // mourait sinon qu'en silence au runtime (calc rejeté → display/metric
    // en aval ne reçoivent plus rien). Les autres variables (colonnes SQL,
    // constantes) restent hors de portée : seul le near-match est signalé.
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

/// Scheme http(s) uniquement (rejette `file://` et assimilés) — check
/// manuel, pnex-core n'a pas de dep URL (wasm-safe, zéro dep).
fn valid_http_scheme(url: &str) -> bool {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let s = scheme.to_ascii_lowercase();
            (s == "http" || s == "https") && !rest.trim().is_empty()
        }
        None => false,
    }
}

/// Validation structurelle d'un nœud fonction (registre « Fonctions ») :
/// références posées. L'existence réelle de la fonction/version est vérifiée
/// au deploy (`function_unresolved`) — jamais une violation de save pour un
/// objet de l'org supprimé après coup (école notify).
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

/// Règles du nœud `pnex-http-fetch` : URL http(s) requise, headers nommés,
/// sous-champs d'auth complets selon le mode, proxy url requis si activé,
/// timeout borné 1..=300 s.
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

/// Règles du nœud `device` : au moins une lecture, device_id en slug
/// (`valid_device_label` — interpolé dans un sélecteur PromQL), pin non
/// vide, clés de payload uniques, fenêtre bornée 1..=3600 s.
/// Validation structurelle du nœud `pnex-coolprop` (pnex-core, sans
/// CoolProp) : champs requis présents. La validité CoolProp proprement
/// dite (paires/paramètres connus) est vérifiée au build du nœud côté
/// runtime — fail-fast au pré-flight `--check` — et à la sauvegarde des
/// mélanges côté backend.
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

/// Validation commune read/write : slug device requis, pins non vides et
/// uniques (normalisation trim + casse ASCII — le payload est sensible à la
/// casse mais « D1 » vs « d1 » est une quasi-certitude d'erreur).
fn validate_device_common(
    id: &str,
    device_id: &str,
    pins: &[String],
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
    if pins.is_empty() {
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
    validate_device_common(id, &c.device_id, &c.pins, v);
    if !(c.window_secs.is_finite() && (1.0..=3600.0).contains(&c.window_secs)) {
        v.push(FlowViolation::new(
            Some(id),
            "device_window_range",
            "window_secs must be between 1 and 3600",
        ));
    }
}

fn validate_device_write(id: &str, c: &DeviceWriteConfig, v: &mut Vec<FlowViolation>) {
    validate_device_common(id, &c.device_id, &c.pins, v);
}

/// Égalité de labels de pins, normalisée (trim + casse ASCII ignorée) —
/// wasm-safe (pas de `normalize_measurement_name`, feature `naming` absente
/// du front).
pub(super) fn pin_key(label: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(label.trim().to_ascii_lowercase())
}

/// Règles communes aux cartes de régulation (liaison device + pins + temps).
/// `sensor_mode_hint` ne sert qu'au message ; le mode réel est déduit du pin
/// résolu côté backend (`caps::validate`, point unique — jamais de mode saisi).
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

/// Règles d'une carte tout-ou-rien (`reg_tt_heat` / `reg_tt_cool`).
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

/// Règles d'une carte PID (`reg_pid`) — sortie relais time-proportional.
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
