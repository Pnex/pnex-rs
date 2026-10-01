//! Couche d'organisation transverse (D42) — endpoints **kind-agnostiques**.
//!
//! Les règles par kind vivent dans le registre
//! (`services/resources/registry.rs`) ; le contrôleur ne fait jamais de
//! match sur les kinds.
//!
//! - 400 per-field shape (flows/media/pois school);
//! - 404 masqué cross-org (kind inconnu → 400, ressource d'une autre org
//!   → 404) ;
//! - writes gated by `can_write()` (owner|admin|member);
//! - routes statiques avant paramétriques (école pois `cluster`) :
//!   `labels/catalog`, `edges`, `folders` avant `{kind}/{id}/…`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::OrgContext;
use crate::services::resources as svc;
use pnex_core::{
    err_codes,
    resources::{parse_label_filter, valid_kind},
};

// ─────────────────────────── erreurs ───────────────────────────

/// Forbidden with the machine code + canonical English description
/// (frontend resolves `err-<code>` at render time, verbatim fallback).
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Mapping des erreurs moteur → HTTP (formes cohérentes pois/media).
fn error_response(e: svc::ResourceError) -> Response {
    use svc::ResourceError as E;
    match e {
        E::KindInvalid => field_status("kind", "kind inconnu de la couche d'organisation."),
        E::ResourceUnknown => Error::NotFound.into_response(),
        E::LabelInvalid(msg) => field_status("labels", &msg),
        E::CycleDetected => field_status(
            "parent",
            "cycle détecté (une ressource ne peut pas être son propre ancêtre).",
        ),
        E::RelationInvalid => {
            field_status("relation", "relation non déclarée pour ce kind source.")
        }
        E::TargetInvalid => field_status("target", "cible non admise par la spec de ce kind."),
        E::Conflict => (
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "resource-edge-duplicate",
                "description": "An edge already exists for this pair."
            })),
        )
            .into_response(),
        E::PlacementInvalid(msg) => field_status("placement", msg),
        E::Db => Error::InternalServerError.into_response(),
    }
}

