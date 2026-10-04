//! Registre des outils de l'assistant IA — **la seule surface d'exécution
//! des sorties du LLM**.
//!
//! Garde-fou structurel : `execute` est un `match` fermé sur exactement les
//! 11 outils déclarés dans `tool_specs()`. Il n'existe ni outil générique
//! (http/sql), ni chemin vers `flow_supervisor` (deploy), ni suppression,
//! ni commande device. Tout nom hors de ce match → erreur « outil inconnu »
//! renvoyée au modèle (qui s'excuse et se corrige).
//!
//! Chaque outil est scoping org via `ToolDeps.org_id` (jamais de paramètre
//! org côté modèle) et les outils d'écriture re-vérifient `can_write`.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde_json::{json, Value};

use crate::models::_entities::{
    device_capability_instances, device_registries, flow_versions, flows, notify_channels,
    notify_templates, predefined_devices,
};
use crate::services::ai::provider::ToolSpec;
use crate::services::flow;

/// Dépendances injectées par le contrôleur (déjà authentifiées).
pub struct ToolDeps<'a> {
    pub db: &'a DatabaseConnection,
    /// Org du membre qui parle à l'assistant (scoping systématique).
    pub org_id: i64,
    /// `OrgContext::can_write()` — requis par les outils d'écriture.
    pub can_write: bool,
    /// Auteur tracé dans `flow_versions.author`.
    pub author: Option<String>,
    /// Client O2 résolu (`None` = télémétrie dégradée).
    pub o2: Option<&'a crate::services::openobserve::client::Client>,
}

/// Résultat d'exécution : la valeur JSON vue par le modèle + l'id du flow
/// touché (pour le deep-link « Ouvrir dans l'éditeur » côté UI).
#[derive(Debug)]
pub struct ToolOutcome {
    pub value: Value,
    /// Positionné par create_flow/update_flow.
    pub flow_id: Option<i64>,
}

/// Trace d'un outil, renvoyée au front (bulle repliable ✓/✗).
#[derive(Clone, Debug)]
pub struct ToolTrace {
    pub name: String,
    pub arguments: Value,
    pub ok: bool,
    /// Résumé court, en français, affiché tel quel.
    pub summary: String,
    /// Flow touché — pilote le bouton « Ouvrir dans l'éditeur ».
    pub flow_id: Option<i64>,
}

/// Les 10 outils du registre (liste figée, ordre du tableau de conception).
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "list_devices",
            description: "Liste les devices de l'organisation (id interne, slug, modèle, actif).",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolSpec {
            name: "get_device_pins",
            description: "Détail des pins d'un device (gpio, label, mode) — id = id interne renvoyé par list_devices.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "device_id": {"type": "integer", "description": "id interne du device (list_devices)"}
                },
                "required": ["device_id"]
            }),
        },
        ToolSpec {
            name: "list_flows",
            description: "Liste les flows de l'organisation (nom, statut draft/deployed, versions).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {"type": "string", "enum": ["draft", "deployed", "error"], "description": "filtre optionnel"}
                },
                "required": []
            }),
        },
        ToolSpec {
            name: "get_flow",
            description: "Détail d'un flow (graphe complet de la dernière version) — id = id interne renvoyé par list_flows.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "flow_id": {"type": "integer", "description": "id interne du flow (list_flows)"}
                },
                "required": ["flow_id"]
            }),
        },
        ToolSpec {
            name: "list_notifications",
            description: "Lists the organization's notification channels (id, name, kind, enabled — never their settings or secrets) and message templates (id, name, declared variables). Needed to wire a pnex_notify node: channel_ids and template_id are these UUIDs.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolSpec {
            name: "query_telemetry",
            description: "Points d'une série télémétrie OpenObserve (métrique × device) sur une fenêtre, + résumé statistique. Utilise la même source que la page Visualisation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "metric": {"type": "string", "description": "nom de la métrique (ex. soil_moisture, a0 — voir le catalogue dans le contexte)"},
                    "device_id": {"type": "string", "description": "slug du device (dimension device_id des séries)"},
                    "window": {"type": "string", "enum": ["1h", "6h", "24h"], "description": "fenêtre glissante"}
                },
                "required": ["metric", "device_id", "window"]
            }),
        },
        ToolSpec {
            name: "describe_node_types",
            description: "Documentation des types de nœuds de flow disponibles et du pipeline canonique — à consulter avant de créer/modifier un flow.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolSpec {
            name: "validate_flow_graph",
            description: "Valide un graphe de flow (structure, nœuds, câblage) SANS le sauvegarder — renvoie les violations à corriger. À appeler avant chaque create_flow/update_flow.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "graph": {"type": "object", "description": "graphe FlowGraph {nodes: [...]}"}
                },
                "required": ["graph"]
            }),
        },
        ToolSpec {
            name: "validate_calc_expression",
            description: "Valide la syntaxe d'une expression calc (formule du nœud calc) SANS la sauvegarder. La casse des clés device n'est vérifiable que dans le graphe : utiliser validate_flow_graph après branchement au device.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "expression": {"type": "string", "description": "expression (variables = clés du payload device, ex. soil_sensor_A0)"}
                },
                "required": ["expression"]
            }),
        },
        ToolSpec {
            name: "create_flow",
            description: "Crée un flow NOUVEAU en brouillon (draft, version 1) — jamais déployé. Le graphe doit être validé au préalable (validate_flow_graph).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "minLength": 1, "maxLength": 200, "description": "nom du flow"},
                    "graph": {"type": "object", "description": "graphe FlowGraph {nodes: [...]}"},
                    "device_id": {"type": "integer", "description": "id interne optionnel du device attaché (list_devices)"}
                },
                "required": ["name", "graph"]
            }),
        },
        ToolSpec {
            name: "update_flow",
            description: "Enregistre une NOUVELLE VERSION brouillon d'un flow existant (append-only, jamais déployée). Utiliser get_flow pour obtenir la version courante ; en cas de 409, recharger avec get_flow.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "flow_id": {"type": "integer", "description": "id interne du flow (list_flows)"},
                    "graph": {"type": "object", "description": "graphe FlowGraph complet de la nouvelle version"},
                    "note": {"type": "string", "description": "note de version optionnelle"}
                },
                "required": ["flow_id", "graph"]
            }),
        },
    ]
}

