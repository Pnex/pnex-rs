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
    /// App config (live caches, flow cluster); `None` in unit tests.
    pub config: Option<&'a loco_rs::config::Config>,
}

/// Résultat d'exécution : la valeur JSON vue par le modèle + l'id du flow
/// touché (pour le deep-link « Ouvrir dans l'éditeur » côté UI).
#[derive(Debug)]
pub struct ToolOutcome {
    pub value: Value,
    /// Positionné par create_flow/update_flow.
    pub flow_id: Option<i64>,
}

/// Refusal of a tool. `message` is what the model reads; `code`/`args`
/// let the UI render the refusal in the user's language (`err-<code>`).
#[derive(Debug)]
pub struct ToolError {
    pub message: String,
    pub code: Option<&'static str>,
    pub args: Option<Value>,
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self {
            message,
            code: None,
            args: None,
        }
    }
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
    /// Machine code of a coded refusal (`ai-flow-running`…).
    pub code: Option<&'static str>,
    /// Interpolation data of `code`.
    pub args: Option<Value>,
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
            description: "Documentation of every flow node kind (purpose, config fields, ports, pitfalls), the authoring rules and a valid example graph. Read it before creating or updating a flow; pass `kinds` to fetch only some kinds.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kinds": {"type": "array", "items": {"type": "string"}, "description": "optional subset of node kinds, e.g. [\"device_read\", \"calc\"]"}
                },
                "required": []
            }),
        },
        ToolSpec {
            name: "search_knowledge",
            description: "Searches the PneX knowledge cards (features, how-tos, troubleshooting) embedded in the server. Use it before answering how something works, where a setting is, or why something fails; then read_knowledge for the full card. Answers must describe gestures in the UI only.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "keywords, e.g. \"device no telemetry\""},
                    "limit": {"type": "integer", "description": "1..=5 (default 5)"}
                },
                "required": ["query"]
            }),
        },
        ToolSpec {
            name: "read_knowledge",
            description: "Full text of one knowledge card (id from search_knowledge).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "card id"}
                },
                "required": ["id"]
            }),
        },
        ToolSpec {
            name: "diagnose_device",
            description: "Read-only diagnosis of one device of the organization: online state and last seen, pins (mode, subscription period, last value), firmware version, last OTA update, hints. id = internal id from list_devices.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "device_id": {"type": "integer", "description": "internal device id (list_devices)"}
                },
                "required": ["device_id"]
            }),
        },
        ToolSpec {
            name: "diagnose_flow",
            description: "Read-only diagnosis of one flow: status, last saved vs deployed version, engine state and last engine error, last message of each debug/display node, hints.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "flow_id": {"type": "integer", "description": "internal flow id (list_flows)"}
                },
                "required": ["flow_id"]
            }),
        },
        ToolSpec {
            name: "list_dashboards",
            description: "Lists the organization's dashboards (id, name, current version).",
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
        },
        ToolSpec {
            name: "get_dashboard",
            description: "Current layout of a dashboard (format, canvas, widgets, mobile sections/pages) and its version. `coupled_flows` maps each widget whose control feeds a DEPLOYED flow to those flows: such a widget can only be moved or resized until the user stops the flows.",
            input_schema: json!({
                "type": "object",
                "properties": {"dashboard_id": {"type": "string", "description": "dashboard UUID (list_dashboards)"}},
                "required": ["dashboard_id"]
            }),
        },
        ToolSpec {
            name: "validate_dashboard_layout",
            description: "Checks a dashboard layout WITHOUT saving: validation violations and, with dashboard_id, the changes refused on coupled widgets. Call it before create_dashboard/update_dashboard and warn the user about coupled widgets first.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "layout": {"type": "object", "description": "complete DashboardLayout"},
                    "dashboard_id": {"type": "string", "description": "optional: compare with this dashboard's current layout"}
                },
                "required": ["layout"]
            }),
        },
        ToolSpec {
            name: "create_dashboard",
            description: "Creates a dashboard (version 1, live immediately). The format (pc or mobile) is fixed at creation. A control widget DECLARES a new control (it never operates anything): wiring it to a device takes a control_source flow the user deploys.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "description": {"type": "string"},
                    "layout": {"type": "object", "description": "optional DashboardLayout (default: empty desktop canvas)"}
                },
                "required": ["name"]
            }),
        },
        ToolSpec {
            name: "update_dashboard",
            description: "Saves a new version of a dashboard — LIVE immediately (no draft). Pass the version read with get_dashboard as expected_version (conflict → reload and redo). Widgets coupled to a deployed flow can only be moved or resized: removing, re-binding, retyping, re-specifying or renaming them is refused with ai-flow-running listing the flows to stop. No deletion.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dashboard_id": {"type": "string"},
                    "expected_version": {"type": "integer"},
                    "layout": {"type": "object", "description": "complete DashboardLayout of the new version"},
                    "name": {"type": "string", "description": "optional new name"}
                },
                "required": ["dashboard_id", "expected_version", "layout"]
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
            description: "Saves a NEW VERSION of an existing flow (append-only, never deployed). Only a stopped or never-deployed flow can be edited: a deployed flow is refused with ai-flow-running — ask the user to stop it in the flow editor first. Pass the latest_version_number read with get_flow as expected_version; on a version conflict, reload with get_flow and redo the change.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "flow_id": {"type": "integer", "description": "id interne du flow (list_flows)"},
                    "expected_version": {"type": "integer", "description": "latest_version_number returned by get_flow"},
                    "graph": {"type": "object", "description": "graphe FlowGraph complet de la nouvelle version"},
                    "note": {"type": "string", "description": "note de version optionnelle"}
                },
                "required": ["flow_id", "expected_version", "graph"]
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
pub async fn execute(
    deps: &ToolDeps<'_>,
    name: &str,
    args: &Value,
) -> Result<ToolOutcome, ToolError> {
    match name {
        "list_devices" => list_devices(deps).await.map_err(Into::into),
        "get_device_pins" => get_device_pins(deps, args).await.map_err(Into::into),
        "list_flows" => list_flows(deps, args).await.map_err(Into::into),
        "get_flow" => get_flow(deps, args).await.map_err(Into::into),
        "query_telemetry" => query_telemetry(deps, args).await.map_err(Into::into),
        "list_notifications" => list_notifications(deps).await.map_err(Into::into),
        "describe_node_types" => describe_node_types(args).map_err(Into::into),
        "search_knowledge" => search_knowledge(args).map_err(Into::into),
        "read_knowledge" => read_knowledge(args).map_err(Into::into),
        "diagnose_device" => diagnose_device(deps, args).await.map_err(Into::into),
        "diagnose_flow" => diagnose_flow(deps, args).await.map_err(Into::into),
        "validate_flow_graph" => validate_flow_graph(args).map_err(Into::into),
        "validate_calc_expression" => validate_calc_expression(args).map_err(Into::into),
        "create_flow" => create_flow(deps, args).await.map_err(Into::into),
        "update_flow" => update_flow(deps, args).await,
        "list_dashboards" => super::dashboard_tools::list_dashboards(deps).await,
        "get_dashboard" => super::dashboard_tools::get_dashboard(deps, args).await,
        "validate_dashboard_layout" => {
            super::dashboard_tools::validate_dashboard_layout(deps, args).await
        }
        "create_dashboard" => super::dashboard_tools::create_dashboard(deps, args).await,
        "update_dashboard" => super::dashboard_tools::update_dashboard(deps, args).await,
        _ => Err(format!(
            "outil inconnu: {name} — seuls les outils listés dans la conversation sont disponibles"
        )
        .into()),
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
        "search_knowledge" => format!(
            "{} card(s)",
            out.value["results"].as_array().map_or(0, Vec::len)
        ),
        "read_knowledge" => format!("card {}", out.value["id"]),
        "diagnose_device" => format!(
            "device {} — {} hint(s)",
            out.value["device"]["slug"],
            out.value["hints"].as_array().map_or(0, Vec::len)
        ),
        "diagnose_flow" => format!(
            "flow #{} — {} hint(s)",
            out.value["flow"]["id"],
            out.value["hints"].as_array().map_or(0, Vec::len)
        ),
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
        "list_dashboards" => format!(
            "{} dashboard(s)",
            out.value["dashboards"].as_array().map_or(0, Vec::len)
        ),
        "get_dashboard" => format!(
            "dashboard « {} » v{}",
            out.value["dashboard"]["name"].as_str().unwrap_or_default(),
            out.value["dashboard"]["version"]
        ),
        "validate_dashboard_layout" => {
            if out.value["valid"].as_bool().unwrap_or(false) {
                "layout valid".to_string()
            } else {
                "layout refused".to_string()
            }
        }
        "create_dashboard" => format!(
            "dashboard « {} » created (live v{})",
            out.value["name"].as_str().unwrap_or_default(),
            out.value["version"]
        ),
        "update_dashboard" => format!(
            "dashboard v{} saved (live): +{} −{} ~{} widget(s)",
            out.value["version"],
            out.value["widgets_added"].as_array().map_or(0, Vec::len),
            out.value["widgets_removed"].as_array().map_or(0, Vec::len),
            out.value["widgets_changed"].as_array().map_or(0, Vec::len)
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

/// Serializes the node documentation table of `pnex_core` (D142): the
/// assistant's knowledge of node kinds has a single source, guarded there.
/// `kinds` (optional) narrows the answer to save tokens.
fn describe_node_types(args: &Value) -> Result<ToolOutcome, String> {
    let wanted: Option<Vec<&str>> = args
        .get("kinds")
        .and_then(Value::as_array)
        .map(|ks| ks.iter().filter_map(Value::as_str).collect());
    let unknown: Vec<&str> = wanted
        .iter()
        .flatten()
        .copied()
        .filter(|k| pnex_core::node_doc(k).is_none())
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown node kind(s): {} — call describe_node_types without kinds for the full list",
            unknown.join(", ")
        ));
    }
    let nodes: Vec<Value> = pnex_core::NODE_DOCS
        .iter()
        .filter(|d| wanted.as_ref().is_none_or(|w| w.contains(&d.kind)))
        .map(|d| {
            let config: serde_json::Map<String, Value> = d
                .config
                .iter()
                .map(|(field, meaning)| (field.to_string(), json!(meaning)))
                .collect();
            let mut node = json!({ "kind": d.kind, "summary": d.summary, "config": config });
            if !d.notes.is_empty() {
                node["notes"] = json!(d.notes);
            }
            node
        })
        .collect();
    let rules: serde_json::Map<String, Value> = pnex_core::FLOW_AUTHORING_RULES
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    let example: Value =
        serde_json::from_str(pnex_core::FLOW_EXAMPLE).map_err(|e| format!("example: {e}"))?;
    Ok(ToolOutcome {
        value: json!({
            "rules": rules,
            "all_kinds": pnex_core::NODE_DOCS.iter().map(|d| d.kind).collect::<Vec<_>>(),
            "nodes": nodes,
            "example": example,
        }),
        flow_id: None,
    })
}

