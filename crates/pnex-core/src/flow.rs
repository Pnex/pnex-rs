//! Modèle typé des flows ETL PNEX (décision D18) — source de vérité unique
//! consommée par le backend Loco (validation + projection), par le runtime
//! EdgeLinkd (contrats aux frontières des nœuds custom) et, plus tard, par
//! l'éditeur Dioxus (palette).
//!
//! Contraintes (mêmes règles que le reste du crate) :
//! - pur serde, **aucune dépendance native** — compile natif et `wasm32` ;
//! - `i64` pour tout identifiant qui retourne en base ;
//! - `f64`/chaînes RFC 3339 pour le temps (pas de chrono).
//!
//! Garde-fou PRD : **pas de type-check à l'échelle du graphe** — la validation
//! couvre la structure du graphe et les contrats aux frontières des nœuds
//! custom uniquement. Les nœuds builtin EdgeLinkd non modélisés passent par
//! [`FlowNodeKind::Red`] (config opaque).

/// Id du « tab » Node-RED projeté — unique par flow : le runtime EdgeLinkd
/// exécute un seul `flows.json` multi-tabs, la projection concatène donc
/// tous les flows déployés de l'instance.
pub fn flow_tab_id(flow_id: i64) -> String {
    format!("pnexflow{flow_id}")
}

use serde::{Deserialize, Serialize};

use crate::calc::validate_calc;
use crate::naming::{device_payload_key, valid_device_label};

/// Statuts possibles d'un flow (`flows.status`, enum PG `flow_status`).
pub const FLOW_STATUS_DRAFT: &str = "draft";
pub const FLOW_STATUS_DEPLOYED: &str = "deployed";
pub const FLOW_STATUS_ERROR: &str = "error";

/// Position 2D d'un nœud sur le canevas de l'éditeur (métadonnée, ignorée du runtime).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// Configuration du nœud `inject` (déclencheur intervalle/cron EdgeLinkd).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct InjectConfig {
    /// Intervalle de répétition en secondes (Node-RED `repeat`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_secs: Option<f64>,
    /// Expression cron (Node-RED `crontab`, 5 ou 6 champs).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron: String,
    /// Injection unique après délai (Node-RED `once` + `onceDelay`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub once_delay_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Payload injecté (valeur JSON quelconque ; `null` = timestamp).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
}

/// Configuration du nœud custom `pnex-sql` — requête **lecture seule** sur
/// Postgres. La connexion vient de l'env du runtime (`DATABASE_URL`), jamais
/// du graphe (aucun secret dans flows.json).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PnexSqlConfig {
    pub query: String,
    /// Clés que `msg.payload` en entrée doit contenir (contrat d'entrée typé).
    /// Vide = tout payload accepté (déclencheur pur).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
}

/// Une lecture du nœud `device` : un couple (device, pin).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceRead {
    /// Slug du device (dimension `device_id` des séries O2).
    pub device_id: String,
    /// Label du pin tel qu'affiché dans l'éditeur (la série O2 est le nom
    /// normalisé, calculé par le runtime via `normalize_measurement_name`).
    pub pin: String,
}

/// Configuration du nœud custom `device` — lecture des **dernières valeurs**
/// des pins de un ou plusieurs devices. La lecture passe par OpenObserve
/// (PromQL `last_over_time` sur la même série que l'ingestion) : une seule
/// source de vérité, cohérente avec le dashboard/Visualisation. Aucune
/// coordonnée d'accès ici — l'org est estampillée dans l'artefact au deploy
/// (`pnex_org_id`) et les creds viennent de l'env du runtime.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Lectures (device, pin) — au moins une (validate_graph).
    pub reads: Vec<DeviceRead>,
    /// Fenêtre de fraîcheur (secondes) : la dernière valeur dans la fenêtre
    /// est renvoyée, au-delà la clé est omise du payload. 1..=3600.
    #[serde(default = "default_window_secs")]
    pub window_secs: f64,
}

fn default_window_secs() -> f64 {
    60.0
}

/// Configuration du nœud custom `calc` — expression sur les clés du payload
/// device (variables identifiants, évaluateur [`crate::calc`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CalcConfig {
    pub expression: String,
}

/// Configuration du nœud custom `metric` — écriture d'une métrique
/// OpenObserve (remote-write) : nom auto-préfixé `etl_`, labels
/// `device_id="flow_{id}"`, `pred_dev="virtual_device"`, `source_type="etl"`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricConfig {
    /// Nom saisi par l'utilisateur — préfixé `etl_` et sanitisé à l'écriture
    /// (`etl_metric_name`), prévisualisé à l'identique dans l'éditeur.
    pub metric_name: String,
}

/// Configuration du nœud `debug` (capture de la sortie d'un pipeline).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebugConfig {
    /// Node-RED `active` — capture activée (défaut : oui).
    #[serde(default = "default_true")]
    pub active: bool,
    /// Propriété capturée (`"payload"` par défaut, `"true"` = message entier).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<String>,
    /// Recopie également sur la console du runtime.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub console: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self { active: true, complete: None, console: false }
    }
}

fn default_true() -> bool {
    true
}

/// Configuration du nœud custom `pnex-display` (sonde) — passthrough +
/// publication au panneau de debug. **Aucun champ saisi** : l'identité
/// (`pnex_node_id`, flow, version) est estampillée par la projection au
/// deploy — jamais lue d'une config client (anti-forgery d'attribution).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayConfig;

