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
    /// Who writes through the ontology tools (role for D188, provenance
    /// `assistant:<user>` for D184); `None` in unit tests.
    pub actor: Option<crate::services::ontology::Actor>,
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

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// Role guard of every writing tool, re-checked server side (R2): a viewer
/// gets a coded refusal the UI renders in the user's language.
pub(super) fn require_write(deps: &ToolDeps<'_>) -> Result<(), ToolError> {
    if deps.can_write {
        return Ok(());
    }
    Err(ToolError {
        message: "owner, admin or member role required: the assistant cannot change this for you"
            .to_string(),
        code: Some(pnex_core::err_codes::AI_WRITE_FORBIDDEN),
        args: None,
    })
}

/// Internal failure of a tool (database, store): the detail is logged, the
/// model and the UI only get a generic coded refusal — a database error may
/// carry SQL or identifiers (R4, R16).
pub(super) fn internal<E: std::fmt::Display>(context: &'static str) -> impl FnOnce(E) -> ToolError {
    move |e| {
        tracing::error!(error = %e, context, "assistant tool internal failure");
        ToolError {
            message: format!("internal error while {context}; tell the user to try again later"),
            code: Some(pnex_core::err_codes::AI_TOOL_INTERNAL),
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
    /// Short canonical English summary (verbatim fallback of the UI).
    pub summary: String,
    /// Fluent key of a successful call's summary, rendered by the UI with
    /// `args` (see [`TraceSummary`]).
    pub summary_key: Option<&'static str>,
    /// Flow touché — pilote le bouton « Ouvrir dans l'éditeur ».
    pub flow_id: Option<i64>,
    /// Machine code of a coded refusal (`ai-flow-running`…).
    pub code: Option<&'static str>,
    /// Interpolation data of `code` (refusal) or of `summary_key` (success).
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
            name: "get_notification_template",
            description: "Full notification template (subject, body, declared variables, updated_at).",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"template_id":{"type":"string","description":"template UUID (list_notifications)"}},"required":["template_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "preview_notification_template",
            description: "Renders a template with example variables and an optional msg payload — NEVER sends anything.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"template_id":{"type":"string"},"vars":{"type":"object","description":"{var: value}; missing ones use the declared examples"},"payload":{"description":"simulated msg.payload"}},"required":["template_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "create_notification_template",
            description: "Creates a notification template (minijinja; variables {{ name }} are detected automatically). Sends nothing. Channels are managed by the user only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"name":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"},"vars":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"example":{"type":"string"}}}}},"required":["name","body"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "update_notification_template",
            description: "Rewrites a template in place, on top of the state read with get_notification_template (pass its updated_at as expected_updated_at). Deployed flows keep their snapshot until the user redeploys.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"template_id":{"type":"string"},"expected_updated_at":{"type":"string"},"name":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"},"vars":{"type":"array","items":{"type":"object"}}},"required":["template_id","expected_updated_at","name","body"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "list_functions",
            description: "Lists the organization's JS/Starlark functions (Functions page).",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{},"required":[]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "get_function",
            description: "Latest code and declared @input/@output interface of a function, its version count, and the deployed flows that use it (with their pinned version).",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"function_id":{"type":"integer"}},"required":["function_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "validate_function",
            description: "Compile-only check of JS or Starlark code (no execution): diagnostics with line/column.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"language":{"type":"string","enum":["js","starlark"]},"code":{"type":"string"}},"required":["language","code"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "test_function",
            description: "Runs a function in the sandbox of the Test button (no network, no device, no side effect): its latest saved version, or `code` (unsaved) in the function's language, with typed `inputs` and an optional `msg`.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"function_id":{"type":"integer"},"code":{"type":"string"},"inputs":{"type":"object"},"msg":{"type":"object"}},"required":["function_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "create_function",
            description: "Creates a JS or Starlark function (version 1). Declare its interface with @input/@output directives; check it with validate_function first.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"name":{"type":"string"},"language":{"type":"string","enum":["js","starlark"]},"code":{"type":"string"},"description":{"type":"string"}},"required":["name","language","code"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "update_function",
            description: "Saves a new version of a function (code and/or name/description) on top of the version read with get_function (expected_version = its latest_version). Deployed flows keep their pinned version until the user selects the new one and redeploys.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"function_id":{"type":"integer"},"expected_version":{"type":"integer"},"code":{"type":"string"},"name":{"type":"string"},"description":{"type":"string"},"note":{"type":"string"}},"required":["function_id","expected_version"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "list_annotation_sets",
            description: "Lists the organization's annotation sets (name, published, attached to a media or a tour). Read-only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{},"required":[]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "list_tours",
            description: "Lists the organization's virtual tours (name, mode, published, whether a public link exists). Read-only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{},"required":[]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "list_pois",
            description: "Lists the map points of interest (label, location, coordinates, devices placed there). Read-only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{},"required":[]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "list_controls",
            description: "Controls of the organization (switches, sliders… declared by dashboards and annotations): spec, last commanded value, and the deployed flows listening to each. Read-only: the assistant never operates a control.",
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
        },
        ToolSpec {
            name: "read_memory",
            description: "Shared memory of the organization (written by memory_write flow nodes). Without keys: the keys, their numeric fields and age. With keys (\"key\" or \"key#field\"): their current values. Read-only.",
            input_schema: json!({
                "type": "object",
                "properties": {"keys": {"type": "array", "items": {"type": "string"}}},
                "required": []
            }),
        },
        ToolSpec {
            name: "list_taxonomies",
            description: "Lists the organization's topic taxonomies (Audio streams › Taxonomies): name, current version, topic count and topic ids. Read-only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{},"required":[]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "get_taxonomy",
            description: "One taxonomy with the full topics (id, label, definition, keywords) of its current version. Read it before create_taxonomy_version.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"taxonomy_id":{"type":"string"}},"required":["taxonomy_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "create_taxonomy_version",
            description: "Saves a new version of a taxonomy with the COMPLETE topic list (topics left out are dropped from the new version), on top of the version read with get_taxonomy (expected_version = its current_version). Topic id = stable slug [a-z0-9_]; keywords match whole words, ignoring case and accents. Never rewrites history: deployed flows keep their pinned version until the user picks the new one and redeploys.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"taxonomy_id":{"type":"string"},"expected_version":{"type":"integer"},"topics":{"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"label":{"type":"string"},"definition":{"type":"string"},"keywords":{"type":"array","items":{"type":"string"}}},"required":["id","label"]}},"note":{"type":"string"}},"required":["taxonomy_id","expected_version","topics"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "describe_ontology",
            description: "The organization's ontology schema: object types (key, name, typed properties, containment, write role, object count; system types are devices, media, dashboards, tours, POIs, flows, folders) and link types (key, from/to types, attributes, cardinality). Read it before any ontology query or write. Read-only.",
            input_schema: json!({"type": "object", "properties": {}}),
        },
        ToolSpec {
            name: "query_ontology",
            description: "Declarative ontology query: start set (type_key, ids, text in titles, property filters {key, op: eq|ne|lt|lte|gt|gte|contains|exists, value}, label 'name' or 'name:value', within = descendants of an object) then up to 4 traversals [{link_type, direction: out|in}], evaluated as_of an RFC 3339 instant (default now); latest = series properties whose last value is joined. At most 50 rows. Read-only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"query":{"type":"object","properties":{"type_key":{"type":"string"},"ids":{"type":"array","items":{"type":"string"}},"text":{"type":"string"},"filters":{"type":"array","items":{"type":"object","properties":{"key":{"type":"string"},"op":{"type":"string"},"value":{}},"required":["key","op"]}},"label":{"type":"string"},"within":{"type":"string"},"traverse":{"type":"array","items":{"type":"object","properties":{"link_type":{"type":"string"},"direction":{"type":"string"}},"required":["link_type"]}},"as_of":{"type":"string"},"latest":{"type":"array","items":{"type":"string"}},"limit":{"type":"integer"}}}},"required":["query"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "get_object",
            description: "One ontology object (properties, version) with its links valid now. Read it before update_object.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"}},"required":["object_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "create_object",
            description: "Creates an object of an organization type (not a system type: devices, media… are created from their own pages). properties must follow the type's schema (describe_ontology); series and events properties hold no value.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"type_key":{"type":"string"},"title":{"type":"string"},"properties":{"type":"object"}},"required":["type_key","title"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "update_object",
            description: "Saves the title and the COMPLETE property document of an object (properties left out are cleared) on top of the version read with get_object (expected_version). For a system object only its organization properties change.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"object_id":{"type":"string"},"expected_version":{"type":"integer"},"title":{"type":"string"},"properties":{"type":"object"}},"required":["object_id","expected_version","properties"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "open_link",
            description: "Opens a typed link between two objects (link types from describe_ontology), optionally valid_from a past RFC 3339 instant. To bind a sensor to an object's series property: link_type 'measures', source = the device object, target = the object, attributes {metric, property}.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"link_type":{"type":"string"},"source_id":{"type":"string"},"target_id":{"type":"string"},"attributes":{"type":"object"},"valid_from":{"type":"string"}},"required":["link_type","source_id","target_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "close_link",
            description: "Closes an open link at valid_to (default now). The link stays in the history; nothing is deleted. Replacing a sensor = close its 'measures' link, then open one from the new device.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"link_id":{"type":"integer"},"valid_to":{"type":"string"}},"required":["link_id"]}"#).expect("static schema"),
        },
        ToolSpec {
            name: "save_object_type",
            description: "Creates an organization object type (no expected_version) or saves a new version of one on top of expected_version (from describe_ontology). def = {key [a-z][a-z0-9_]*, name, icon, properties: [{key, name, kind: text|number|bool|date|date_time|enum|geo_point|url|ref|series|events, required, indexed, unit, min, max, values, to_type, stream, max_len}], may_contain / may_be_contained_in: '*' or a list of type keys or null, write_role: member|admin|owner}. For a system type only properties are taken. Organization owners and admins only.",
            input_schema: serde_json::from_str(r#"{"type":"object","properties":{"def":{"type":"object"},"expected_version":{"type":"integer"}},"required":["def"]}"#).expect("static schema"),
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
                    "expression": {"type": "string", "description": "expression (variables = clés du payload device, ex. probe_1_A0)"}
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
        .ok_or_else(|| format!("argument '{key}' missing or not a string"))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("argument '{key}' missing or not an integer"))
}

