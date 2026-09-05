//! Endpoints flows ETL (D18) — CRUD versionné + deploy/rollback + runtime.
//!
//! L'éditeur ne parle **qu'à l'API Loco**, jamais au runtime (garde-fou PRD,
//! docs/architecture/flow-engine.md). Les types viennent de `pnex-core`
//! (source de vérité partagée, wasm32) ; `update` porte la concurrence
//! optimiste (`expected_version_number`, 409 si périmé).

use pnex_core::{
    CreateFlow, DeployFlow, Flow, FlowDebugFeed, FlowRuntimeStatus, FlowSummary,
    FlowVersionDetail, FlowVersionSummary, FlowViolation, Paginated, RunOnceResult, UpdateFlow,
};

use crate::api::client;
use crate::api::error::ApiError;

/// Filtres de `GET /api/v1/flows` + pagination (D14) — absents = défauts
/// serveur (limit 10, offset 0).
#[derive(Default)]
pub struct FlowFilters {
    /// Sous-chaîne, insensible à la casse, sur `name`.
    pub search: Option<String>,
    /// `draft | deployed | error` — valeur inconnue ignorée côté serveur.
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl FlowFilters {
    fn to_query(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(v) = &self.search {
            parts.push(format!("search={}", urlencode(v)));
        }
        if let Some(v) = &self.status {
            parts.push(format!("status={}", urlencode(v)));
        }
        if let Some(v) = self.limit {
            parts.push(format!("limit={v}"));
        }
        if let Some(v) = self.offset {
            parts.push(format!("offset={v}"));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!("?{}", parts.join("&"))
        }
    }
}

/// Encodage percent minimal (le front n'a pas la crate url) — même code
/// que `api/devices.rs`.
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `GET /api/v1/flows` — flows de l'org courante, enveloppe paginée.
pub async fn list(filters: &FlowFilters) -> Result<Paginated<FlowSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows{}", filters.to_query()),
        None,
    )
    .await
}