/// Types de nœuds modélisés côté PNEX. Le tag serde est `"kind"` et la config
/// de chaque variante est portée par un champ `config` (pas de `flatten`
/// d'enum taggé, non supporté par serde) : `{"id": "n1", "kind": "inject",
/// "config": {"repeat_secs": 5.0}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowNodeKind {
    /// Déclencheur intervalle/cron (nœud builtin `inject`).
    Inject {
        #[serde(default)]
        config: InjectConfig,
    },
    /// Nœud custom PNEX : requête SQL Postgres en lecture seule.
    PnexSql {
        config: PnexSqlConfig,
    },
    /// Nœud custom PNEX : lecture des dernières valeurs des pins d'un ou
    /// plusieurs devices via OpenObserve (même série que l'ingestion).
    Device {
        config: DeviceConfig,
    },
    /// Nœud custom PNEX : calcul sur les clés du payload device.
    Calc {
        config: CalcConfig,
    },
    /// Nœud custom PNEX : écriture d'une métrique OpenObserve
    /// (remote-write, préfixe `etl_`, device virtuel `flow_{id}`).
    Metric {
        config: MetricConfig,
    },
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
    /// Échappement : nœud builtin EdgeLinkd non modélisé, config opaque.
    Red {
        /// Nom du type Node-RED cible (ex. `"change"`, `"json"`).
        type_name: String,
        /// Config Node-RED brute du nœud.
        #[serde(default)]
        config: serde_json::Value,
    },
}

/// Câblage d'un port de sortie : `targets` = ids des nœuds destinataires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowWiring {
    pub port: usize,
    pub targets: Vec<String>,
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
    #[serde(flatten)]
    pub kind: FlowNodeKind,
}

/// Violation de validation d'un graphe (rejetée en 400 par l'API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowViolation {
    /// Nœud fautif, si la violation est localisée.
    pub node_id: Option<String>,
    /// Code machine (`duplicate_node_id`, `readonly_sql`…).
    pub code: String,
    /// Message en français, affichable tel quel au client.
    pub message: String,
}

impl FlowViolation {
    pub fn new(node_id: Option<&str>, code: &str, message: impl Into<String>) -> Self {
        Self { node_id: node_id.map(str::to_owned), code: code.to_owned(), message: message.into() }
    }
}

/// Graphe d'un flow — c'est ce qui est stocké (JSONB) dans `flow_versions`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FlowGraph {
    pub nodes: Vec<FlowNode>,
}

/// Métadonnées embarquées dans l'artefact `flows.json` projeté (traçabilité
/// de la version réellement en exécution). `org_id` permet aux nœuds custom
/// Phase 6 (device/metric) de déduire l'org OpenObserve (`pnex_org_{id}`)
/// sans accès à la base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowArtifactMeta {
    pub flow_id: i64,
    pub version_number: i64,
    pub org_id: i64,
}

// ─────────────────────────── Contrats aux frontières ───────────────────────────

/// Contrat d'entrée du nœud `pnex-sql`. Si la config du nœud déclare des
/// `params` (clés requises), `msg.payload` doit être un **objet JSON** les
/// contenant ; sans `params`, tout payload est accepté (déclencheur pur :
/// timestamp `inject`, valeur de capteur…). Le non-respect est rejeté
/// **à la frontière** du nœud — jamais de panic.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SqlQueryRequest {
    pub params: serde_json::Map<String, serde_json::Value>,
}

impl SqlQueryRequest {
    /// Valide un payload entrant (`None` = payload absent) contre les clés
    /// requises déclarées par le nœud.
    pub fn validate_payload(
        payload: Option<&serde_json::Value>,
        required: &[String],
    ) -> Result<Self, FlowViolation> {
        // Payload objet → vérification des clés requises. Absent/null/scalaire/
        // tableau → acceptable uniquement comme déclencheur pur.
        let object = match payload {
            Some(serde_json::Value::Object(map)) => Some(map),
            Some(other @ (serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
            | serde_json::Value::Array(_))) => {
                return if required.is_empty() {
                    Ok(Self::default())
                } else {
                    Err(FlowViolation::new(
                        None,
                        "sql_input_contract",
                        format!(
                            "pnex-sql attend un objet JSON en payload (clés requises : {}), reçu : {}",
                            required.join(", "),
                            type_of(other)
                        ),
                    ))
                };
            }
            Some(serde_json::Value::Null) | None => None,
        };

        match object {
            Some(map) => {
                for key in required {
                    if !map.contains_key(key) {
                        return Err(FlowViolation::new(
                            None,
                            "sql_input_contract",
                            format!("paramètre « {key} » absent du payload"),
                        ));
                    }
                }
                Ok(Self { params: map.clone() })
            }
            None if required.is_empty() => Ok(Self::default()),
            None => Err(FlowViolation::new(
                None,
                "sql_input_contract",
                format!(
                    "pnex-sql attend un objet JSON en payload (clés requises : {}), payload absent",
                    required.join(", ")
                ),
            )),
        }
    }
}

/// Contrat de sortie du nœud `pnex-sql` : toujours un tableau de lignes
/// (objets colonne → valeur), poussé dans `msg.payload`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SqlQueryResult {
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