// ───────────────────────── Extraction d'arguments ─────────────────────────

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("argument '{key}' manquant ou non textuel"))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("argument '{key}' manquant ou non entier"))
}

fn arg_value<'a>(args: &'a Value, key: &str) -> Result<&'a Value, String> {
    args.get(key)
        .ok_or_else(|| format!("argument '{key}' manquant"))
}

// ─────────────────────────── Exécution ───────────────────────────

/// Exécute un outil du registre. `match` **fermé** — tout autre nom est
/// refusé. Les erreurs (String) sont rendues au modèle comme résultat
/// d'outil en échec ; il se corrige et répond à l'utilisateur.
pub async fn execute(deps: &ToolDeps<'_>, name: &str, args: &Value) -> Result<ToolOutcome, String> {
    match name {
        "list_devices" => list_devices(deps).await,
        "get_device_pins" => get_device_pins(deps, args).await,
        "list_flows" => list_flows(deps, args).await,
        "get_flow" => get_flow(deps, args).await,
        "query_telemetry" => query_telemetry(deps, args).await,
        "list_notifications" => list_notifications(deps).await,
        "describe_node_types" => describe_node_types(),
        "validate_flow_graph" => validate_flow_graph(args),
        "validate_calc_expression" => validate_calc_expression(args),
        "create_flow" => create_flow(deps, args).await,
        "update_flow" => update_flow(deps, args).await,
        _ => Err(format!(
            "outil inconnu: {name} — seuls les outils listés dans la conversation sont disponibles"
        )),
    }
}

/// Résumé court pour la trace UI.
pub fn summarize(name: &str, out: &ToolOutcome) -> String {
    match name {
        "list_devices" => format!(
            "{} device(s)",
            out.value["devices"].as_array().map_or(0, Vec::len)
        ),
        "get_device_pins" => format!(
            "device #{} — {} pin(s)",
            out.value["device_id"],
            out.value["pins"].as_array().map_or(0, Vec::len)
        ),
        "list_flows" => format!(
            "{} flow(s)",
            out.value["flows"].as_array().map_or(0, Vec::len)
        ),
        "get_flow" => format!(
            "flow #{} « {} »",
            out.value["flow"]["id"], out.value["flow"]["name"]
        ),
        "query_telemetry" => format!(
            "{} point(s), dernier = {}",
            out.value["points"].as_array().map_or(0, Vec::len),
            out.value["summary"]["last"]
        ),
        "list_notifications" => format!(
            "{} channel(s), {} template(s)",
            out.value["channels"].as_array().map_or(0, Vec::len),
            out.value["templates"].as_array().map_or(0, Vec::len)
        ),
        "describe_node_types" => "catalogue des nœuds".to_string(),
        "validate_flow_graph" => {
            if out.value["valid"].as_bool().unwrap_or(false) {
                "graphe valide".to_string()
            } else {
                format!(
                    "{} violation(s)",
                    out.value["violations"].as_array().map_or(0, Vec::len)
                )
            }
        }
        "validate_calc_expression" => {
            if out.value["valid"].as_bool().unwrap_or(false) {
                "expression valide".to_string()
            } else {
                "expression invalide".to_string()
            }
        }
        "create_flow" => format!(
            "flow #{} « {} » créé (draft v{})",
            out.value["flow_id"], out.value["name"], out.value["version"]
        ),
        "update_flow" => format!(
            "flow #{} — version {} enregistrée (draft)",
            out.value["flow_id"], out.value["version"]
        ),
        _ => "ok".to_string(),
    }
}