fn search_knowledge(args: &Value) -> Result<ToolOutcome, String> {
    let query = arg_str(args, "query")?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(super::knowledge::SEARCH_MAX, |l| l as usize);
    let hits = super::knowledge::search(&query, limit);
    Ok(ToolOutcome {
        value: json!({ "results": hits }),
        flow_id: None,
    })
}

fn read_knowledge(args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let card = super::knowledge::card(&id)
        .ok_or_else(|| format!("unknown card {id} — use search_knowledge to find ids"))?;
    Ok(ToolOutcome {
        value: json!({
            "id": card.id,
            "title": card.title,
            "kind": card.kind,
            "pages": card.pages,
            "body": card.body,
        }),
        flow_id: None,
    })
}

async fn diagnose_device(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let device_id = arg_i64(args, "device_id")?;
    let value = super::diagnose::device(deps.db, deps.config, deps.org_id, device_id).await?;
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

async fn diagnose_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, String> {
    let flow_id = arg_i64(args, "flow_id")?;
    let value = super::diagnose::flow(deps.db, deps.config, deps.org_id, flow_id).await?;
    Ok(ToolOutcome {
        value,
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

async fn update_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    if !deps.can_write {
        return Err(
            "réservé aux rôles owner/admin/member — l'assistant ne peut pas modifier de flow pour vous"
                .to_string()
                .into(),
        );
    }
    let flow_id = arg_i64(args, "flow_id")?;
    let expected_version = arg_i64(args, "expected_version")?;
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
        return Err(format!("flow #{flow_id} inconnu dans cette organisation").into());
    };
    // D143: a running flow is frozen for the assistant. Checked here, at
    // execution time, from the stored status — never from the model's word.
    // Stopping stays a human gesture (no stop tool exists).
    if row.status == pnex_core::FLOW_STATUS_DEPLOYED {
        return Err(flow_running_error(&[(row.id, row.name.clone())]));
    }
    // Optimistic concurrency: the model edits the version it read with
    // get_flow; a human save in between → conflict rendered to the model,
    // which reloads instead of overwriting the human's change.
    flow::append_version(
        deps.db,
        &row,
        expected_version,
        &graph,
        None,
        deps.author.clone(),
        note.or_else(|| Some("modifié par l'assistant IA".into())),
        &ASSISTANT_WRITER,
    )
    .await
    .map(|(_, version, _)| ToolOutcome {
        value: json!({ "flow_id": row.id, "version": version }),
        flow_id: Some(row.id),
    })
    .map_err(|e| flow_write_error_string(e).into())
}

/// `ai-flow-running` refusal listing the deployed flows that block the
/// change (D143, D144); `args.flow` = their names for the UI.
pub(super) fn flow_running_error(flows: &[(i64, String)]) -> ToolError {
    let listed: Vec<String> = flows
        .iter()
        .map(|(id, name)| format!("#{id} « {name} »"))
        .collect();
    ToolError {
        message: format!(
            "{}: refused, deployed and running flow(s) {} — ask the user to stop them in the flow editor, then retry. You cannot stop, deploy or delete a flow.",
            pnex_core::err_codes::AI_FLOW_RUNNING,
            listed.join(", ")
        ),
        code: Some(pnex_core::err_codes::AI_FLOW_RUNNING),
        args: Some(json!({
            "flow": flows.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", "),
        })),
    }
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
                "search_knowledge",
                "read_knowledge",
                "diagnose_device",
                "diagnose_flow",
                "list_dashboards",
                "get_dashboard",
                "validate_dashboard_layout",
                "create_dashboard",
                "update_dashboard",
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
            config: None,
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
            assert!(err.message.contains("outil inconnu"), "{name} → {err:?}");
        }
        // Un outil de lecture simple fonctionne sans rien d'autre.
        let out = execute(&deps, "describe_node_types", &serde_json::json!({}))
            .await
            .expect("describe_node_types statique");
        assert!(out.value["nodes"].as_array().expect("nodes").len() >= 8);
    }

    /// The catalogue is the core table: every documented kind is served,
    /// and the `kinds` filter narrows it (the core guard checks the table
    /// against `FlowNodeKind`).
    #[test]
    fn catalogue_serves_the_core_node_docs() {
        let out = describe_node_types(&serde_json::json!({})).expect("catalogue");
        let nodes = out.value["nodes"].as_array().expect("nodes");
        assert_eq!(nodes.len(), pnex_core::NODE_DOCS.len());
        assert!(out.value["example"]["nodes"].is_array());

        let out = describe_node_types(&serde_json::json!({"kinds": ["calc", "reg_pid"]}))
            .expect("subset");
        let kinds: Vec<&str> = out.value["nodes"]
            .as_array()
            .expect("nodes")
            .iter()
            .filter_map(|n| n["kind"].as_str())
            .collect();
        assert_eq!(kinds, vec!["calc", "reg_pid"]);

        let err = describe_node_types(&serde_json::json!({"kinds": ["device"]}))
            .expect_err("removed kind");
        assert!(err.contains("unknown node kind"), "{err}");
    }
}