impl SqlQueryResult {
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::Value::Array(self.rows.iter().map(|r| serde_json::Value::Object(r.clone())).collect())
    }
}

/// Map numérique extraite d'un payload (clés device sanitisées → valeur).
pub type NumericMap = std::collections::HashMap<String, f64>;

/// Contrat d'entrée du nœud `calc` / de sortie du nœud `device` : objet JSON
/// `clé → valeur numérique` (booléens convertis 1/0, parité avec
/// `handle_state_report`). Rejeté à la frontière — jamais de panic.
pub fn numeric_map_from_payload(
    payload: Option<&serde_json::Value>,
    node: &str,
) -> Result<NumericMap, FlowViolation> {
    let Some(serde_json::Value::Object(map)) = payload else {
        return Err(FlowViolation::new(
            None,
            &format!("{node}_input_contract"),
            format!(
                "le nœud {node} attend un objet JSON en payload (sortie d'un nœud device), reçu : {}",
                payload.map(type_of).unwrap_or("payload absent")
            ),
        ));
    };
    let mut out = NumericMap::with_capacity(map.len());
    for (key, value) in map {
        let v = match value {
            serde_json::Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
            serde_json::Value::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            other => {
                return Err(FlowViolation::new(
                    None,
                    &format!("{node}_input_contract"),
                    format!("valeur non numérique pour la clé « {key} » (reçu : {})", type_of(other)),
                ));
            }
        };
        out.insert(key.clone(), v);
    }
    Ok(out)
}

/// Contrat d'entrée du nœud `metric` : une valeur numérique (sortie d'un
/// nœud `calc`) — booléen converti 1/0, sinon rejet typé.
pub fn metric_value_from_payload(payload: Option<&serde_json::Value>) -> Result<f64, FlowViolation> {
    match payload {
        Some(serde_json::Value::Number(n)) => n
            .as_f64()
            .ok_or_else(|| FlowViolation::new(None, "metric_input_contract", "valeur numérique hors plage f64")),
        Some(serde_json::Value::Bool(b)) => Ok(if *b { 1.0 } else { 0.0 }),
        other => Err(FlowViolation::new(
            None,
            "metric_input_contract",
            format!(
                "le nœud metric attend une valeur numérique en payload (sortie d'un nœud calc), reçu : {}",
                other.map(type_of).unwrap_or("payload absent")
            ),
        )),
    }
}

fn type_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "booléen",
        serde_json::Value::Number(_) => "nombre",
        serde_json::Value::String(_) => "chaîne",
        serde_json::Value::Array(_) => "tableau",
        serde_json::Value::Object(_) => "objet",
    }
}

// ─────────────────────────────── Validation ───────────────────────────────

/// Valide la structure du graphe + les contrats par nœud. Retourne **toutes**
/// les violations (l'API les renvoie en 400, champ `violations`).
pub fn validate_graph(g: &FlowGraph) -> Vec<FlowViolation> {
    let mut v = Vec::new();
    if g.nodes.is_empty() {
        v.push(FlowViolation::new(None, "empty_graph", "le graphe est vide"));
    }

    let mut seen = std::collections::HashSet::new();
    for n in &g.nodes {
        if !seen.insert(n.id.clone()) {
            v.push(FlowViolation::new(
                Some(&n.id),
                "duplicate_node_id",
                format!("id de nœud dupliqué : « {} »", n.id),
            ));
        }
        match &n.kind {
            FlowNodeKind::Inject { config } => validate_inject(&n.id, config, &mut v),
            FlowNodeKind::PnexSql { config } => {
                if let Err(e) = validate_sql_readonly(&config.query) {
                    v.push(FlowViolation::new(Some(&n.id), &e.code, e.message));
                }
            }
            FlowNodeKind::Device { config } => validate_device(&n.id, config, &mut v),
            FlowNodeKind::Calc { config } => {
                for e in validate_calc(&config.expression) {
                    v.push(FlowViolation::new(Some(&n.id), "calc_bad_expression", e.to_string()));
                }
            }
            FlowNodeKind::Metric { config } => {
                if config.metric_name.trim().is_empty() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "metric_name_missing",
                        "le nom de la métrique est requis",
                    ));
                }
            }
            FlowNodeKind::Debug { .. } => {}
            FlowNodeKind::Display { .. } => {}
            FlowNodeKind::Red { type_name, config } => {
                if type_name.trim().is_empty() {
                    v.push(FlowViolation::new(Some(&n.id), "bad_red_node", "type Node-RED manquant"));
                }
                if !config.is_null() && !config.is_object() {
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "bad_red_node",
                        "la config d'un nœud Red doit être un objet JSON",
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
                    v.push(FlowViolation::new(
                        Some(&n.id),
                        "dangling_target",
                        format!("le port {} du nœud « {} » cible un nœud inconnu : « {} »", w.port, n.id, t),
                    ));
                }
            }
        }
    }
    v
}

fn validate_inject(id: &str, c: &InjectConfig, v: &mut Vec<FlowViolation>) {
    if let Some(r) = c.repeat_secs {
        if !(r.is_finite() && r > 0.0) {
            v.push(FlowViolation::new(Some(id), "bad_repeat", "repeat_secs doit être > 0"));
        }
    }
    if c.repeat_secs.is_none() && c.cron.trim().is_empty() && c.once_delay_secs.is_none() {
        v.push(FlowViolation::new(
            Some(id),
            "no_trigger",
            "le nœud inject n'a pas de déclencheur (repeat_secs, cron ou once_delay_secs requis)",
        ));
    }
}