// ─────────────────────────── Outils : devices ───────────────────────────

/// Cap défensif du listing devices (le contexte du prompt en liste déjà
/// quelques-uns ; ici c'est la vue complète mais bornée).
const DEVICES_CAP: usize = 50;
/// Cap du listing flows.
const FLOWS_CAP: usize = 20;

async fn list_devices(deps: &ToolDeps<'_>) -> Result<ToolOutcome, String> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .order_by_asc(device_registries::Column::Id)
        .find_also_related(predefined_devices::Entity)
        .all(deps.db)
        .await
        .map_err(|e| format!("lecture devices: {e}"))?;
    let devices: Vec<Value> = rows
        .into_iter()
        .take(DEVICES_CAP)
        .map(|(d, pre)| {
            json!({
                "id": d.id,
                "slug": d.device_id,
                "model": pre.map(|p| p.name),
                "active": d.active,
            })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "devices": devices }),
        flow_id: None,
    })
}

async fn get_device_pins(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let device_id = arg_i64(args, "device_id")?;
    // Scoping org : le device doit appartenir à l'org (404 équivalent).
    let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .filter(device_registries::Column::Id.eq(device_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("lecture device: {e}"))?
    else {
        return Err(format!(
            "device #{device_id} inconnu dans cette organisation"
        ));
    };
    let pins = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
        .order_by_asc(device_capability_instances::Column::Gpio)
        .all(deps.db)
        .await
        .map_err(|e| format!("lecture pins: {e}"))?;
    Ok(ToolOutcome {
        value: json!({
            "device_id": device.id,
            "slug": device.device_id,
            "pins": pins.iter().map(|p| json!({
                "gpio": p.gpio,
                "label": p.label,
                "mode": p.mode,
            })).collect::<Vec<_>>(),
        }),
        flow_id: None,
    })
}

// ─────────────────────────── Outils : flows ───────────────────────────

async fn list_flows(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let status_filter = args.get("status").and_then(Value::as_str);
    let rows = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .order_by_desc(flows::Column::Id)
        .all(deps.db)
        .await
        .map_err(|e| format!("lecture flows: {e}"))?;
    let page_rows: Vec<&flows::Model> = rows
        .iter()
        .filter(|f| status_filter.is_none_or(|s| f.status == s))
        .take(FLOWS_CAP)
        .collect();
    let ids: Vec<i64> = page_rows.iter().map(|f| f.id).collect();
    let latest: std::collections::HashMap<i64, i64> = if ids.is_empty() {
        Default::default()
    } else {
        flow_versions::Entity::find()
            .select_only()
            .column(flow_versions::Column::FlowId)
            .column_as(flow_versions::Column::VersionNumber.max(), "latest")
            .filter(flow_versions::Column::FlowId.is_in(ids))
            .group_by(flow_versions::Column::FlowId)
            .into_tuple::<(i64, i64)>()
            .all(deps.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let flows_json: Vec<Value> = page_rows
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "name": f.name,
                "status": f.status,
                "latest_version_number": latest.get(&f.id).copied().unwrap_or(0),
                "deployed": f.deployed_version_id.is_some(),
            })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "flows": flows_json }),
        flow_id: None,
    })
}

async fn get_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let flow_id = arg_i64(args, "flow_id")?;
    let Some(flow) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("lecture flow: {e}"))?
    else {
        return Err(format!("flow #{flow_id} inconnu dans cette organisation"));
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(deps.db)
        .await
        .map_err(|e| format!("lecture version: {e}"))?
    else {
        return Err(format!("flow #{flow_id} sans version (état incohérent)"));
    };
    Ok(ToolOutcome {
        value: json!({
            "flow": {
                "id": flow.id,
                "name": flow.name,
                "status": flow.status,
                "latest_version_number": version.version_number,
                "graph": version.graph,
            }
        }),
        flow_id: None,
    })
}

async fn query_telemetry(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let metric = arg_str(args, "metric")?;
    let device_slug = arg_str(args, "device_id")?;
    let window = arg_str(args, "window")?;
    let resp = crate::services::visualization::series_points(
        deps.db,
        deps.o2,
        deps.org_id,
        &metric,
        &device_slug,
        &window,
    )
    .await
    .map_err(|e| {
        // Les 400 anti-injection/nom invalide deviennent une erreur outil
        // exploitable par le modèle ; les autres erreurs aussi (jamais de 500 brut).
        e.to_string()
    })?;
    let points: Vec<Value> = resp
        .points
        .iter()
        .map(|p| json!({ "t": p.ts, "v": p.value }))
        .collect();
    let values: Vec<f64> = resp.points.iter().map(|p| p.value).collect();
    let summary = if values.is_empty() {
        json!(null)
    } else {
        let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let last = values[values.len() - 1];
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        json!({ "min": min, "max": max, "last": last, "mean": mean })
    };
    Ok(ToolOutcome {
        value: json!({
            "available": resp.available,
            "metric": resp.metric,
            "device_id": resp.device_id,
            "points": points,
            "summary": summary,
        }),
        flow_id: None,
    })
}