/// Préambule commun des routes `{kind}/{id}/…` : kind inconnu → 400,
/// ressource absente / cross-org → 404 (résolution via le registre).
async fn require_resource(ctx: &AppContext, org: &OrgContext, kind: &str, id: &str) -> Result<()> {
    if !valid_kind(kind) {
        return Err(Error::BadRequest("unknown kind".into()));
    }
    let Some(entry) = svc::registry::global().entry(kind) else {
        return Err(Error::BadRequest("unknown kind".into()));
    };
    let ok = entry
        .resolver
        .resolve(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !ok {
        return Err(Error::NotFound);
    }
    Ok(())
}

// ─────────────────────────── labels ───────────────────────────

#[derive(Serialize)]
struct LabelsDto {
    kind: String,
    id: String,
    labels: pnex_core::resources::LabelSet,
}

fn labels_dto(kind: &str, id: &str, labels: &pnex_core::resources::LabelSet) -> LabelsDto {
    LabelsDto {
        kind: kind.to_string(),
        id: id.to_string(),
        labels: labels.clone(),
    }
}

#[derive(Serialize)]
struct EffectiveLabelsDto {
    kind: String,
    id: String,
    /// Fusion « le plus proche gagne ».
    merged: pnex_core::resources::LabelSet,
    /// Héritage détaillé (feuille → racine) — « hérité de Serre ».
    inherited: Vec<InheritedFrom>,
}

#[derive(Serialize)]
struct InheritedFrom {
    kind: String,
    id: String,
    labels: pnex_core::resources::LabelSet,
}

#[derive(Serialize)]
struct CatalogEntryDto {
    name: String,
    values: Vec<String>,
    count: i64,
}

/// `GET /api/v1/resources/labels/catalog` — autocomplétion (tous kinds).
async fn labels_catalog(ctx: State<AppContext>, org: OrgContext) -> Result<Response> {
    let rows = svc::labels::catalog(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<CatalogEntryDto> = rows
        .into_iter()
        .map(|(name, values, count)| CatalogEntryDto {
            name,
            values,
            count,
        })
        .collect();
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

/// `GET /api/v1/resources/{kind}/{id}/labels` — propres uniquement.
async fn get_labels(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let labels = svc::labels::get_labels(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .unwrap_or_default();
    format::json(labels_dto(&kind, &id, &labels))
}

/// `PUT /api/v1/resources/{kind}/{id}/labels` — remplace tout le doc.
async fn put_labels(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    require_resource(&ctx, &org, &kind, &id).await?;
    let Some(doc) = body.get("labels") else {
        return Ok(field_status("labels", err_codes::FIELD_REQUIRED));
    };
    let Some(doc) = doc.as_object() else {
        return Ok(field_status("labels", "objet JSON attendu"));
    };
    let mut labels = pnex_core::resources::LabelSet::new();
    for (name, value) in doc {
        let value = match value {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            _ => {
                return Ok(field_status("labels", "valeurs : string ou null (tag nu)"));
            }
        };
        labels.insert(name.clone(), value);
    }
    match svc::labels::put_labels(
        &ctx.db,
        org.org.id,
        &kind,
        &id,
        &labels,
        Some(org.auth.user.id),
    )
    .await
    {
        Ok(put) => format::json(labels_dto(&kind, &id, &put)),
        Err(e) => Ok(error_response(e)),
    }
}

/// `GET /api/v1/resources/{kind}/{id}/labels/effective` — fusion au read.
async fn get_effective_labels(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let (merged, inherited) = svc::labels::effective(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    format::json(EffectiveLabelsDto {
        kind,
        id,
        merged,
        inherited: inherited
            .into_iter()
            .map(|(kind, id, labels)| InheritedFrom { kind, id, labels })
            .collect(),
    })
}

// ─────────────────────────── containment ───────────────────────────

#[derive(Serialize)]
struct ContainmentDto {
    kind: String,
    id: String,
    /// Parent (`null` = racine).
    parent: Option<RefDto>,
    /// Chemin feuille → racine (breadcrumb, parent exclu).
    path: Vec<RefDto>,
    /// Enfants directs (ordre `sort_key`).
    children: Vec<RefDto>,
}

#[derive(Serialize)]
struct RefDto {
    kind: String,
    id: String,
    /// Nom d'affichage d'un folder (sans valeur pour les autres kinds).
    name: Option<String>,
    emoji: Option<String>,
    sort_key: Option<String>,
}

fn ref_dto(
    kind: &str,
    id: &str,
    name: Option<String>,
    emoji: Option<String>,
    sort_key: Option<String>,
) -> RefDto {
    RefDto {
        kind: kind.to_string(),
        id: id.to_string(),
        name,
        emoji,
        sort_key,
    }
}

#[derive(Deserialize)]
struct ContainmentPut {
    /// `{"kind": "folder", "id": "3"}` ou `null` (détacher).
    parent: Option<ParentRef>,
    /// Ordre dans le parent (optionnel).
    sort_key: Option<String>,
}

#[derive(Deserialize)]
struct ParentRef {
    kind: String,
    id: String,
}

/// `GET /api/v1/resources/{kind}/{id}/containment` — parent + chemin +
/// enfants.
async fn get_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    require_resource(&ctx, &org, &kind, &id).await?;
    let parent = svc::containment::parent_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let path = svc::containment::path_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Enfants + hydratation des noms d'affichage : folders (1 requête
    // batchée) via `folder_names`, autres kinds via le résolveur du registre
    // (appel unitaire par ref — listes d'arbre courtes).
    let children = svc::containment::children_of(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let folder_ids: Vec<i64> = children
        .iter()
        .filter(|(k, _, _)| k == pnex_core::resources::KIND_FOLDER)
        .filter_map(|(_, i, _)| i.parse::<i64>().ok())
        .collect();
    let names = svc::containment::folder_names(&ctx.db, org.org.id, &folder_ids)
        .await
        .map_err(|_| Error::InternalServerError)?;
    // Refs hors folder à nommer : parent + chemin + enfants (dédupliquées).
    let mut display = std::collections::HashMap::<String, String>::new();
    let mut refs: Vec<(&str, &str)> = Vec::new();
    if let Some((pk, pi)) = &parent {
        if pk != pnex_core::resources::KIND_FOLDER {
            refs.push((pk.as_str(), pi.as_str()));
        }
    }
    for (k, i) in &path {
        if k != pnex_core::resources::KIND_FOLDER {
            refs.push((k.as_str(), i.as_str()));
        }
    }
    for (k, i, _) in &children {
        if k != pnex_core::resources::KIND_FOLDER {
            refs.push((k.as_str(), i.as_str()));
        }
    }
    for (k, i) in refs {
        let key = format!("{k}:{i}");
        if display.contains_key(&key) {
            continue;
        }
        if let Some(entry) = svc::registry::global().entry(k) {
            if let Ok(Some(name)) = entry.resolver.display_name(&ctx.db, org.org.id, i).await {
                display.insert(key, name);
            }
        }
    }
    let parent_name: Option<(String, Option<String>)> = parent.as_ref().and_then(|(pk, pi)| {
        if pk == pnex_core::resources::KIND_FOLDER {
            pi.parse::<i64>()
                .ok()
                .and_then(|i| names.get(&i))
                .map(|(n, e)| (n.clone(), e.clone()))
        } else {
            display
                .get(&format!("{pk}:{pi}"))
                .cloned()
                .map(|n| (n, None))
        }
    });
    format::json(ContainmentDto {
        kind: kind.clone(),
        id: id.clone(),
        parent: parent.map(|(pk, pi)| {
            let name = parent_name.clone().map(|(n, _)| n);
            let emoji = parent_name.and_then(|(_, e)| e);
            ref_dto(&pk, &pi, name, emoji, None)
        }),
        path: path
            .iter()
            .map(|(pk, pi)| {
                let (name, emoji) = ref_name(&display, &names, pk, pi);
                ref_dto(pk, pi, name, emoji, None)
            })
            .collect(),
        children: children
            .iter()
            .map(|(k, i, sk)| {
                let (name, emoji) = ref_name(&display, &names, k, i);
                ref_dto(k, i, name, emoji, sk.clone())
            })
            .collect(),
    })
}

/// (nom, emoji) d'une ref : folders via `folder_names`, autres kinds via les
/// noms résolus par les résolveurs du registre.
fn ref_name(
    display: &std::collections::HashMap<String, String>,
    names: &std::collections::HashMap<i64, (String, Option<String>)>,
    kind: &str,
    id: &str,
) -> (Option<String>, Option<String>) {
    if kind == pnex_core::resources::KIND_FOLDER {
        return id
            .parse::<i64>()
            .ok()
            .and_then(|i| names.get(&i))
            .map(|(n, e)| (Some(n.clone()), e.clone()))
            .unwrap_or((None, None));
    }
    (display.get(&format!("{kind}:{id}")).cloned(), None)
}

/// `PUT /api/v1/resources/{kind}/{id}/containment` — (re)parente ; cycle →
/// 400 ; détache si `parent: null` (le sous-arbre suit, rien n'est supprimé).
async fn put_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
    Json(p): Json<ContainmentPut>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    require_resource(&ctx, &org, &kind, &id).await?;
    let parent = p.parent.map(|r| (r.kind, r.id));
    let parent = parent.as_ref().map(|(k, i)| (k.as_str(), i.as_str()));
    match svc::containment::set_parent(
        &ctx.db,
        org.org.id,
        &kind,
        &id,
        parent,
        p.sort_key.as_deref(),
    )
    .await
    {
        Ok(()) => get_containment(ctx, org, Path((kind, id))).await,
        Err(e) => Ok(error_response(e)),
    }
}

/// `DELETE /api/v1/resources/{kind}/{id}/containment` — détache.
async fn delete_containment(
    ctx: State<AppContext>,
    org: OrgContext,
    Path((kind, id)): Path<(String, String)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    require_resource(&ctx, &org, &kind, &id).await?;
    let detached = svc::containment::detach(&ctx.db, org.org.id, &kind, &id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !detached {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── edges ───────────────────────────

#[derive(Serialize)]
struct EdgeDto {
    id: i64,
    relation: String,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    placement: Option<Value>,
    created_at: String,
}

fn edge_dto(e: &crate::models::_entities::resource_edges::Model) -> EdgeDto {
    EdgeDto {
        id: e.id,
        relation: e.relation.clone(),
        source_kind: e.source_kind.clone(),
        source_id: e.source_id.clone(),
        target_kind: e.target_kind.clone(),
        target_id: e.target_id.clone(),
        placement: e.placement.clone(),
        created_at: e.created_at.to_rfc3339(),
    }
}

#[derive(Deserialize)]
struct EdgeListQuery {
    relation: Option<String>,
    source_kind: Option<String>,
    source_id: Option<String>,
    target_kind: Option<String>,
    target_id: Option<String>,
}

/// `GET /api/v1/resources/edges` — arêtes filtrées.
async fn list_edges(
    ctx: State<AppContext>,
    org: OrgContext,
    Query(q): Query<EdgeListQuery>,
) -> Result<Response> {
    let rows = svc::edges::list_edges(
        &ctx.db,
        org.org.id,
        q.relation.as_deref(),
        match (&q.source_kind, &q.source_id) {
            (Some(k), Some(i)) => Some((k.as_str(), i.as_str())),
            _ => None,
        },
        match (&q.target_kind, &q.target_id) {
            (Some(k), Some(i)) => Some((k.as_str(), i.as_str())),
            _ => None,
        },
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    let results: Vec<EdgeDto> = rows.iter().map(edge_dto).collect();
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

#[derive(Deserialize)]
struct EdgeCreate {
    relation: String,
    source_kind: String,
    source_id: String,
    target_kind: String,
    target_id: String,
    placement: Option<Value>,
}

/// `POST /api/v1/resources/edges` — validité par kind (spec), doublon 409.
async fn create_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(p): Json<EdgeCreate>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    match svc::edges::create_edge(
        &ctx.db,
        org.org.id,
        &p.relation,
        &p.source_kind,
        &p.source_id,
        &p.target_kind,
        &p.target_id,
        p.placement.as_ref(),
    )
    .await
    {
        Ok(edge) => {
            let dto = edge_dto(&edge);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(error_response(e)),
    }
}

#[derive(Deserialize)]
struct EdgePlacementPatch {
    placement: Value,
}

/// `PATCH /api/v1/resources/edges/{id}` — placement (déplacement du
/// hotspot, overrides).
async fn patch_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<EdgePlacementPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    match svc::edges::update_placement(&ctx.db, org.org.id, id, &p.placement).await {
        Ok(Some(edge)) => format::json(edge_dto(&edge)),
        Ok(None) => Err(Error::NotFound),
        Err(e) => Ok(error_response(e)),
    }
}

/// `DELETE /api/v1/resources/edges/{id}` — 204 ; 404 masqué cross-org.
async fn delete_edge(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    let deleted = svc::edges::delete_edge(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !deleted {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── folders ───────────────────────────

#[derive(Serialize)]
struct FolderDto {
    id: i64,
    name: String,
    emoji: Option<String>,
    created_at: String,
    updated_at: String,
}

fn folder_dto(f: &crate::models::_entities::resource_folders::Model) -> FolderDto {
    FolderDto {
        id: f.id,
        name: f.name.clone(),
        emoji: f.emoji.clone(),
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

#[derive(Deserialize)]
struct FolderCreate {
    name: String,
    emoji: Option<String>,
}

/// `POST /api/v1/resources/folders`.
async fn create_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(p): Json<FolderCreate>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    match svc::folders::create_folder(
        &ctx.db,
        org.org.id,
        &p.name,
        p.emoji.as_deref(),
        Some(org.auth.user.id),
    )
    .await
    {
        Ok(f) => {
            let dto = folder_dto(&f);
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(error_response(e)),
    }
}

/// `GET /api/v1/resources/folders` — tous les folders de l'org.
async fn list_folders(ctx: State<AppContext>, org: OrgContext) -> Result<Response> {
    let rows = svc::folders::list_folders(&ctx.db, org.org.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<FolderDto> = rows.iter().map(folder_dto).collect();
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

#[derive(Deserialize)]
struct FolderPatch {
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    emoji: Option<Option<String>>,
}

/// Idiome `Option<Option<T>>` (école pois).
fn deserialize_some<'de, T, D>(d: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    serde::Deserialize::deserialize(d).map(Some)
}

/// `PATCH /api/v1/resources/folders/{id}`.
async fn patch_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(p): Json<FolderPatch>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    match svc::folders::update_folder(
        &ctx.db,
        org.org.id,
        id,
        svc::folders::FolderPatch {
            name: p.name.as_deref(),
            emoji: p.emoji.as_ref().map(|o| o.as_deref()),
        },
    )
    .await
    {
        Ok(Some(f)) => format::json(folder_dto(&f)),
        Ok(None) => Err(Error::NotFound),
        Err(e) => Ok(error_response(e)),
    }
}

/// `DELETE /api/v1/resources/folders/{id}` — purge organisation (sous-arbre
/// re-rooté), ligne entité supprimée.
async fn delete_folder(
    ctx: State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "resource-write-forbidden",
            "Write access restricted to owner/admin/member roles.",
        ));
    }
    let deleted = svc::folders::delete_folder(&ctx.db, org.org.id, id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !deleted {
        return Err(Error::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── recherche cross-kind ───────────────────────────

#[derive(Deserialize)]
struct SearchBody {
    /// Kinds ciblés (absent = tous les kinds vivants).
    kinds: Option<Vec<String>>,
    /// Filtre label effectif `name` ou `name:valeur`.
    label: String,
}

#[derive(Serialize)]
struct SearchResultItem {
    kind: String,
    id: String,
}

/// `POST /api/v1/resources/search` — ressources de l'org portant le label
/// effectivement (propres OU hérités), cap 500.
async fn search(
    ctx: State<AppContext>,
    org: OrgContext,
    Json(p): Json<SearchBody>,
) -> Result<Response> {
    let (filter, err) = parse_label_filter(&p.label);
    let Some(filter) = filter else {
        return Ok(field_status(
            "label",
            err.as_deref().unwrap_or("label invalide"),
        ));
    };
    let kinds: Vec<String> = match p.kinds {
        Some(kinds) if !kinds.is_empty() => {
            for k in &kinds {
                if !valid_kind(k) {
                    return Ok(field_status("kinds", "kind inconnu"));
                }
            }
            kinds
        }
        _ => svc::registry::global()
            .kinds()
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    let mut results = Vec::new();
    for kind in &kinds {
        let ids = svc::labels::ids_with_effective_label(&ctx.db, org.org.id, &filter, Some(kind))
            .await
            .map_err(|_| Error::InternalServerError)?;
        for (k, id) in ids {
            results.push(SearchResultItem { kind: k, id });
        }
        if results.len() >= 500 {
            break;
        }
    }
    results.truncate(500);
    format::json(serde_json::json!({ "count": results.len(), "results": results }))
}

// ─────────────────────────── routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/resources")
        // Statiques avant paramétriques (matchit — école pois).
        .add("/labels/catalog", get(labels_catalog))
        .add("/edges", get(list_edges).post(create_edge))
        .add(
            "/edges/{id}",
            axum::routing::patch(patch_edge).delete(delete_edge),
        )
        .add("/folders", get(list_folders).post(create_folder))
        .add(
            "/folders/{id}",
            axum::routing::patch(patch_folder).delete(delete_folder),
        )
        .add("/search", post(search))
        .add("/{kind}/{id}/labels", get(get_labels).put(put_labels))
        .add("/{kind}/{id}/labels/effective", get(get_effective_labels))
        .add(
            "/{kind}/{id}/containment",
            get(get_containment)
                .put(put_containment)
                .delete(delete_containment),
        )
}