/// Règles du nœud `device` : au moins une lecture, device_id en slug
/// (`valid_device_label` — interpolé dans un sélecteur PromQL), pin non
/// vide, clés de payload uniques, fenêtre bornée 1..=3600 s.
fn validate_device(id: &str, c: &DeviceConfig, v: &mut Vec<FlowViolation>) {
    if c.reads.is_empty() {
        v.push(FlowViolation::new(
            Some(id),
            "device_no_reads",
            "aucune lecture configurée (au moins un couple device/pin est requis)",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for r in &c.reads {
        if !valid_device_label(&r.device_id) {
            v.push(FlowViolation::new(
                Some(id),
                "device_bad_read",
                format!(
                    "device « {} » invalide (slug requis : lettres, chiffres, . _ -)",
                    r.device_id
                ),
            ));
        }
        if r.pin.trim().is_empty() {
            v.push(FlowViolation::new(
                Some(id),
                "device_bad_read",
                format!("pin manquant pour le device « {} »", r.device_id),
            ));
        }
        if !seen.insert(device_payload_key(&r.device_id, &r.pin)) {
            v.push(FlowViolation::new(
                Some(id),
                "device_duplicate_key",
                format!(
                    "lecture dupliquée « {} » (les clés de payload doivent être uniques)",
                    device_payload_key(&r.device_id, &r.pin)
                ),
            ));
        }
    }
    if !(c.window_secs.is_finite() && (1.0..=3600.0).contains(&c.window_secs)) {
        v.push(FlowViolation::new(
            Some(id),
            "device_window_range",
            "window_secs doit être compris entre 1 et 3600",
        ));
    }
}

/// N'impose que du SQL **lecture seule** : première requête SELECT/WITH,
/// aucun mot-clé de modification de données, une seule instruction.
/// Analyse best-effort (fail-closed) — la défense réelle reste le rôle
/// Postgres en lecture seule, documenté dans docs/architecture/flow-engine.md.
pub fn validate_sql_readonly(query: &str) -> Result<(), FlowViolation> {
    let q = query.trim();
    if q.is_empty() {
        return Err(FlowViolation::new(None, "empty_query", "la requête SQL est vide"));
    }

    let first_word = q.split_whitespace().next().unwrap_or_default().to_ascii_uppercase();
    if first_word != "SELECT" && first_word != "WITH" {
        return Err(FlowViolation::new(
            None,
            "readonly_sql",
            format!("seules les requêtes SELECT/WITH sont autorisées, reçu : {first_word}"),
        ));
    }

    // Les CTE data-modifying (WITH x AS (INSERT …)) restent interdites.
    const FORBIDDEN: [&str; 10] =
        ["INSERT", "UPDATE", "DELETE", "MERGE", "TRUNCATE", "ALTER", "CREATE", "DROP", "GRANT", "COPY"];
    let upper = q.to_ascii_uppercase();
    for kw in FORBIDDEN {
        if upper.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == kw) {
            return Err(FlowViolation::new(
                None,
                "readonly_sql",
                format!("mot-clé interdit en lecture seule : {kw}"),
            ));
        }
    }

    // Une seule instruction : au plus un `;`, et uniquement terminal.
    if q.trim_end_matches(';').contains(';') {
        return Err(FlowViolation::new(None, "multi_statement", "une seule instruction SQL est autorisée"));
    }
    Ok(())
}

// ─────────────────────────────── Projection ───────────────────────────────

/// Projette le graphe typé PNEX vers les entrées Node-RED du `flows.json`
/// (un tab + ses nœuds) consommées par le runtime EdgeLinkd headless.
/// L'artefact complet = concaténation des projections de tous les flows
/// déployés. Les métadonnées de version sont embarquées sur le tab et sur
/// les nœuds custom (clés inconnues préservées par le désérialiseur).
pub fn to_red_flows_json(g: &FlowGraph, meta: &FlowArtifactMeta) -> serde_json::Value {
    let tab_id = flow_tab_id(meta.flow_id);
    let mut entries = vec![serde_json::json!({
        "id": tab_id,
        "type": "tab",
        "label": format!("Flow #{} v{}", meta.flow_id, meta.version_number),
        "pnex_flow_id": meta.flow_id,
        "pnex_version": meta.version_number,
        "pnex_org_id": meta.org_id,
    })];

    for n in &g.nodes {
        let wires = wires_of(n);
        let mut e = match &n.kind {
            FlowNodeKind::Inject { config } => inject_entry(config),
            FlowNodeKind::PnexSql { config } => serde_json::json!({
                "type": "pnex-sql",
                "query": config.query,
                "params": config.params,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
            }),
            FlowNodeKind::Device { config } => serde_json::json!({
                "type": "pnex-device",
                "reads": config.reads,
                "window_secs": config.window_secs,
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
            FlowNodeKind::Metric { config } => serde_json::json!({
                "type": "pnex-metric",
                "metric_name": config.metric_name,
                "pnex_flow_id": meta.flow_id,
                "pnex_version": meta.version_number,
                "pnex_org_id": meta.org_id,
            }),
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
            FlowNodeKind::Red { type_name, config } => {
                let mut obj = config.as_object().cloned().unwrap_or_default();
                obj.insert("type".into(), serde_json::Value::String(type_name.clone()));
                serde_json::Value::Object(obj)
            }
        };

        {
            let obj = e.as_object_mut().expect("entrée flows.json");
            obj.insert("id".into(), serde_json::Value::String(n.id.clone()));
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
        entries.push(e);
    }

    serde_json::Value::Array(entries)
}

/// Construit le tableau `wires` Node-RED : `wires[port] = [ids cibles]`.
fn wires_of(n: &FlowNode) -> serde_json::Value {
    let max_port = n.outputs.iter().map(|w| w.port).max().map(|p| p + 1).unwrap_or(0);
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

// ───────────────────────────── DTOs de l'API ─────────────────────────────

/// Résumé d'un flow (liste paginée — sans graphe, évite le N+1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSummary {
    pub id: i64,
    pub org_id: i64,
    pub device_id: Option<i64>,
    pub name: String,
    pub status: String,
    pub deployed_version_number: Option<i64>,
    pub latest_version_number: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Flow tel que renvoyé par l'API (graphe = dernière version).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    pub id: i64,
    pub org_id: i64,
    pub device_id: Option<i64>,
    pub name: String,
    pub status: String,
    pub deployed_version_number: Option<i64>,
    pub latest_version_number: i64,
    pub graph: FlowGraph,
    pub created_at: String,
    pub updated_at: String,
}

/// Résumé d'une version (liste paginée).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowVersionSummary {
    pub id: i64,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub deployed: bool,
    pub created_at: String,
}

/// Détail d'une version (graphe inclus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowVersionDetail {
    pub id: i64,
    pub version_number: i64,
    pub author: Option<String>,
    pub note: Option<String>,
    pub deployed: bool,
    pub created_at: String,
    pub graph: FlowGraph,
}

/// Création : flow + version 1 en une transaction. `Serialize` sert au
/// front (l'éditeur construit la requête typée) — le backend ne lit que
/// `Deserialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFlow {
    pub name: String,
    #[serde(default)]
    pub device_id: Option<i64>,
    pub graph: FlowGraph,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Enregistrement d'une nouvelle version (append-only). La concurrence est
/// optimiste : `expected_version_number` doit valoir la version courante,
/// sinon rejet 409 — deux éditeurs ne s'écrasent pas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFlow {
    pub expected_version_number: i64,
    pub graph: FlowGraph,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Déploiement explicite d'une version (projection + rechargement runtime).
/// `version_number` absent → dernière version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployFlow {
    #[serde(default)]
    pub version_number: Option<i64>,
}

/// État du runtime de flow vu par le superviseur backend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowRuntimeStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: u64,
    pub deployed_flow_id: Option<i64>,
    pub deployed_version_number: Option<i64>,
    /// Outils de debug actifs (`settings.flow.debug_tools` — mode dev/debug
    /// uniquement ; en mode run le panneau et le run-once sont refusés 403
    /// et masqués dans l'éditeur).
    #[serde(default)]
    pub debug_tools: bool,
}

/// Une entrée du panneau de debug (anneau mémoire du superviseur, alimenté
/// par le stdout du runtime — événements `debug` attribués à un flow).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDebugEntry {
    /// Ordre global croissant (horloge du process backend).
    pub seq: u64,
    /// Horodatage RFC 3339 (horloge backend à la réception).
    pub ts: String,
    pub flow_id: i64,
    /// Id éditeur du nœud émetteur (`"n2"` — brut, jamais le hash moteur).
    pub node_id: String,
    /// Nom du nœud, s'il en porte un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Valeur capturée — brute : objet pour `pnex-display`, chaîne
    /// pré-stringifiée pour le `debug` builtin (le client tente un re-parse).
    pub msg: serde_json::Value,
    /// `"debug"` (builtin) ou `"pnex-display"` (sonde).
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msgid: Option<String>,
}