fn arg_value<'a>(args: &'a Value, key: &str) -> Result<&'a Value, String> {
    args.get(key)
        .ok_or_else(|| format!("argument '{key}' missing"))
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
        "list_devices" => list_devices(deps).await,
        "get_device_pins" => get_device_pins(deps, args).await,
        "list_flows" => list_flows(deps, args).await,
        "get_flow" => get_flow(deps, args).await,
        "query_telemetry" => query_telemetry(deps, args).await,
        "list_notifications" => list_notifications(deps).await,
        "describe_node_types" => describe_node_types(args),
        "search_knowledge" => search_knowledge(args),
        "read_knowledge" => read_knowledge(args),
        "diagnose_device" => diagnose_device(deps, args).await,
        "diagnose_flow" => diagnose_flow(deps, args).await,
        "validate_flow_graph" => validate_flow_graph(args),
        "validate_calc_expression" => validate_calc_expression(args),
        "create_flow" => create_flow(deps, args).await,
        "update_flow" => update_flow(deps, args).await,
        "list_dashboards" => super::dashboard_tools::list_dashboards(deps).await,
        "get_dashboard" => super::dashboard_tools::get_dashboard(deps, args).await,
        "validate_dashboard_layout" => {
            super::dashboard_tools::validate_dashboard_layout(deps, args).await
        }
        "create_dashboard" => super::dashboard_tools::create_dashboard(deps, args).await,
        "update_dashboard" => super::dashboard_tools::update_dashboard(deps, args).await,
        "get_notification_template" => {
            super::more_tools::get_notification_template(deps, args).await
        }
        "preview_notification_template" => {
            super::more_tools::preview_notification_template(deps, args).await
        }
        "create_notification_template" => {
            super::more_tools::create_notification_template(deps, args).await
        }
        "update_notification_template" => {
            super::more_tools::update_notification_template(deps, args).await
        }
        "list_functions" => super::more_tools::list_functions(deps).await,
        "get_function" => super::more_tools::get_function(deps, args).await,
        "validate_function" => super::more_tools::validate_function(deps, args).await,
        "test_function" => super::more_tools::test_function(deps, args).await,
        "create_function" => super::more_tools::create_function(deps, args).await,
        "update_function" => super::more_tools::update_function(deps, args).await,
        "list_annotation_sets" => super::more_tools::list_annotation_layers(deps).await,
        "list_tours" => super::more_tools::list_tours(deps).await,
        "list_pois" => super::more_tools::list_pois(deps).await,
        "list_controls" => super::more_tools::list_controls(deps).await,
        "read_memory" => super::more_tools::read_memory(deps, args).await,
        "list_taxonomies" => super::more_tools::list_taxonomies(deps).await,
        "get_taxonomy" => super::more_tools::get_taxonomy(deps, args).await,
        "create_taxonomy_version" => super::more_tools::create_taxonomy_version(deps, args).await,
        "describe_ontology" => super::ontology_tools::describe_ontology(deps).await,
        "query_ontology" => super::ontology_tools::query_ontology(deps, args).await,
        "get_object" => super::ontology_tools::get_object(deps, args).await,
        "create_object" => super::ontology_tools::create_object(deps, args).await,
        "update_object" => super::ontology_tools::update_object(deps, args).await,
        "open_link" => super::ontology_tools::open_link(deps, args).await,
        "close_link" => super::ontology_tools::close_link(deps, args).await,
        "save_object_type" => super::ontology_tools::save_object_type(deps, args).await,
        _ => Err(format!(
            "unknown tool: {name} — only the tools listed in this conversation are available"
        )
        .into()),
    }
}