// ─────────────────── Outils statiques (pnex-core) ───────────────────

/// Documentation statique des nœuds — doct elle est construite depuis les
/// formes de config réelles de `pnex-core/src/flow.rs` (un test vérifie que
/// chaque variante de `FlowNodeKind` est mentionnée).
/// Channels and templates of the org, as references for a pnex_notify node.
/// Channel settings (tokens, topics, webhooks) are never exposed to the model.
async fn list_notifications(deps: &ToolDeps<'_>) -> Result<ToolOutcome, String> {
    let channels = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_channels::Column::Name)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading notification channels: {e}"))?;
    let templates = notify_templates::Entity::find()
        .filter(notify_templates::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_templates::Column::Name)
        .all(deps.db)
        .await
        .map_err(|e| format!("reading notification templates: {e}"))?;
    let channels: Vec<Value> = channels
        .iter()
        .map(|c| json!({ "id": c.id, "name": c.name, "kind": c.kind, "enabled": c.enabled }))
        .collect();
    let templates: Vec<Value> = templates
        .iter()
        .map(|t| {
            let vars: Vec<Value> = t
                .vars
                .as_array()
                .map(|vs| vs.iter().filter_map(|v| v.get("name").cloned()).collect())
                .unwrap_or_default();
            json!({ "id": t.id, "name": t.name, "subject": t.subject, "vars": vars })
        })
        .collect();
    Ok(ToolOutcome {
        value: json!({ "channels": channels, "templates": templates }),
        flow_id: None,
    })
}