/// Réponse de `GET /flows/{id}/debug` — feed du panneau (les plus anciennes
/// d'abord).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDebugFeed {
    pub flow_id: i64,
    pub entries: Vec<FlowDebugEntry>,
}

/// Réponse de `POST /flows/{id}/run-once`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunOnceResult {
    /// Messages effectivement injectés dans les cibles (succès seulement).
    pub injected: u32,
    /// Nœuds inject trouvés dans le flow déployé.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node_inject(id: &str) -> FlowNode {
        FlowNode {
            id: id.into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Inject {
                config: InjectConfig { repeat_secs: Some(1.0), ..Default::default() },
            },
        }
    }

    fn node_display(id: &str, targets: &[String]) -> FlowNode {
        FlowNode {
            id: id.into(),
            name: None,
            position: None,
            outputs: if targets.is_empty() {
                vec![]
            } else {
                vec![FlowWiring { port: 0, targets: targets.to_vec() }]
            },
            kind: FlowNodeKind::Display { config: DisplayConfig },
        }
    }

    fn simple_graph() -> FlowGraph {
        FlowGraph {
            nodes: vec![
                node_inject("n1"),
                FlowNode {
                    id: "n2".into(),
                    name: Some("query".into()),
                    position: Some(Position { x: 200.0, y: 100.0 }),
                    outputs: vec![FlowWiring { port: 0, targets: vec!["n3".into()] }],
                    kind: FlowNodeKind::PnexSql {
                        config: PnexSqlConfig { query: "SELECT 1".into(), params: vec![] },
                    },
                },
                FlowNode {
                    id: "n3".into(),
                    name: None,
                    position: None,
                    outputs: vec![],
                    kind: FlowNodeKind::Debug { config: DebugConfig::default() },
                },
            ],
        }
    }

    #[test]
    fn graph_serde_roundtrip() {
        let g = simple_graph();
        let json = serde_json::to_string(&g).unwrap();
        let back: FlowGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn graph_minimal_form_deserialise() {
        // Forme minimale (champs optionnels absents) acceptée.
        let g: FlowGraph = serde_json::from_str(
            r#"{"nodes":[{"id":"n1","kind":"debug","config":{}}]}"#,
        )
        .unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert!(matches!(&g.nodes[0].kind, FlowNodeKind::Debug { config } if config.active));
    }

    #[test]
    fn validate_accepts_simple_pipeline() {
        assert!(validate_graph(&simple_graph()).is_empty());
    }

    #[test]
    fn validate_rejects_structure_errors() {
        let mut g = simple_graph();
        g.nodes.push(node_inject("n1")); // dupliqué
        g.nodes[1].outputs.push(FlowWiring { port: 1, targets: vec!["ghost".into()] }); // cible inconnue
        let v = validate_graph(&g);
        let codes: Vec<&str> = v.iter().map(|x| x.code.as_str()).collect();
        assert!(codes.contains(&"duplicate_node_id"), "{v:?}");
        assert!(codes.contains(&"dangling_target"), "{v:?}");
    }

    #[test]
    fn validate_rejects_inject_sans_declencheur() {
        let g = FlowGraph {
            nodes: vec![FlowNode {
                id: "n1".into(),
                name: None,
                position: None,
                outputs: vec![],
                kind: FlowNodeKind::Inject { config: InjectConfig::default() },
            }],
        };
        let v = validate_graph(&g);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "no_trigger");
    }

    #[test]
    fn sql_readonly_accepte_select_et_with() {
        assert!(validate_sql_readonly("SELECT 1").is_ok());
        assert!(validate_sql_readonly("  select * from t where x = 'with' ").is_ok());
        assert!(validate_sql_readonly("WITH a AS (SELECT 1) SELECT * FROM a;").is_ok());
    }

    #[test]
    fn sql_readonly_rejecte_ecriture() {
        assert_eq!(validate_sql_readonly("DELETE FROM t").unwrap_err().code, "readonly_sql");
        // CTE data-modifying : rejetée aussi.
        assert_eq!(
            validate_sql_readonly("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d")
                .unwrap_err()
                .code,
            "readonly_sql"
        );
        assert_eq!(validate_sql_readonly("UPDATE t SET x = 1").unwrap_err().code, "readonly_sql");
        assert_eq!(validate_sql_readonly("INSERT INTO t VALUES (1)").unwrap_err().code, "readonly_sql");
    }

    #[test]
    fn sql_readonly_rejecte_multi_instructions_et_vide() {
        assert_eq!(validate_sql_readonly("SELECT 1; SELECT 2").unwrap_err().code, "multi_statement");
        assert_eq!(validate_sql_readonly("   ").unwrap_err().code, "empty_query");
        assert_eq!(validate_sql_readonly("GRANT ALL ON t TO x").unwrap_err().code, "readonly_sql");
    }

    #[test]
    fn sql_contrat_frontiere() {
        // Sans clés requises : déclencheur pur accepté (timestamp, valeur…).
        let none: &[String] = &[];
        assert!(SqlQueryRequest::validate_payload(None, none).is_ok());
        assert!(SqlQueryRequest::validate_payload(Some(&json!(1725300000)), none).is_ok());
        assert!(SqlQueryRequest::validate_payload(Some(&json!("capteur")), none).is_ok());
        assert!(SqlQueryRequest::validate_payload(Some(&json!({"k": 1})), none).is_ok());

        // Clés requises déclarées : payload objet obligatoire.
        let req = [String::from("k")];
        assert!(SqlQueryRequest::validate_payload(Some(&json!({"k": 1})), req.as_slice()).is_ok());
        assert_eq!(
            SqlQueryRequest::validate_payload(Some(&json!(42)), req.as_slice()).unwrap_err().code,
            "sql_input_contract"
        );
        assert_eq!(
            SqlQueryRequest::validate_payload(Some(&json!({"autre": 1})), req.as_slice())
                .unwrap_err()
                .code,
            "sql_input_contract"
        );

        // Sortie : toujours un tableau de lignes.
        let res = SqlQueryResult {
            rows: vec![serde_json::from_value(json!({"?column?": 1})).unwrap()],
        };
        assert_eq!(res.to_value(), json!([{"?column?": 1}]));
    }

    #[test]
    fn projection_flows_json_snapshot() {
        let meta = FlowArtifactMeta { flow_id: 12, version_number: 3, org_id: 7 };
        let out = to_red_flows_json(&simple_graph(), &meta);
        assert_eq!(out[0], json!({
            "id": "pnexflow12", "type": "tab", "label": "Flow #12 v3",
            "pnex_flow_id": 12, "pnex_version": 3, "pnex_org_id": 7,
        }));
        // inject : intervalle projeté, payload JSON encodé en chaîne.
        assert_eq!(out[1]["type"], "inject");
        assert_eq!(out[1]["repeat"], 1.0);
        assert_eq!(out[1]["payloadType"], "date");
        // pnex-sql : type custom + traçabilité de version.
        assert_eq!(out[2]["type"], "pnex-sql");
        assert_eq!(out[2]["query"], "SELECT 1");
        assert_eq!(out[2]["pnex_flow_id"], 12);
        assert_eq!(out[2]["pnex_version"], 3);
        assert_eq!(out[2]["z"], "pnexflow12");
        // debug : câblage vide, capture payload par défaut.
        assert_eq!(out[3]["type"], "debug");
        assert_eq!(out[3]["complete"], "payload");
        // wires : n2 (port 0) → n3 ; n3 sans port de sortie → tableau vide.
        assert_eq!(out[2]["wires"], json!([["n3"]]));
        assert_eq!(out[3]["wires"], json!([]));
        // position reportée.
        assert_eq!(out[2]["x"], 200.0);
        assert_eq!(out[2]["name"], "query");
    }

    fn node_device_reads(id: &str, reads: &[(&str, &str)]) -> FlowNode {
        FlowNode {
            id: id.into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Device {
                config: DeviceConfig {
                    reads: reads
                        .iter()
                        .map(|(d, p)| DeviceRead { device_id: (*d).into(), pin: (*p).into() })
                        .collect(),
                    window_secs: 60.0,
                },
            },
        }
    }

    #[test]
    fn validate_device_calc_metric() {
        // Device : lectures vides → device_no_reads.
        let mut g = FlowGraph { nodes: vec![node_device_reads("d1", &[])] };
        let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
        assert!(codes.contains(&"device_no_reads".to_string()), "{codes:?}");

        // Device : lecture complète + déclencheur → vert.
        g.nodes = vec![
            node_inject("n1"),
            node_device_reads("d1", &[("fuzzy-zebra", "D1")]),
        ];
        assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));

        // Device : slug invalide + pin vide + clé dupliquée + fenêtre hors bornes.
        g.nodes = vec![FlowNode {
            id: "d1".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Device {
                config: DeviceConfig {
                    reads: vec![
                        DeviceRead { device_id: "deux mots".into(), pin: "D1".into() },
                        DeviceRead { device_id: "a".into(), pin: "  ".into() },
                        DeviceRead { device_id: "a-b".into(), pin: "c".into() },
                        DeviceRead { device_id: "a".into(), pin: "b-c".into() },
                    ],
                    window_secs: 0.5,
                },
            },
        }];
        let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
        for code in ["device_bad_read", "device_duplicate_key", "device_window_range"] {
            assert!(codes.contains(&code.to_string()), "{codes:?}");
        }

        // Calc : expression invalide rejetée, variables tolérées.
        g.nodes = vec![FlowNode {
            id: "c1".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Calc { config: CalcConfig { expression: "foo(a) + ".into() } },
        }];
        let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
        assert!(codes.contains(&"calc_bad_expression".to_string()), "{codes:?}");

        g.nodes = vec![FlowNode {
            id: "c1".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Calc { config: CalcConfig { expression: "a + b".into() } },
        }];
        assert!(validate_graph(&g).is_empty());

        // Metric : nom requis.
        g.nodes = vec![FlowNode {
            id: "m1".into(),
            name: None,
            position: None,
            outputs: vec![],
            kind: FlowNodeKind::Metric { config: MetricConfig { metric_name: "  ".into() } },
        }];
        let codes: Vec<String> = validate_graph(&g).iter().map(|x| x.code.clone()).collect();
        assert!(codes.contains(&"metric_name_missing".to_string()), "{codes:?}");
    }

    #[test]
    fn projection_noeuds_phase6_estampilles() {
        let g = FlowGraph {
            nodes: vec![
                node_device_reads("d1", &[("fuzzy-zebra", "D1")]),
                FlowNode {
                    id: "c1".into(),
                    name: None,
                    position: None,
                    outputs: vec![],
                    kind: FlowNodeKind::Calc { config: CalcConfig { expression: "a * 2".into() } },
                },
                FlowNode {
                    id: "m1".into(),
                    name: None,
                    position: None,
                    outputs: vec![],
                    kind: FlowNodeKind::Metric { config: MetricConfig { metric_name: "moyenne".into() } },
                },
            ],
        };
        let out = to_red_flows_json(&g, &FlowArtifactMeta { flow_id: 9, version_number: 2, org_id: 4 });
        assert_eq!(out[0]["pnex_org_id"], 4);
        assert_eq!(out[1]["type"], "pnex-device");
        assert_eq!(out[1]["pnex_flow_id"], 9);
        assert_eq!(out[1]["pnex_version"], 2);
        assert_eq!(out[1]["pnex_org_id"], 4);
        assert_eq!(out[1]["window_secs"], 60.0);
        assert_eq!(out[1]["reads"][0]["device_id"], "fuzzy-zebra");
        assert_eq!(out[2]["type"], "pnex-calc");
        assert_eq!(out[2]["expression"], "a * 2");
        assert_eq!(out[3]["type"], "pnex-metric");
        assert_eq!(out[3]["metric_name"], "moyenne");
        assert_eq!(out[3]["pnex_org_id"], 4);
    }

    #[test]
    fn display_sans_config_est_valide() {
        // La sonde n'a aucune config saisie : défaut → graphe vert.
        let g = FlowGraph {
            nodes: vec![node_inject("n1"), node_display("n2", &[])],
        };
        assert!(validate_graph(&g).is_empty(), "{:?}", validate_graph(&g));
        // Forme minimale (config absente) acceptée par le désérialiseur.
        let g: FlowGraph = serde_json::from_str(
            r#"{"nodes":[{"id":"n1","kind":"display"}]}"#,
        )
        .unwrap();
        assert!(matches!(g.nodes[0].kind, FlowNodeKind::Display { .. }));
    }

    #[test]
    fn projection_noeud_display_estampille() {
        let g = FlowGraph {
            nodes: vec![
                node_inject("n1"),
                node_display("n3", &["n4".into()]),
                FlowNode {
                    id: "n4".into(),
                    name: None,
                    position: None,
                    outputs: vec![],
                    kind: FlowNodeKind::Debug { config: DebugConfig::default() },
                },
            ],
        };
        let out = to_red_flows_json(&g, &FlowArtifactMeta { flow_id: 12, version_number: 3, org_id: 7 });
        assert_eq!(out[0]["id"], "pnexflow12");
        assert_eq!(out[2]["type"], "pnex-display");
        // L'id canvas brut est la clé de rattachement panneau/badge.
        assert_eq!(out[2]["pnex_node_id"], "n3");
        assert_eq!(out[2]["pnex_flow_id"], 12);
        assert_eq!(out[2]["pnex_version"], 3);
        assert_eq!(out[2]["pnex_org_id"], 7);
        assert_eq!(out[2]["z"], "pnexflow12");
        assert_eq!(out[2]["wires"], json!([["n4"]]));
        // Le debug builtin reste un nœud builtin (pas de stamp custom requis).
        assert_eq!(out[3]["type"], "debug");
    }

    #[test]
    fn projection_debug_tosidebar_toujours_vrai() {
        // Garde-fou panneau : sans `tosidebar`, le nœud debug builtin ne
        // publie jamais sur le canal — l'éditeur en dépend.
        let g = FlowGraph {
            nodes: vec![FlowNode {
                id: "n2".into(),
                name: None,
                position: None,
                outputs: vec![],
                kind: FlowNodeKind::Debug { config: DebugConfig::default() },
            }],
        };
        let out = to_red_flows_json(&g, &FlowArtifactMeta { flow_id: 1, version_number: 1, org_id: 1 });
        assert_eq!(out[1]["tosidebar"], true);
        assert_eq!(out[1]["active"], true);
    }

    #[test]
    fn contrats_payload_phase6() {
        // Sortie device / entrée calc : objet numérique, bool → 1/0.
        let payload = json!({"cap_1_D1": 21.5, "cap_2_D0": true});
        let map = numeric_map_from_payload(Some(&payload), "calc").unwrap();
        assert_eq!(map["cap_1_D1"], 21.5);
        assert_eq!(map["cap_2_D0"], 1.0);
        // Non-objet → rejet typé.
        assert_eq!(
            numeric_map_from_payload(Some(&json!(42)), "calc").unwrap_err().code,
            "calc_input_contract"
        );
        assert_eq!(
            numeric_map_from_payload(None, "calc").unwrap_err().code,
            "calc_input_contract"
        );
        // Valeur non numérique → rejet avec la clé en cause.
        assert_eq!(
            numeric_map_from_payload(Some(&json!({"k": "texte"})), "calc")
                .unwrap_err()
                .message,
            "valeur non numérique pour la clé « k » (reçu : chaîne)"
        );
        // Entrée metric : nombre, bool, sinon rejet.
        assert_eq!(metric_value_from_payload(Some(&json!(21.5))).unwrap(), 21.5);
        assert_eq!(metric_value_from_payload(Some(&json!(true))).unwrap(), 1.0);
        assert_eq!(
            metric_value_from_payload(Some(&json!({"a": 1}))).unwrap_err().code,
            "metric_input_contract"
        );
    }

    #[test]
    fn projection_red_passthrough_conserve_la_config() {
        let g = FlowGraph {
            nodes: vec![FlowNode {
                id: "r1".into(),
                name: None,
                position: None,
                outputs: vec![],
                kind: FlowNodeKind::Red {
                    type_name: "change".into(),
                    config: json!({"rules": [{"p": "payload"}]}),
                },
            }],
        };
        let out = to_red_flows_json(&g, &FlowArtifactMeta { flow_id: 1, version_number: 1, org_id: 7 });
        assert_eq!(out[1]["type"], "change");
        assert_eq!(out[1]["rules"], json!([{"p": "payload"}]));
        assert_eq!(out[1]["z"], "pnexflow1");
    }

    #[test]
    fn dto_create_flow_deserialise() {
        let json = r#"{
            "name": "t",
            "device_id": 3,
            "author": "alice",
            "graph": {"nodes":[{"id":"n1","kind":"inject","config":{"repeat_secs":5.0}}]}
        }"#;
        let c: CreateFlow = serde_json::from_str(json).unwrap();
        assert_eq!(c.name, "t");
        assert_eq!(c.device_id, Some(3));
        assert_eq!(validate_graph(&c.graph).len(), 0);

        // Concurrence optimiste : UpdateFlow porte la version attendue.
        let u: UpdateFlow = serde_json::from_str(
            r#"{"expected_version_number": 2, "graph": {"nodes": []}}"#,
        )
        .unwrap();
        assert_eq!(u.expected_version_number, 2);
    }
}
