//! Client API de la couche d'organisation transverse (D42) —
//! labels / containment / edges / folders / recherche cross-kind.
//! École `api/media.rs` : types miroirs du contrôleur `/api/v1/resources`.
//!
//! Surface client complète dès maintenant (containment/edges/folders/search
//! consommés par les UI V2 : arbre mixte, placement, recherche).

#![allow(dead_code)]

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::api::client;
use crate::api::error::ApiError;

/// Labels d'une ressource (propres).
pub type LabelSet = BTreeMap<String, Option<String>>;

/// Arête D42 (liens croisés avec placement).
#[derive(Debug, Clone, Deserialize)]
pub struct Edge {
    pub id: i64,
    pub relation: String,
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub placement: Option<serde_json::Value>,
    pub created_at: String,
}

/// Dossier (kind `folder`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub emoji: Option<String>,
    created_at: String,
    updated_at: String,
}

/// Recherche cross-kind (labels effectifs).
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub kind: String,
    pub id: String,
}

// ─────────────────────────── labels ───────────────────────────

/// Max ids accepted by one `labels/batch` call (server bound).
pub const LABELS_BATCH_MAX: usize = 200;

/// `POST /api/v1/resources/labels/batch` — own labels of a page of rows
/// (`kind` + stringified PKs, at most [`LABELS_BATCH_MAX`]). Ids without
/// labels are absent from the returned map.
pub async fn labels_batch(
    kind: &str,
    ids: Vec<String>,
) -> Result<std::collections::HashMap<String, LabelSet>, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        #[serde(default)]
        labels: std::collections::HashMap<String, LabelSet>,
    }
    let dto: Dto = client::request(
        reqwest::Method::POST,
        "/api/v1/resources/labels/batch",
        Some(serde_json::json!({ "kind": kind, "ids": ids })),
    )
    .await?;
    Ok(dto.labels)
}

/// `GET /api/v1/resources/{kind}/{id}/labels`.
pub async fn get_labels(kind: &str, id: &str) -> Result<LabelSet, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        labels: LabelSet,
    }
    let dto: Dto = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/labels"),
        None,
    )
    .await?;
    Ok(dto.labels)
}

/// `PUT /api/v1/resources/{kind}/{id}/labels` — remplace tout le doc.
pub async fn put_labels(kind: &str, id: &str, labels: &LabelSet) -> Result<LabelSet, ApiError> {
    #[derive(Deserialize)]
    struct Dto {
        labels: LabelSet,
    }
    let dto: Dto = client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/resources/{kind}/{id}/labels"),
        Some(serde_json::json!({ "labels": labels })),
    )
    .await?;
    Ok(dto.labels)
}

/// Labels effectifs (fusion « le plus proche gagne » + héritage détaillé).
#[derive(Debug, Clone, Deserialize)]
pub struct EffectiveLabels {
    pub merged: LabelSet,
    /// (kind, id, labels) feuille → racine.
    pub inherited: Vec<(String, String, LabelSet)>,
}

/// `GET /api/v1/resources/{kind}/{id}/labels/effective`.
pub async fn get_effective_labels(kind: &str, id: &str) -> Result<EffectiveLabels, ApiError> {
    #[derive(Deserialize)]
    struct Inherited {
        kind: String,
        id: String,
        labels: LabelSet,
    }
    #[derive(Deserialize)]
    struct Dto {
        merged: LabelSet,
        inherited: Vec<Inherited>,
    }
    let dto: Dto = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/labels/effective"),
        None,
    )
    .await?;
    Ok(EffectiveLabels {
        merged: dto.merged,
        inherited: dto
            .inherited
            .into_iter()
            .map(|i| (i.kind, i.id, i.labels))
            .collect(),
    })
}

// ─────────────────────────── containment ───────────────────────────

/// Nœud de l'arbre (parent/chemin/enfants).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RefNode {
    pub kind: String,
    pub id: String,
    /// Nom d'affichage (folders uniquement).
    pub name: Option<String>,
    pub emoji: Option<String>,
    pub sort_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Containment {
    pub parent: Option<RefNode>,
    /// Chemin feuille → racine (parent exclu).
    pub path: Vec<RefNode>,
    pub children: Vec<RefNode>,
}

/// `GET …/containment`.
pub async fn get_containment(kind: &str, id: &str) -> Result<Containment, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/containment"),
        None,
    )
    .await
}

/// `PUT …/containment` — (re)parente ; `None` = détache.
pub async fn set_parent(
    kind: &str,
    id: &str,
    parent: Option<(&str, &str)>,
) -> Result<Containment, ApiError> {
    let body = match parent {
        Some((pk, pi)) => serde_json::json!({"parent": {"kind": pk, "id": pi}}),
        None => serde_json::json!({"parent": null}),
    };
    client::request(
        reqwest::Method::PUT,
        &format!("/api/v1/resources/{kind}/{id}/containment"),
        Some(body),
    )
    .await
}