fn describe_node_types() -> Result<ToolOutcome, String> {
    Ok(ToolOutcome {
        value: json!({
            "pipeline": "[inject] → [device] → [calc] → [metric]",
            "payload_key_rule": "les variables du nœud calc sont les clés du payload produit par le nœud device : sanitize(device_slug) + \"_\" + sanitize(pin_label), CASSE CONSERVÉE (pnex_core::device_payload_key, ex. device \"proud-puffin\" + pin \"A0\" → \"proud_puffin_A0\" — pin \"a0\" serait rejeté : calc_case_mismatch)",
            "metric_rule": "le nœud metric écrit la série etl_<nom_sanitisé> avec device_id=\"flow_<id>\" (pnex_core::etl_metric_name) ; un payload objet écrit une série etl_<nom>_<champ> par champ numérique",
            "nodes": [
                {
                    "kind": "inject",
                    "description": "Déclencheur — un flow doit toujours en avoir un (sinon violation no_trigger).",
                    "config": {
                        "repeat_secs": "nombre, secondes entre chaque injection (ex. 30 = toutes les 30 s) — IDIOME PAR DÉFAUT : un flow tourne en continu",
                        "cron": "expression cron 5-6 champs (alternative à repeat_secs)",
                        "once_delay_secs": "nombre, injection unique après ce délai (exception ponctuelle, éviter)",
                        "topic": "chaîne optionnelle",
                        "payload": "valeur JSON injectée (null = timestamp)"
                    }
                },
                {
                    "kind": "device_read",
                    "description": "Lit les dernières valeurs des pins d'UN SEUL device (série O2, PromQL last_over_time). Sorties : un port par pin (payload {device_pin: valeur}) + un port final « tout » avec l'objet combiné clé→valeur (pour croiser plusieurs pins du même device dans un calc).",
                    "config": {
                        "device_id": "slug du device — un seul device par nœud (cohérence physique)",
                        "pins": "[label de pin (ex. \"A0\", \"D1\")] — un port de sortie par pin, ordre = ordre des ports",
                        "window_secs": "fenêtre de fraîcheur 1..=3600 (défaut 60) ; pas de donnée = payload {} (jamais de zéro inventé)"
                    }
                },
                {
                    "kind": "device_write",
                    "description": "Écrit les pins output d'UN SEUL device (digital 1/0, pwm duty 0-100). Payload entrant = map {pin: valeur} : seules les pins configurées et présentes dans la map sont écrites ; sortie passthrough.",
                    "config": {
                        "device_id": "slug du device — un seul device par nœud",
                        "pins": "[label de pin output (digital_out/pwm_out)] — ancres visuelles, une par pin"
                    }
                },
                {
                    "kind": "calc",
                    "description": "Applique une expression arithmétique au payload (opérateurs, ternaire, ^, fonctions min/max/abs/round…). Valider avec validate_calc_expression.",
                    "config": { "expression": "chaîne (variables = clés du payload device, casse exacte : le pin \"A0\" donne la variable …_A0, jamais …_a0)" }
                },
                {
                    "kind": "value",
                    "description": "Remplace le payload par une valeur fixe (mode static) ou un nombre aléatoire uniforme dans [min, max] (mode random). Transformateur : le déclencheur reste un nœud inject amont.",
                    "config": {
                        "mode": "\"static\" (défaut) | \"random\"",
                        "value": "valeur JSON quelconque (mode static ; null = non renseigné → violation value_static_missing)",
                        "min": "borne inférieure incluse (mode random, défaut 0)",
                        "max": "borne supérieure incluse (mode random, défaut 10)"
                    }
                },
                {
                    "kind": "metric",
                    "description": "Écrit le payload dans OpenObserve comme une métrique (série etl_<nom>, device virtuel flow_<id>).",
                    "config": { "metric_name": "chaîne — le préfixe etl_ et la sanitisation sont automatiques" }
                },
                {
                    "kind": "pnex_notify",
                    "description": "Sends a message template to notification channels (in-app websocket, ntfy, webhook, Telegram, Slack, Discord, SMTP). Get the UUIDs with list_notifications. The node has named input rows: a MANDATORY boolean `trigger` row (the message is sent while it is true — e.g. a calc `x_A0 > 500`) and one row per template variable (the payload arriving on that row fills the variable). Wiring a row = the source node lists this node in its outputs[port].targets AND this node declares the row in its own `inputs`.",
                    "config": {
                        "channel_ids": "[channel UUID] — at least one, from list_notifications",
                        "template_id": "template UUID, from list_notifications",
                        "template_vars": "[variable name] — exactly the template's vars, one input row each",
                        "strict": "bool (default false: a failed send is logged and the message passes through)",
                        "anti_spam": "optional {max_msgs, window_secs} rate limit (e.g. {\"max_msgs\": 3, \"window_secs\": 600})"
                    },
                    "inputs": "[{\"pin\": \"trigger\", \"from\": \"<source node id>\", \"from_port\": <port>}, {\"pin\": \"<var name>\", \"from\": …, \"from_port\": …}] — the trigger row is required (a flow with an unwired trigger is refused)"
                },
                {
                    "kind": "debug",
                    "description": "Capture du message (panneau Debug de l'éditeur).",
                    "config": { "active": "bool (défaut true)", "complete": "\"payload\" (défaut) ou \"true\" = message entier", "console": "bool" }
                },
                {
                    "kind": "display",
                    "description": "Sonde live sous le nœud dans l'éditeur (badge de valeur) — aucun champ de config saisi.",
                    "config": {}
                },
                {
                    "kind": "red",
                    "description": "Raw Node-RED node (type_name + free config) for unmodelled builtins. Only pure transforms are allowed: change, switch, range, rbe, delay, trigger, json, csv, yaml, split, join, sort, batch, link in/out/call, catch, status, complete, comment, junction, inject, debug. Avoid unless needed.",
                    "config": { "type_name": "type Node-RED natif", "config": "objet libre" }
                },
                {
                    "kind": "http_fetch",
                    "description": "Requête HTTP client configurable (type curl) : la réponse remplace msg.payload (JSON auto-parsé si content-type json, sinon texte) + msg.statusCode. Collecte web courante — pour les pages JS lourdes, voir la spec extension (C1b).",
                    "config": {
                        "url": "chaîne requise, http(s)://… (query inclus, pas de templating)",
                        "method": "\"get\" (défaut) | \"post\"",
                        "headers": "[{name, value}] optionnels",
                        "auth": "{\"mode\": \"none\"} (défaut) | {\"mode\": \"basic\", username, password} | {\"mode\": \"bearer\", token} | {\"mode\": \"header\", name, value}",
                        "proxy": "{\"mode\": \"none\"} (défaut) | {\"mode\": \"custom\", url, username?, password?} — providers de scraping : proxy + creds ou clé dans l'URL cible",
                        "timeout_secs": "1..=300 (défaut 30)",
        "body": "corps littéral (POST) ; absent = payload entrant (chaîne → brut, autre → JSON)",
                        "on_error": "\"reject\" (défaut) | \"passthrough\" (payload null + statusCode + http_error)"
                    }
                },
                {
                    "kind": "camera_source",
                    "description": "Event-driven source (no inject needed): one message per frame of a camera device. payload = {device_id, seq, ts_ms, width, height, size, frame_key} — a reference to the JPEG, never the bytes. Live view needs no flow.",
                    "config": {
                        "device_id": "camera device slug (required)",
                        "max_fps": "sampling, 0 = every frame (default), max 25"
                    }
                },
                {
                    "kind": "video_record",
                    "description": "Records camera-source frames into MJPEG-AVI segments stored by the server (fs or S3/RustFS), one buffer per camera; emits one message per stored segment. No video_record node = nothing is stored.",
                    "config": {
                        "segment_secs": "5..=3600 (default 60)",
                        "max_segment_mb": "1..=256 (default 32)",
                        "gap_secs": "flush after this many seconds without frames, 1..=600 (default 10)",
                        "max_fps": "recording rate cap, 0 = every frame (default)",
                        "retention_days": "0 = keep forever, default 7, max 3650",
                        "stream": "logical stream name (default: camera slug)"
                    }
                },
                {
                    "kind": "vision_detect",
                    "description": "Object detection with a registry model (ml_models, e.g. YOLOX COCO: person, car, dog…) on camera_source frames. payload = {device_id, ts_ms, count, labels, detections: [{label, score, bbox:[x,y,w,h]}], frame_key}. Emits only on detection by default.",
                    "config": {
                        "model_id": "ml_models.id (UUID, required)",
                        "labels": "keep only these labels (empty = all)",
                        "min_score": "0..1, overrides the model threshold when > 0",
                        "emit": "\"on_detection\" (default) | \"always\"",
                        "max_fps": "inference rate per camera, default 1, 0 = every frame"
                    }
                },
                {
                    "kind": "event_log",
                    "description": "Stores msg.payload (any JSON) as an event in OpenObserve logs (stream ev_<name>), never in the database; passthrough. Events are searchable on the Events page.",
                    "config": {
                        "stream": "stream label (default \"events\" → ev_events)",
                        "level": "\"debug\" | \"info\" (default) | \"warn\" | \"error\"",
                        "message": "optional short text (≤ 500 chars)"
                    }
                },
                {
                    "kind": "memory_write",
                    "description": "Stores msg.payload (any JSON, e.g. a cool_prop result object) in the org shared memory (Valkey) under a key, with a lifetime; passthrough. Any flow of the org can read it back (memory_read) and dashboards can display its numeric fields (source \"Memory\").",
                    "config": {
                        "key": "[A-Za-z0-9_.-]{1,64}, e.g. \"cycle.p1\" (required)",
                        "ttl_secs": "lifetime in seconds, 1..=2592000, default 3600 (the value disappears if not rewritten)"
                    }
                },
                {
                    "kind": "memory_read",
                    "description": "On each incoming message, reads keys of the org shared memory. Port 0 = object {key: value|null}; then one port per key (payload = value, topic = key; muted when missing or older than max_age_secs).",
                    "config": {
                        "keys": "list of keys (1..=32)",
                        "max_age_secs": "freshness in seconds, 0 (default) = any age"
                    }
                },
                {
                    "kind": "control_source",
                    "description": "Event source fed by org controls (switch, slider, button, number) operated from dashboards and annotations. One output port per listed control, in list order: payload = value (switch 1/0, slider 0..100 = PWM duty by default), topic = control key, msg.control = {id, key, by, via, ts_ms}. Wire it to device_write (one or several devices): a surface never writes a pin itself.",
                    "config": {
                        "controls": "list of org control ids (UUID, 1..=32, must exist in the org at deploy)",
                        "emit_on_start": "bool, default false: resend each control's last value at engine start / redeploy"
                    }
                },
                {
                    "kind": "weather",
                    "description": "Timed weather source (no input) for the coordinates, from an allowlisted provider. Port 0 = current conditions {temperature, feels_like, humidity, pressure, wind_speed (km/h), wind_gust, wind_direction, precipitation, cloud_cover, condition, condition_code, icon, is_day}; port 1 = 7-day forecast {days: [...], d0_t_min, d0_t_max, d0_precipitation, d0_condition_code, ... d6_*}; port 2 = 48-hour forecast {hours: [...], h0_temperature ... h23_*}. Wire to memory_write (live values for dashboards) and/or metric (object payload = one series per numeric field).",
                    "config": {
                        "provider": "\"met_norway\" (default, CC BY 4.0, commercial use allowed) | \"open_meteo\" (non-commercial use only)",
                        "latitude": "-90..=90",
                        "longitude": "-180..=180",
                        "interval_min": "refresh in minutes, 10..=1440 (default 30)",
                        "emit_on_start": "bool, default true: fetch at engine start / redeploy"
                    }
                },
                {
                    "kind": "anomaly",
                    "description": "Flags unusual values of a numeric series without a fixed threshold (one series per msg.topic; payload = number, or object + key). History persists across redeploys. Port 0 = {value, anomaly, score, expected, lower, upper, warming_up, samples}; port 1 = boolean anomaly state (wire it to a pnex_notify trigger).",
                    "config": {
                        "method": "\"robust_z\" (default: median/MAD outlier) | \"forecast_band\" (outside the one-step ETS forecast band, follows trends/seasons) | \"changepoint\" (level/variance regime change, fires once per change)",
                        "key": "object payload field (empty = the payload is the number)",
                        "window": "history samples per series, 16..=5000 (default 200)",
                        "min_samples": "warm-up before scoring, 8..=window (default 30)",
                        "threshold": "robust_z only: |z| alarm level (default 3.5)",
                        "level": "forecast_band only: interval level 0.5..0.999 (default 0.99)",
                        "season_length": "forecast_band only: samples per cycle, 0 = none (warm-up ≥ 2 seasons)",
                        "hazard": "changepoint only: expected samples between changes (default 250)"
                    }
                },
                {
                    "kind": "forecast",
                    "description": "Forecasts a numeric series (one per msg.topic) and, with a threshold, predicts when it will be crossed — predictive maintenance (wear, fouling, slow drift). One step = the median sampling interval. Port 0 = {value, points:[{ts, mean, lower, upper}], breach, breach_in_secs, breach_at, earliest_breach_in_secs, step_secs}; port 1 = boolean breach (notify trigger); port 2 = seconds until the predicted breach.",
                    "config": {
                        "model": "\"ets\" (default: exponential smoothing, MSTL when season_length > 0) | \"linear\" (least-squares trend, best for slow wear)",
                        "key": "object payload field (empty = the payload is the number)",
                        "window": "history samples per series, 16..=5000 (default 500)",
                        "min_samples": "warm-up, 8..=window (default 48)",
                        "horizon": "steps ahead, 1..=1000 (default 300; one step = the median sampling interval)",
                        "season_length": "ets only: samples per cycle, 0 = none",
                        "level": "interval level 0.5..0.999 (default 0.95)",
                        "threshold": "optional breach level (null = forecast only)",
                        "direction": "\"above\" (default) | \"below\"",
                        "every": "refit every N samples, 1..=1000 (default 1)"
                    }
                }
            ],
            "example": {
                "description": "« lecture A0 sur device X → formule → écriture O2 toutes les 2 s »",
                "note": "Les ids de nœuds sont des chaînes libres uniques ; outputs = [{port, targets: [ids des nœuds suivants]}].",
                "graph": {
                    "nodes": [
                        {"id": "n1", "kind": "inject", "config": {"repeat_secs": 2}, "outputs": [{"port": 0, "targets": ["n2"]}]},
                        {"id": "n2", "kind": "device", "config": {"reads": [{"device_id": "X", "pin": "A0"}]}, "outputs": [{"port": 0, "targets": ["n3"]}]},
                        {"id": "n3", "kind": "calc", "config": {"expression": "x_A0 * 0.01"}, "outputs": [{"port": 0, "targets": ["n4"]}]},
                        {"id": "n4", "kind": "metric", "config": {"metric_name": "x_volt"}, "outputs": [{"port": 0, "targets": []}]}
                    ]
                }
            },
            "device_payload_key_examples": ["soil_sensor_A0", "d1_mini_D1"]
        }),
        flow_id: None,
    })
}