/// `POST /api/v1/flows` — création (graphe = version 1).
pub async fn create(params: CreateFlow) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/flows",
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/flows/{id}` — détail (graphe = dernière version).
pub async fn detail(id: i64) -> Result<Flow, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/flows/{id}"), None).await
}

/// `PATCH /api/v1/flows/{id}` — enregistre une **nouvelle version**
/// (append-only) ; 409 si `expected_version_number` est périmé, 400
/// `{"violations": […]}` si le graphe est invalide.
pub async fn update(id: i64, params: UpdateFlow) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/flows/{id}"),
        Some(serde_json::to_value(params).unwrap_or_default()),
    )
    .await
}

/// `DELETE /api/v1/flows/{id}` — flow + versions (cascade).
pub async fn delete(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/flows/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `GET /api/v1/flows/{id}/versions` — échelle de versions, desc.
pub async fn versions(id: i64, limit: i64, offset: i64) -> Result<Paginated<FlowVersionSummary>, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/versions?limit={limit}&offset={offset}"),
        None,
    )
    .await
}

/// `GET /api/v1/flows/{id}/versions/{n}` — graphe historique.
pub async fn version(id: i64, version_number: i64) -> Result<FlowVersionDetail, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/flows/{id}/versions/{version_number}"),
        None,
    )
    .await
}

/// `POST /api/v1/flows/{id}/deploy` — projette **tous** les flows déployés
/// dans `flows.json` puis rechargement à chaud ; `version_number` absent =
/// dernière version. 503 `flow_runtime` si le moteur est off/down.
pub async fn deploy(id: i64, version_number: Option<i64>) -> Result<Flow, ApiError> {
    let body = DeployFlow { version_number };
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/deploy"),
        Some(serde_json::to_value(body).unwrap_or_default()),
    )
    .await
}

/// `POST /api/v1/flows/{id}/rollback` — deploy d'une version antérieure
/// (ne crée pas de version).
pub async fn rollback(id: i64, version_number: i64) -> Result<Flow, ApiError> {
    client::request(
        reqwest::Method::POST,
        &format!("/api/v1/flows/{id}/rollback"),
        Some(serde_json::to_value(DeployFlow { version_number: Some(version_number) }).unwrap_or_default()),
    )
    .await
}

/// `GET /api/v1/flows/{id}/runtime` — état du superviseur vu par le backend.
pub async fn runtime(id: i64) -> Result<FlowRuntimeStatus, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/flows/{id}/runtime"), None).await
}

/// `GET /api/v1/flows/{id}/debug` — feed du panneau de debug (anneau
/// mémoire du superviseur). 403 hors mode dev/debug.
pub async fn debug(id: i64) -> Result<FlowDebugFeed, ApiError> {
    client::request(reqwest::Method::GET, &format!("/api/v1/flows/{id}/debug"), None).await
}

/// `POST /api/v1/flows/{id}/run-once` — exécute une fois le flow déployé
/// (inject du payload de ses nœuds inject). 403 hors mode dev/debug, 409 si
/// non déployé, 503 `flow_runtime` si le runtime n'acquitte pas.
pub async fn run_once(id: i64) -> Result<RunOnceResult, ApiError> {
    client::request(reqwest::Method::POST, &format!("/api/v1/flows/{id}/run-once"), None).await
}

/// Échec d'un enregistrement, trié pour l'UI : conflit de concurrence (409,
/// modal « recharger / écraser »), graphe invalide (400 violations,
/// surlignage), ou autre (toast verbatim).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    Conflict { description: String },
    Invalid(Vec<FlowViolation>),
    Other(String),
}

/// Classe une `ApiError` issue de `update()` — uniquement un 409 ou un 400
/// à champ `violations` a une sémantique exploitable.
pub fn classify_save_error(err: &ApiError) -> SaveError {
    match err.status {
        Some(409) => SaveError::Conflict { description: err.message.clone() },
        Some(400) => {
            let violations = err
                .body
                .as_ref()
                .and_then(|body| body.get("violations").cloned())
                .and_then(|value| serde_json::from_value::<Vec<FlowViolation>>(value).ok());
            match violations {
                Some(violations) => SaveError::Invalid(violations),
                None => SaveError::Other(err.message.clone()),
            }
        }
        _ => SaveError::Other(err.message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtres_vers_query() {
        let mut f = FlowFilters::default();
        assert_eq!(f.to_query(), "");
        f.search = Some("pipeline température".into());
        f.status = Some("deployed".into());
        f.limit = Some(10);
        f.offset = Some(20);
        assert_eq!(
            f.to_query(),
            "?search=pipeline%20temp%C3%A9rature&status=deployed&limit=10&offset=20"
        );
    }

    #[test]
    fn conflit_409_reconnu() {
        let err = ApiError::http(
            409,
            r#"{"error":"conflict","description":"version périmée : attendu 1, version courante 2 — rechargez la dernière version"}"#,
        );
        assert_eq!(
            classify_save_error(&err),
            SaveError::Conflict {
                description: "conflict : version périmée : attendu 1, version courante 2 — rechargez la dernière version".into(),
            }
        );
    }

    #[test]
    fn violations_400_extraites() {
        let err = ApiError::http(
            400,
            r#"{"violations":[{"node_id":"n1","code":"readonly_sql","message":"mot-clé interdit en lecture seule : DELETE"}]}"#,
        );
        match classify_save_error(&err) {
            SaveError::Invalid(violations) => {
                assert_eq!(violations.len(), 1);
                assert_eq!(violations[0].code, "readonly_sql");
                assert_eq!(violations[0].node_id.as_deref(), Some("n1"));
            }
            other => panic!("violations attendues, reçu {other:?}"),
        }
    }

    #[test]
    fn erreurs_autres_relayees() {
        assert_eq!(
            classify_save_error(&ApiError::new("réseau : timeout")),
            SaveError::Other("réseau : timeout".into())
        );
        // 400 sans champ violations (jamais vu côté API, défensif).
        let err = ApiError::http(400, r#"{"detail":"boom"}"#);
        assert_eq!(classify_save_error(&err), SaveError::Other("boom".into()));
    }
}