// ─────────────────────────── edges ───────────────────────────

/// `GET /api/v1/resources/edges?relation=&source_kind=&source_id=…`.
pub async fn list_edges(
    relation: Option<&str>,
    source: Option<(&str, &str)>,
    target: Option<(&str, &str)>,
) -> Result<Vec<Edge>, ApiError> {
    #[derive(Deserialize)]
    struct Page {
        results: Vec<Edge>,
    }
    let mut params: Vec<String> = Vec::new();
    if let Some(r) = relation {
        params.push(format!("relation={r}"));
    }
    if let Some((k, i)) = source {
        params.push(format!("source_kind={k}&source_id={i}"));
    }
    if let Some((k, i)) = target {
        params.push(format!("target_kind={k}&target_id={i}"));
    }
    let page: Page = client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/edges?{}", params.join("&")),
        None,
    )
    .await?;
    Ok(page.results)
}

/// `POST /api/v1/resources/edges` — 201 ou 409 (doublon).
pub async fn create_edge(
    relation: &str,
    source: (&str, &str),
    target: (&str, &str),
    placement: Option<serde_json::Value>,
) -> Result<Edge, ApiError> {
    let (sk, si) = source;
    let (tk, ti) = target;
    client::request(
        reqwest::Method::POST,
        "/api/v1/resources/edges",
        Some(serde_json::json!({
            "relation": relation,
            "source_kind": sk,
            "source_id": si,
            "target_kind": tk,
            "target_id": ti,
            "placement": placement,
        })),
    )
    .await
}

/// `DELETE /api/v1/resources/edges/{id}`.
/// 204 sans corps → `request_opt` (un `request` lèverait « réponse vide »).
pub async fn delete_edge(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/resources/edges/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

// ─────────────────────────── folders ───────────────────────────

/// `GET /api/v1/resources/folders`.
pub async fn list_folders() -> Result<Vec<Folder>, ApiError> {
    #[derive(Deserialize)]
    struct Page {
        results: Vec<Folder>,
    }
    let page: Page =
        client::request(reqwest::Method::GET, "/api/v1/resources/folders", None).await?;
    Ok(page.results)
}

/// `POST /api/v1/resources/folders`.
pub async fn create_folder(name: &str, emoji: Option<&str>) -> Result<Folder, ApiError> {
    client::request(
        reqwest::Method::POST,
        "/api/v1/resources/folders",
        Some(serde_json::json!({"name": name, "emoji": emoji})),
    )
    .await
}

/// `DELETE /api/v1/resources/folders/{id}` — sous-arbre re-rooté.
/// 204 sans corps → `request_opt` (un `request` lèverait « réponse vide »).
pub async fn delete_folder(id: i64) -> Result<(), ApiError> {
    client::request_opt::<serde_json::Value>(
        reqwest::Method::DELETE,
        &format!("/api/v1/resources/folders/{id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// `PATCH /api/v1/resources/folders/{id}` — `None` = champ absent ;
/// `Some(None)` = efface l'emoji (idiom `Option<Option>` du contrôleur).
pub async fn patch_folder(
    id: i64,
    name: Option<&str>,
    emoji: Option<Option<&str>>,
) -> Result<Folder, ApiError> {
    let mut body = serde_json::Map::new();
    if let Some(name) = name {
        body.insert("name".into(), serde_json::json!(name));
    }
    if let Some(emoji) = emoji {
        body.insert(
            "emoji".into(),
            match emoji {
                Some(e) => serde_json::json!(e),
                None => serde_json::Value::Null,
            },
        );
    }
    client::request(
        reqwest::Method::PATCH,
        &format!("/api/v1/resources/folders/{id}"),
        Some(serde_json::Value::Object(body)),
    )
    .await
}

// ─────────────────────────── recherche ───────────────────────────

/// `POST /api/v1/resources/search` — labels effectifs cross-kind.
pub async fn search(label: &str, kinds: Option<Vec<&str>>) -> Result<Vec<SearchHit>, ApiError> {
    #[derive(Deserialize)]
    struct Page {
        results: Vec<SearchHit>,
    }
    let body = match kinds {
        Some(kinds) => serde_json::json!({"label": label, "kinds": kinds}),
        None => serde_json::json!({"label": label}),
    };
    let page: Page = client::request(
        reqwest::Method::POST,
        "/api/v1/resources/search",
        Some(body),
    )
    .await?;
    Ok(page.results)
}

/// `GET /api/v1/resources/{kind}/{id}/location` — site breadcrumb(s).
pub async fn location(
    kind: &str,
    id: &str,
) -> Result<pnex_core::resources::ResourceLocations, ApiError> {
    client::request(
        reqwest::Method::GET,
        &format!("/api/v1/resources/{kind}/{id}/location"),
        None,
    )
    .await
}