fn validate_flow_graph(args: &Value) -> Result<ToolOutcome, String> {
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("graphe illisible (FlowGraph attendu): {e}"))?;
    let violations = pnex_core::validate_graph(&graph);
    Ok(ToolOutcome {
        value: json!({
            "valid": violations.is_empty(),
            "violations": violations,
        }),
        flow_id: None,
    })
}

fn validate_calc_expression(args: &Value) -> Result<ToolOutcome, String> {
    let expression = arg_str(args, "expression")?;
    let errors: Vec<String> = pnex_core::validate_calc(&expression)
        .into_iter()
        .map(|e| e.to_string())
        .collect();
    Ok(ToolOutcome {
        value: json!({ "valid": errors.is_empty(), "errors": errors }),
        flow_id: None,
    })
}

// ─────────────────── Outils d'écriture (drafts uniquement) ───────────────────

async fn create_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    if !deps.can_write {
        return Err(
            "réservé aux rôles owner/admin/member — l'assistant ne peut pas créer de flow pour vous"
                .into(),
        );
    }
    let name = arg_str(args, "name")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("graphe illisible (FlowGraph attendu): {e}"))?;
    let device_id = args.get("device_id").and_then(Value::as_i64);
    let (flow, version, _) = flow::create_flow(
        deps.db,
        deps.org_id,
        &name,
        &graph,
        device_id,
        deps.author.clone(),
        Some("créé par l'assistant IA".into()),
        &ASSISTANT_WRITER,
    )
    .await
    .map_err(flow_write_error_string)?;
    Ok(ToolOutcome {
        value: json!({
            "flow_id": flow.id,
            "name": flow.name,
            "status": flow.status,
            "version": version,
        }),
        flow_id: Some(flow.id),
    })
}

