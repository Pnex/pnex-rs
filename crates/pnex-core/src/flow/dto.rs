//! API DTOs: flow CRUD payloads, version summaries/details, runtime status
//! and the debug feed frames.

use super::*;
use serde::{Deserialize, Serialize};

// ───────────────────────────── DTOs de l'API ─────────────────────────────

/// Résumé d'un flow (liste paginée — sans graphe, évite le N+1).
/// `PartialEq` requis par les props des composants du socle CRUD (macro
/// `#[component]` dioxus 0.7 — impl généré par champ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// État du runtime de flow vu par le superviseur backend. Santé du process
/// (`runtime.json`) croisée avec la santé **par flow** (événements
/// `flow_started`/`flow_error`/`flow_stopped` du runtime — ferme « N engines,
/// 1 process ») : `deployed_flow_id`/`deployed_version_number` sont remplis
/// par le handler au prisme du flow demandé, depuis la DB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowRuntimeStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: u64,
    pub deployed_flow_id: Option<i64>,
    pub deployed_version_number: Option<i64>,
    /// État de l'engine de CE flow : `"running"` / `"error"` / `"stopped"`
    /// — `None` = inconnu (backend démarré sans annonce du runtime encore).
    #[serde(default)]
    pub engine_status: Option<String>,
    /// Dernière erreur moteur réelle de CE flow (émise par le runtime dans
    /// `flow_error`). Effacée au prochain `flow_started` de ce flow.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Outils de debug actifs (`settings.flow.debug_tools` — mode dev/debug
    /// uniquement ; en mode run le panneau est refusé 403 et masqué dans
    /// l'éditeur).
    #[serde(default)]
    pub debug_tools: bool,
    /// Latest saved version of this flow (DB). Lets an open editor notice a
    /// version saved by another user without reloading the detail.
    #[serde(default)]
    pub latest_version_number: Option<i64>,
    /// DB status of this flow (`draft` / `deployed` / `stopped`): an open
    /// editor refreshes its runtime controls when another user changed it.
    #[serde(default)]
    pub flow_status: Option<String>,
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