/// Short summary of a successful tool call, for the UI trace: `key` is the
/// fluent key the UI renders with `args` (every value a string), `text` the
/// canonical English form (verbatim fallback, and what the history replay
/// shows the model).
pub struct TraceSummary {
    pub key: &'static str,
    pub args: Value,
    pub text: String,
}

/// Length of a JSON array field (0 when absent).
fn count(v: &Value) -> String {
    v.as_array().map_or(0, Vec::len).to_string()
}

/// Display form of a scalar JSON field (strings unquoted, absent = "?").
fn shown(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "?".to_string(),
        other => other.to_string(),
    }
}

/// Summary of one successful tool call (see [`TraceSummary`]).
pub fn summarize(name: &str, out: &ToolOutcome) -> TraceSummary {
    let v = &out.value;
    let s = |key: &'static str, args: Value, text: String| TraceSummary { key, args, text };
    match name {
        "list_devices" => {
            let n = count(&v["devices"]);
            s(
                "ai-trace-devices",
                json!({"count": n}),
                format!("{n} device(s)"),
            )
        }
        "get_device_pins" => {
            let (d, n) = (shown(&v["device_id"]), count(&v["pins"]));
            s(
                "ai-trace-device-pins",
                json!({"device": d, "count": n}),
                format!("device #{d}: {n} pin(s)"),
            )
        }
        "list_flows" => {
            let n = count(&v["flows"]);
            s(
                "ai-trace-flows",
                json!({"count": n}),
                format!("{n} flow(s)"),
            )
        }
        "get_flow" => {
            let (id, nm) = (shown(&v["flow"]["id"]), shown(&v["flow"]["name"]));
            s(
                "ai-trace-flow",
                json!({"id": id, "name": nm}),
                format!("flow #{id} \"{nm}\""),
            )
        }
        "query_telemetry" => {
            let (n, last) = (count(&v["points"]), shown(&v["summary"]["last"]));
            s(
                "ai-trace-telemetry",
                json!({"count": n, "last": last}),
                format!("{n} point(s), last = {last}"),
            )
        }
        "list_notifications" => {
            let (c, t) = (count(&v["channels"]), count(&v["templates"]));
            s(
                "ai-trace-notifications",
                json!({"channels": c, "templates": t}),
                format!("{c} channel(s), {t} template(s)"),
            )
        }
        "describe_node_types" => s("ai-trace-node-types", json!({}), "node catalogue".into()),
        "search_knowledge" => {
            let n = count(&v["results"]);
            s(
                "ai-trace-cards",
                json!({"count": n}),
                format!("{n} card(s)"),
            )
        }
        "read_knowledge" => {
            let id = shown(&v["id"]);
            s("ai-trace-card", json!({"id": id}), format!("card {id}"))
        }
        "diagnose_device" => {
            let (d, n) = (shown(&v["device"]["slug"]), count(&v["hints"]));
            s(
                "ai-trace-diagnose-device",
                json!({"device": d, "count": n}),
                format!("device {d}: {n} hint(s)"),
            )
        }
        "diagnose_flow" => {
            let (id, n) = (shown(&v["flow"]["id"]), count(&v["hints"]));
            s(
                "ai-trace-diagnose-flow",
                json!({"id": id, "count": n}),
                format!("flow #{id}: {n} hint(s)"),
            )
        }
        "validate_flow_graph" => {
            if v["valid"].as_bool().unwrap_or(false) {
                s("ai-trace-graph-valid", json!({}), "valid graph".into())
            } else {
                let n = count(&v["violations"]);
                s(
                    "ai-trace-graph-violations",
                    json!({"count": n}),
                    format!("{n} violation(s)"),
                )
            }
        }
        "validate_calc_expression" => {
            if v["valid"].as_bool().unwrap_or(false) {
                s(
                    "ai-trace-expression-valid",
                    json!({}),
                    "valid expression".into(),
                )
            } else {
                s(
                    "ai-trace-expression-invalid",
                    json!({}),
                    "invalid expression".into(),
                )
            }
        }
        "create_flow" => {
            let (id, nm, ver) = (
                shown(&v["flow_id"]),
                shown(&v["name"]),
                shown(&v["version"]),
            );
            s(
                "ai-trace-flow-created",
                json!({"id": id, "name": nm, "version": ver}),
                format!("flow #{id} \"{nm}\" created (draft v{ver})"),
            )
        }
        "update_flow" => {
            let (id, ver) = (shown(&v["flow_id"]), shown(&v["version"]));
            s(
                "ai-trace-flow-saved",
                json!({"id": id, "version": ver}),
                format!("flow #{id}: version {ver} saved (draft)"),
            )
        }
        "list_dashboards" => {
            let n = count(&v["dashboards"]);
            s(
                "ai-trace-dashboards",
                json!({"count": n}),
                format!("{n} dashboard(s)"),
            )
        }
        "get_dashboard" => {
            let (nm, ver) = (
                shown(&v["dashboard"]["name"]),
                shown(&v["dashboard"]["version"]),
            );
            s(
                "ai-trace-dashboard",
                json!({"name": nm, "version": ver}),
                format!("dashboard \"{nm}\" v{ver}"),
            )
        }
        "validate_dashboard_layout" => {
            if v["valid"].as_bool().unwrap_or(false) {
                s("ai-trace-layout-valid", json!({}), "layout valid".into())
            } else {
                s(
                    "ai-trace-layout-refused",
                    json!({}),
                    "layout refused".into(),
                )
            }
        }
        "create_dashboard" => {
            let (nm, ver) = (shown(&v["name"]), shown(&v["version"]));
            s(
                "ai-trace-dashboard-created",
                json!({"name": nm, "version": ver}),
                format!("dashboard \"{nm}\" created (live v{ver})"),
            )
        }
        "update_dashboard" => {
            let ver = shown(&v["version"]);
            let (a, r, c) = (
                count(&v["widgets_added"]),
                count(&v["widgets_removed"]),
                count(&v["widgets_changed"]),
            );
            s(
                "ai-trace-dashboard-saved",
                json!({"version": ver, "added": a, "removed": r, "changed": c}),
                format!("dashboard v{ver} saved (live): +{a} -{r} ~{c} widget(s)"),
            )
        }
        "get_notification_template" => {
            let nm = shown(&v["template"]["name"]);
            s(
                "ai-trace-template",
                json!({"name": nm}),
                format!("template \"{nm}\""),
            )
        }
        "preview_notification_template" => s(
            "ai-trace-template-preview",
            json!({}),
            "preview rendered (not sent)".into(),
        ),
        "create_notification_template" | "update_notification_template" => {
            let nm = shown(&v["name"]);
            s(
                "ai-trace-template-saved",
                json!({"name": nm}),
                format!("template \"{nm}\" saved"),
            )
        }
        "list_functions" => {
            let n = count(&v["functions"]);
            s(
                "ai-trace-functions",
                json!({"count": n}),
                format!("{n} function(s)"),
            )
        }
        "get_function" => {
            let (nm, ver) = (shown(&v["function"]["name"]), shown(&v["latest_version"]));
            s(
                "ai-trace-function",
                json!({"name": nm, "version": ver}),
                format!("function \"{nm}\" v{ver}"),
            )
        }
        "validate_function" => {
            if v["ok"].as_bool().unwrap_or(false) {
                s(
                    "ai-trace-function-compiles",
                    json!({}),
                    "code compiles".into(),
                )
            } else {
                s(
                    "ai-trace-function-compile-errors",
                    json!({}),
                    "compile errors".into(),
                )
            }
        }
        "test_function" => {
            if v["ok"].as_bool().unwrap_or(false) {
                s(
                    "ai-trace-function-test-passed",
                    json!({}),
                    "test passed".into(),
                )
            } else {
                s(
                    "ai-trace-function-test-failed",
                    json!({}),
                    "test failed".into(),
                )
            }
        }
        "create_function" => {
            let (nm, ver) = (shown(&v["name"]), shown(&v["version"]));
            s(
                "ai-trace-function-created",
                json!({"name": nm, "version": ver}),
                format!("function \"{nm}\" created (v{ver})"),
            )
        }
        "update_function" => {
            let (id, ver) = (shown(&v["function_id"]), shown(&v["new_version"]));
            s(
                "ai-trace-function-saved",
                json!({"id": id, "version": ver}),
                format!("function #{id}: version {ver}"),
            )
        }
        "list_annotation_sets" => {
            let n = count(&v["annotation_sets"]);
            s(
                "ai-trace-annotation-sets",
                json!({"count": n}),
                format!("{n} annotation set(s)"),
            )
        }
        "list_tours" => {
            let n = count(&v["tours"]);
            s(
                "ai-trace-tours",
                json!({"count": n}),
                format!("{n} tour(s)"),
            )
        }
        "list_pois" => {
            let n = count(&v["pois"]);
            s("ai-trace-pois", json!({"count": n}), format!("{n} POI(s)"))
        }
        "list_controls" => {
            let n = count(&v["controls"]);
            s(
                "ai-trace-controls",
                json!({"count": n}),
                format!("{n} control(s)"),
            )
        }
        "read_memory" => match v["values"].as_array() {
            Some(values) => {
                let n = values.len().to_string();
                s(
                    "ai-trace-memory-values",
                    json!({"count": n}),
                    format!("{n} value(s)"),
                )
            }
            None => {
                let n = count(&v["keys"]);
                s(
                    "ai-trace-memory-keys",
                    json!({"count": n}),
                    format!("{n} key(s)"),
                )
            }
        },
        "list_taxonomies" => {
            let n = count(&v["taxonomies"]);
            s(
                "ai-trace-taxonomies",
                json!({"count": n}),
                format!("{n} taxonomy(ies)"),
            )
        }
        "get_taxonomy" => {
            let (nm, ver) = (
                shown(&v["taxonomy"]["name"]),
                shown(&v["taxonomy"]["current_version"]),
            );
            s(
                "ai-trace-taxonomy",
                json!({"name": nm, "version": ver}),
                format!("taxonomy \"{nm}\" v{ver}"),
            )
        }
        "create_taxonomy_version" => {
            let (nm, ver) = (shown(&v["name"]), shown(&v["new_version"]));
            s(
                "ai-trace-taxonomy-saved",
                json!({"name": nm, "version": ver}),
                format!("taxonomy \"{nm}\": version {ver}"),
            )
        }
        "describe_ontology" => {
            let n = count(&v["types"]);
            s(
                "ai-trace-ontology-schema",
                json!({"count": n}),
                format!("{n} object type(s)"),
            )
        }
        "query_ontology" => {
            let n = shown(&v["count"]);
            s(
                "ai-trace-ontology-query",
                json!({"count": n}),
                format!("{n} object(s)"),
            )
        }
        "get_object" => {
            let t = shown(&v["object"]["title"]);
            s(
                "ai-trace-ontology-object",
                json!({"title": t}),
                format!("object \"{t}\""),
            )
        }
        "create_object" | "update_object" => {
            let (t, ver) = (shown(&v["title"]), shown(&v["version"]));
            s(
                "ai-trace-ontology-object-saved",
                json!({"title": t, "version": ver}),
                format!("object \"{t}\" v{ver}"),
            )
        }
        "open_link" | "close_link" => {
            let (a, b, k) = (
                shown(&v["source"]),
                shown(&v["target"]),
                shown(&v["link_type"]),
            );
            let key = if name == "open_link" {
                "ai-trace-ontology-link-opened"
            } else {
                "ai-trace-ontology-link-closed"
            };
            s(
                key,
                json!({"source": a, "target": b, "link": k}),
                format!("{a} —{k}→ {b}"),
            )
        }
        "save_object_type" => {
            let (k, ver) = (shown(&v["type_key"]), shown(&v["version"]));
            s(
                "ai-trace-ontology-type-saved",
                json!({"key": k, "version": ver}),
                format!("type {k} v{ver}"),
            )
        }
        _ => s("ai-trace-ok", json!({}), "ok".into()),
    }
}