async fn update_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    if !deps.can_write {
        return Err(
            "réservé aux rôles owner/admin/member — l'assistant ne peut pas modifier de flow pour vous"
                .into(),
        );
    }
    let flow_id = arg_i64(args, "flow_id")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("graphe illisible (FlowGraph attendu): {e}"))?;
    let note = args.get("note").and_then(Value::as_str).map(str::to_string);
    // Scoping org : le flow doit appartenir à l'org.
    let Some(row) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(|e| format!("lecture flow: {e}"))?
    else {
        return Err(format!("flow #{flow_id} inconnu dans cette organisation"));
    };
    // Concurrence optimiste : l'agent travaille sur la dernière version
    // connue du serveur ; un éditeur humain entre-temps → 409 rendu au
    // modèle (qui recharge avec get_flow).
    let latest = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(row.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(deps.db)
        .await
        .map_err(|e| format!("lecture version: {e}"))?
        .map(|v| v.version_number)
        .unwrap_or(0);
    let (_, version, _) = flow::append_version(
        deps.db,
        &row,
        latest,
        &graph,
        None,
        deps.author.clone(),
        note.or_else(|| Some("modifié par l'assistant IA".into())),
        &ASSISTANT_WRITER,
    )
    .await
    .map_err(flow_write_error_string)?;
    Ok(ToolOutcome {
        value: json!({ "flow_id": row.id, "version": version }),
        flow_id: Some(row.id),
    })
}