// ─────────────────────────── Outils : devices ───────────────────────────

/// Cap défensif du listing devices (le contexte du prompt en liste déjà
/// quelques-uns ; ici c'est la vue complète mais bornée).
const DEVICES_CAP: usize = 50;
/// Cap du listing flows.
const FLOWS_CAP: usize = 20;

async fn list_devices(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let rows = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .order_by_asc(device_registries::Column::Id)
        .find_also_related(predefined_devices::Entity)
        .all(deps.db)
        .await
        .map_err(internal("reading devices"))?;
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

async fn get_device_pins(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let device_id = arg_i64(args, "device_id")?;
    // Scoping org : le device doit appartenir à l'org (404 équivalent).
    let Some(device) = device_registries::Entity::find()
        .filter(device_registries::Column::OrgId.eq(deps.org_id))
        .filter(device_registries::Column::Id.eq(device_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the device"))?
    else {
        return Err(format!("device #{device_id} not found in this organization").into());
    };
    let pins = device_capability_instances::Entity::find()
        .filter(device_capability_instances::Column::DeviceRegistryId.eq(device.id))
        .order_by_asc(device_capability_instances::Column::Gpio)
        .all(deps.db)
        .await
        .map_err(internal("reading pins"))?;
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

async fn list_flows(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let status_filter = args.get("status").and_then(Value::as_str);
    let rows = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .order_by_desc(flows::Column::Id)
        .all(deps.db)
        .await
        .map_err(internal("reading flows"))?;
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

async fn get_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let flow_id = arg_i64(args, "flow_id")?;
    let Some(flow) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the flow"))?
    else {
        return Err(format!("flow #{flow_id} not found in this organization").into());
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(deps.db)
        .await
        .map_err(internal("reading the flow version"))?
    else {
        return Err(format!("flow #{flow_id} has no version (inconsistent state)").into());
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

async fn query_telemetry(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
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
async fn list_notifications(deps: &ToolDeps<'_>) -> Result<ToolOutcome, ToolError> {
    let channels = notify_channels::Entity::find()
        .filter(notify_channels::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_channels::Column::Name)
        .all(deps.db)
        .await
        .map_err(internal("reading notification channels"))?;
    let templates = notify_templates::Entity::find()
        .filter(notify_templates::Column::OrgId.eq(deps.org_id))
        .order_by_asc(notify_templates::Column::Name)
        .all(deps.db)
        .await
        .map_err(internal("reading notification templates"))?;
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
fn describe_node_types(args: &Value) -> Result<ToolOutcome, ToolError> {
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
        )
        .into());
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

fn search_knowledge(args: &Value) -> Result<ToolOutcome, ToolError> {
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

fn read_knowledge(args: &Value) -> Result<ToolOutcome, ToolError> {
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

async fn diagnose_device(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let device_id = arg_i64(args, "device_id")?;
    let value = super::diagnose::device(deps.db, deps.config, deps.org_id, device_id).await?;
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

async fn diagnose_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    let flow_id = arg_i64(args, "flow_id")?;
    let value = super::diagnose::flow(deps.db, deps.config, deps.org_id, flow_id).await?;
    Ok(ToolOutcome {
        value,
        flow_id: None,
    })
}

fn validate_flow_graph(args: &Value) -> Result<ToolOutcome, ToolError> {
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let violations = pnex_core::validate_graph(&graph);
    Ok(ToolOutcome {
        value: json!({
            "valid": violations.is_empty(),
            "violations": violations,
        }),
        flow_id: None,
    })
}

fn validate_calc_expression(args: &Value) -> Result<ToolOutcome, ToolError> {
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

async fn create_flow(deps: &ToolDeps<'_>, args: &Value) -> Result<ToolOutcome, ToolError> {
    require_write(deps)?;
    let name = arg_str(args, "name")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let device_id = args.get("device_id").and_then(Value::as_i64);
    let (flow, version, _) = flow::create_flow(
        deps.db,
        deps.org_id,
        &name,
        &graph,
        device_id,
        deps.author.clone(),
        Some("created by the AI assistant".into()),
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
    require_write(deps)?;
    let flow_id = arg_i64(args, "flow_id")?;
    let expected_version = arg_i64(args, "expected_version")?;
    let graph: pnex_core::FlowGraph = serde_json::from_value(arg_value(args, "graph")?.clone())
        .map_err(|e| format!("unreadable graph (FlowGraph expected): {e}"))?;
    let note = args.get("note").and_then(Value::as_str).map(str::to_string);
    // Scoping org : le flow doit appartenir à l'org.
    let Some(row) = flows::Entity::find_by_id(flow_id)
        .filter(flows::Column::OrgId.eq(deps.org_id))
        .one(deps.db)
        .await
        .map_err(internal("reading the flow"))?
    else {
        return Err(format!("flow #{flow_id} not found in this organization").into());
    };
    // D143: a running flow is frozen for the assistant. Checked here, at
    // execution time, from the stored status — never from the model's word.
    // Stopping stays a human gesture (no stop tool exists).
    if row.status == pnex_core::FLOW_STATUS_DEPLOYED {
        return Err(flow_running_error(&[(row.id, row.name.clone())]));
    }
    // Optimistic concurrency: the model edits the version it read with
    // get_flow; a human save in between → conflict rendered to the model,
    // which reloads instead of overwriting the human's change. The deployed
    // status is checked again under the write lock (deploy in between).
    flow::append_version_if_stopped(
        deps.db,
        &row,
        expected_version,
        &graph,
        None,
        deps.author.clone(),
        note.or_else(|| Some("changed by the AI assistant".into())),
        &ASSISTANT_WRITER,
    )
    .await
    .map(|(_, version, _)| ToolOutcome {
        value: json!({ "flow_id": row.id, "version": version }),
        flow_id: Some(row.id),
    })
    .map_err(|e| match e {
        flow::FlowWriteError::Deployed => flow_running_error(&[(row.id, row.name.clone())]),
        e => flow_write_error_string(e).into(),
    })
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
            "invalid graph: {}",
            serde_json::to_string(&v).unwrap_or_default()
        ),
        E::NameRequired => "flow name required".into(),
        E::NameTooLong => "flow name too long (> 200 characters)".into(),
        E::DeviceUnknown => "device_id not found in this organization".into(),
        E::Conflict { current, .. } => format!(
            "version conflict: the flow is now at version {current} — reload it with get_flow, then retry"
        ),
        E::Db => "database error; tell the user to try again later".into(),
        E::Deployed => "the flow is deployed: it can only be changed once stopped".into(),
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
                "get_notification_template",
                "preview_notification_template",
                "create_notification_template",
                "update_notification_template",
                "list_functions",
                "get_function",
                "validate_function",
                "test_function",
                "create_function",
                "update_function",
                "list_annotation_sets",
                "list_tours",
                "list_pois",
                "list_controls",
                "read_memory",
                "list_taxonomies",
                "get_taxonomy",
                "create_taxonomy_version",
                "describe_ontology",
                "query_ontology",
                "get_object",
                "create_object",
                "update_object",
                "open_link",
                "close_link",
                "save_object_type",
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
        // Refused before any query: no database needed.
        let db = sea_orm::DatabaseConnection::default();
        let deps = ToolDeps {
            db: &db,
            org_id: 1,
            can_write: true,
            author: None,
            o2: None,
            config: None,
            actor: None,
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
            assert!(err.message.contains("unknown tool"), "{name} → {err:?}");
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
        assert!(err.message.contains("unknown node kind"), "{}", err.message);
    }

    /// Every `ai-trace-*` key `summarize` can emit exists in both UI
    /// locales (the UI falls back to the English summary otherwise, but a
    /// missing key is unfinished work), and every registry tool has its own
    /// summary arm (no silent `ai-trace-ok`).
    #[test]
    fn trace_summary_keys_exist_in_both_locales() {
        let src = include_str!("tools.rs");
        let keys: std::collections::BTreeSet<&str> = src
            .match_indices("\"ai-trace-")
            .map(|(i, _)| {
                let rest = &src[i + 1..];
                &rest[..rest.find('"').unwrap()]
            })
            // The scan pattern itself is not a key.
            .filter(|k| *k != "ai-trace-")
            .collect();
        assert!(keys.len() > 30, "sterile scan: {keys:?}");
        let locales =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pnex-frontend/locales");
        for locale in ["en-US.ftl", "fr-FR.ftl"] {
            let ftl = std::fs::read_to_string(locales.join(locale)).unwrap();
            for key in &keys {
                assert!(
                    ftl.lines().any(|l| l.starts_with(&format!("{key} ="))),
                    "{key} missing from {locale}"
                );
            }
        }
        let empty = ToolOutcome {
            value: json!({}),
            flow_id: None,
        };
        for spec in tool_specs() {
            assert_ne!(
                summarize(spec.name, &empty).key,
                "ai-trace-ok",
                "{} has no summary arm",
                spec.name
            );
        }
    }
}