/// Erreurs d'écriture → messages lisibles par le modèle (violations
/// détaillées : le modèle se corrige tout seul).
fn flow_write_error_string(e: flow::FlowWriteError) -> String {
    use crate::services::flow::FlowWriteError as E;
    match e {
        E::Violations(v) => format!(
            "graphe invalide: {}",
            serde_json::to_string(&v).unwrap_or_default()
        ),
        E::NameRequired => "nom de flow requis".into(),
        E::NameTooLong => "nom de flow trop long (> 200 caractères)".into(),
        E::DeviceUnknown => "device_id inconnu dans cette organisation".into(),
        E::Conflict { current, .. } => format!(
            "conflit de version : le flow est désormais en version {current} — rechargez-le avec get_flow puis réessayez"
        ),
        E::Db => "erreur base de données".into(),
        E::Secret(e) => format!("secret field refused: {e}"),
    }
}

/// The assistant never types a secret value: it may only keep or pick
/// vault references (secrets.md D117).
const ASSISTANT_WRITER: crate::services::secrets::flow::GraphWriter<'static> =
    crate::services::secrets::flow::GraphWriter {
        ring: None,
        user_id: None,
        can_write_secrets: false,
    };

#[cfg(test)]
mod tests {
    use super::*;

    /// Le registre est **exactement** l'ensemble autorisé : c'est l'assertion
    /// structurelle du garde-fou « pas de deploy / suppression / commandes ».
    #[test]
    fn registre_ne_contient_que_des_outils_autorises() {
        let names: Vec<&str> = tool_specs().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "list_devices",
                "get_device_pins",
                "list_flows",
                "get_flow",
                "list_notifications",
                "query_telemetry",
                "describe_node_types",
                "validate_flow_graph",
                "validate_calc_expression",
                "create_flow",
                "update_flow",
            ]
        );
        // Aucun nom ne suggère une action interdite — assertion en profondeur
        // (un futur « deploy_flow » ajouté à tool_specs casserait ce test).
        for name in &names {
            for forbidden in [
                "deploy",
                "delete",
                "remove",
                "command",
                "write_gpio",
                "http",
                "sql_exec",
            ] {
                assert!(
                    !name.contains(forbidden),
                    "outil interdit détecté dans le registre: {name}"
                );
            }
        }
    }

    /// `execute` refuse tout ce qui n'est pas le registre — y compris des
    /// noms tentants (« deploy_flow », « run_sql », « http »).
    #[tokio::test]
    async fn execute_refuse_les_noms_hors_registre() {
        let db = sea_orm::Database::connect("sqlite::memory:")
            .await
            .expect("sqlite mémoire");
        let deps = ToolDeps {
            db: &db,
            org_id: 1,
            can_write: true,
            author: None,
            o2: None,
        };
        for name in [
            "deploy_flow",
            "delete_flow",
            "delete_device",
            "send_device_command",
            "http",
            "run_sql",
        ] {
            let err = execute(&deps, name, &serde_json::json!({}))
                .await
                .expect_err("outil interdit doit échouer");
            assert!(err.contains("outil inconnu"), "{name} → {err}");
        }
        // Un outil de lecture simple fonctionne sans rien d'autre.
        let out = execute(&deps, "describe_node_types", &serde_json::json!({}))
            .await
            .expect("describe_node_types statique");
        assert!(out.value["nodes"].as_array().expect("nodes").len() >= 8);
    }

    /// Le catalogue cité au modèle mentionne toutes les variantes de
    /// `FlowNodeKind` (sinon l'agent inventerait des nœuds inexistants).
    #[test]
    fn catalogue_cite_toutes_les_variantes_de_flow_node_kind() {
        let out = describe_node_types().expect("catalogue");
        let text = serde_json::to_string(&out.value).expect("sérialisable");
        for kind in [
            "inject",
            "device_read",
            "device_write",
            "calc",
            "metric",
            "debug",
            "display",
            "red",
            "camera_source",
            "video_record",
            "vision_detect",
            "event_log",
            "memory_write",
            "memory_read",
            "control_source",
            "weather",
            "anomaly",
            "forecast",
        ] {
            assert!(
                text.contains(&format!("\"kind\": \"{kind}\"")) || text.contains(kind),
                "variante {kind} absente du catalogue"
            );
        }
    }
}
