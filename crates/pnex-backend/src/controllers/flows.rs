//! Flows ETL (D18) — CRUD versionné + déploiement du runtime, scoping org
//! (D2). Source de vérité = la base ; le `flows.json` n'est qu'un artefact
//! projeté à chaque deploy.
//!
//! Contrat versioning :
//! - **Enregistrer ≠ déployer** : `POST`/`PATCH` créent des versions
//!   (append-only) **sans toucher au runtime** ; le déploiement est une
//!   action explicite (`POST /{id}/deploy`) qui publie une version ;
//! - concurrence optimiste : `PATCH` porte `expected_version_number`, un
//!   enregistrement périmé est rejeté **409** (exigence PRD — écart assumé
//!   vs la convention 400 du reste du repo) ;
//! - rollback = redéploiement d'une version antérieure (`/rollback`) ;
//! - l'artefact projeté contient **tous** les flows `deployed` de
//!   l'instance (le runtime exécute un flows.json multi-tabs) — le deploy
//!   reprojette donc l'ensemble, pas seulement le flow publié.
//!
//! Erreurs : forme `{"detail": ...}` (comme devices) pour les 400 champ-par-
//! champ ; violations de graphe en 400 `{"violations": [...]}` ; 409/503 via
//! `Error::CustomError` (corps Loco `{"error": code, "description": msg}`,
//! patron `orgs.rs::conflict`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Set,
};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{flow_versions, flows};
use crate::services::flow::FlowSettings;
use crate::services::flow_supervisor;
use crate::services::openobserve;
use pnex_core::{FlowArtifactMeta, FlowGraph};

// ─────────────────────────── Aides ───────────────────────────

fn conflict(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new("conflict", msg.to_string()),
    )
}

fn forbidden(msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new("forbidden", msg.to_string()),
    )
}

/// Erreur 400 champ-par-champ, forme DRF `{"<champ>": "..."}`.
fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Flow de l'org courante, sinon None (→ 404).
async fn find_flow(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<flows::Model>> {
    flows::Entity::find_by_id(id)
        .filter(flows::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Dernière version d'un flow (numéro le plus élevé).
async fn latest_version(
    db: &DatabaseConnection,
    flow_id: i64,
) -> Result<Option<flow_versions::Model>> {
    flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow_id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

fn summary_dto(
    f: flows::Model,
    latest: i64,
    deployed_number: Option<i64>,
) -> pnex_core::FlowSummary {
    pnex_core::FlowSummary {
        id: f.id,
        org_id: f.org_id,
        device_id: f.device_registry_id,
        name: f.name,
        status: f.status,
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

fn flow_dto(
    f: flows::Model,
    graph: FlowGraph,
    latest: i64,
    deployed_number: Option<i64>,
) -> pnex_core::Flow {
    pnex_core::Flow {
        id: f.id,
        org_id: f.org_id,
        device_id: f.device_registry_id,
        name: f.name,
        status: f.status,
        deployed_version_number: deployed_number,
        latest_version_number: latest,
        graph,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

/// Numéro de version d'une ligne `flow_versions` (→ `deployed_version_id`).
async fn deployed_number_of(
    db: &DatabaseConnection,
    deployed_version_id: Option<i64>,
) -> Result<Option<i64>> {
    let Some(id) = deployed_version_id else {
        return Ok(None);
    };
    Ok(flow_versions::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number))
}

/// Validation de graphe → 400 `{"violations": [...]}` si invalide.
fn reject_invalid_graph(graph: &FlowGraph) -> Option<Response> {
    let violations = pnex_core::validate_graph(graph);
    if violations.is_empty() {
        return None;
    }
    Some(
        (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "violations": violations })),
        )
            .into_response(),
    )
}

/// Reprojette l'artefact de **tous** les flows `deployed` de l'instance et
/// demande au superviseur le rechargement. Erreur si le moteur est coupé ou
/// n'acquitte pas (l'état DB reste `deployed` — cohérent avec ce qui sera
/// relancé au prochain deploy/boot).
///
/// L'org O2 réelle (`openobserve_orgs.o2_org`) est résolue **en lecture
/// seule** (`provisioned_credentials`) : pas de provisioning sur le chemin
/// HTTP (doctrine du module O2). Une org sans O2 estampille `pnex_o2_org`
/// vide — les nœuds device/metric dégradent (warn, lecture/écriture sautée)
/// et la reprojection déclenchée par le sink après le provisioning comble
/// le champ (self-healing).
pub(crate) async fn reproject_and_signal(db: &DatabaseConnection) -> Result<()> {
    let deployed_flows = flows::Entity::find()
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut meta: Option<FlowArtifactMeta> = None;
    let mut o2_org_cache: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for f in &deployed_flows {
        let Some(deployed_id) = f.deployed_version_id else {
            continue;
        };
        let Some(version) = flow_versions::Entity::find_by_id(deployed_id)
            .one(db)
            .await
            .map_err(|_| Error::InternalServerError)?
        else {
            continue;
        };
        let graph: FlowGraph = serde_json::from_value(version.graph.clone())
            .map_err(|_| Error::InternalServerError)?;
        // Résolution par org (une seule requête par org du lot).
        let o2_org = match o2_org_cache.get(&f.org_id) {
            Some(cached) => cached.clone(),
            None => {
                let resolved = openobserve::provisioned_credentials(db, f.org_id)
                    .await
                    .unwrap_or(None)
                    .map(|c| c.o2_org)
                    .unwrap_or_default();
                o2_org_cache.insert(f.org_id, resolved.clone());
                resolved
            }
        };
        let m = FlowArtifactMeta {
            flow_id: f.id,
            version_number: version.version_number,
            org_id: f.org_id,
            o2_org,
        };
        if meta.is_none() {
            meta = Some(m.clone());
        }
        if let serde_json::Value::Array(nodes) = pnex_core::to_red_flows_json(&graph, &m) {
            entries.extend(nodes);
        }
    }

    flow_supervisor::deploy(
        serde_json::Value::Array(entries),
        meta.unwrap_or_else(FlowArtifactMeta::empty),
    )
    .await
    .map_err(|e| {
        Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            loco_rs::controller::ErrorDetail::new("flow_runtime", e),
        )
    })
}

// ─────────────────────────── POST /flows ───────────────────────────

/// `POST /api/v1/flows` — crée le flow **et sa version 1** (une transaction).
/// Aucun effet runtime : le flow reste `draft` jusqu'au deploy.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateFlow>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les flows"));
    }
    let name = params.name.trim();
    if name.is_empty() {
        return Ok(field_status("name", "This field is required."));
    }
    if name.chars().count() > 200 {
        return Ok(field_status(
            "name",
            "Ensure this field has no more than 200 characters.",
        ));
    }
    if let Some(response) = reject_invalid_graph(&params.graph) {
        return Ok(response);
    }
    // Attachement device optionnel : doit appartenir à l'org.
    if let Some(device_id) = params.device_id {
        let known = crate::models::_entities::device_registries::Entity::find()
            .filter(crate::models::_entities::device_registries::Column::OrgId.eq(org.org.id))
            .filter(crate::models::_entities::device_registries::Column::Id.eq(device_id))
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .is_some();
        if !known {
            return Ok(field_status(
                "device_id",
                "Device inconnu pour cette organisation.",
            ));
        }
    }

    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let flow = flows::ActiveModel {
        name: Set(name.to_string()),
        status: Set(pnex_core::FLOW_STATUS_DRAFT.to_string()),
        org_id: Set(org.org.id),
        device_registry_id: Set(params.device_id),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(serde_json::to_value(&params.graph).map_err(|_| Error::InternalServerError)?),
        author: Set(params.author),
        note: Set(params.note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;

    tracing::info!(flow_id = flow.id, org_id = org.org.id, "flow créé (v1)");
    Ok((
        StatusCode::CREATED,
        format::json(flow_dto(flow, params.graph, 1, None)),
    )
        .into_response())
}

// ─────────────────────────── GET /flows ───────────────────────────

#[derive(Debug, Default, Deserialize)]
struct ListFlowsQuery {
    search: Option<String>,
    status: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/flows` — flows de l'org, paginés (D14), filtres
/// `search` (nom) et `status`.
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListFlowsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org.org.id))
        .order_by_desc(flows::Column::Id);
    if let Some(status) = q.status.as_deref().filter(|s| !s.is_empty()) {
        query = query.filter(flows::Column::Status.eq(status));
    }
    let rows = query
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let filtered: Vec<&flows::Model> = rows
        .iter()
        .filter(|f| pagination::rust_search_match(&q.search, &[&f.name]))
        .collect();
    let count = filtered.len() as i64;
    let (skip, take) = page.slice(filtered.len());

    // Hydratation en vrac (pas de N+1) : dernier + numéro déployé par flow.
    let page_rows: Vec<flows::Model> = filtered
        .into_iter()
        .skip(skip)
        .take(take)
        .cloned()
        .collect();
    let ids: Vec<i64> = page_rows.iter().map(|f| f.id).collect();
    let latest: std::collections::HashMap<i64, i64> = flow_versions::Entity::find()
        .select_only()
        .column(flow_versions::Column::FlowId)
        .column_as(flow_versions::Column::VersionNumber.max(), "latest")
        .filter(flow_versions::Column::FlowId.is_in(ids.clone()))
        .group_by(flow_versions::Column::FlowId)
        .into_tuple::<(i64, i64)>()
        .all(&ctx.db)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect();
    let deployed_ids: Vec<i64> = page_rows
        .iter()
        .filter_map(|f| f.deployed_version_id)
        .collect();
    let deployed_numbers: std::collections::HashMap<i64, i64> = if deployed_ids.is_empty() {
        Default::default()
    } else {
        flow_versions::Entity::find()
            .filter(flow_versions::Column::Id.is_in(deployed_ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|v| (v.id, v.version_number))
            .collect()
    };
    let results: Vec<pnex_core::FlowSummary> = page_rows
        .into_iter()
        .map(|f| {
            let latest_n = latest.get(&f.id).copied().unwrap_or(0);
            let deployed_n = f
                .deployed_version_id
                .and_then(|id| deployed_numbers.get(&id).copied());
            summary_dto(f, latest_n, deployed_n)
        })
        .collect();

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("status".to_string(), s.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/flows",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /flows/{id} ───────────────────────────

/// `GET /api/v1/flows/{id}` — détail + graphe de la dernière version.
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let latest = latest_version(&ctx.db, flow.id).await?;
    let latest_number = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
    let graph: FlowGraph = latest
        .map(|v| serde_json::from_value(v.graph).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    let deployed_number = deployed_number_of(&ctx.db, flow.deployed_version_id).await?;
    Ok(format::json(flow_dto(flow, graph, latest_number, deployed_number)).into_response())
}

// ─────────────────────────── PATCH /flows/{id} ───────────────────────────

/// `PATCH /api/v1/flows/{id}` — enregistre une **nouvelle version**
/// (append-only). 409 si `expected_version_number` ≠ version courante.
/// Aucun rechargement runtime — l'exécution ne change qu'au deploy.
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(params): Json<pnex_core::UpdateFlow>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les flows"));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    if let Some(response) = reject_invalid_graph(&params.graph) {
        return Ok(response);
    }
    let Some(latest) = latest_version(&ctx.db, flow.id).await? else {
        return Err(Error::NotFound);
    };
    if params.expected_version_number != latest.version_number {
        return Err(conflict(&format!(
            "version périmée : attendu {}, version courante {} — rechargez la dernière version",
            params.expected_version_number, latest.version_number
        )));
    }

    let txn = ctx
        .db
        .begin()
        .await
        .map_err(|_| Error::InternalServerError)?;
    let mut active: flows::ActiveModel = flow.clone().into();
    if let Some(name) = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        active.name = Set(name.to_string());
    }
    let flow = active
        .update(&txn)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let new_version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(latest.version_number + 1),
        graph: Set(serde_json::to_value(&params.graph).map_err(|_| Error::InternalServerError)?),
        author: Set(params.author),
        note: Set(params.note),
        ..Default::default()
    }
    .insert(&txn)
    .await
    .map_err(|_| Error::InternalServerError)?;
    txn.commit().await.map_err(|_| Error::InternalServerError)?;

    tracing::info!(
        flow_id = flow.id,
        version = new_version.version_number,
        "flow enregistré (nouvelle version)"
    );
    Ok(format::json(flow_dto(
        flow,
        params.graph,
        new_version.version_number,
        None,
    ))
    .into_response())
}

// ─────────────────────────── DELETE /flows/{id} ───────────────────────────

/// `DELETE /api/v1/flows/{id}` — 204 ; versions supprimées en cascade. Si le
/// flow était déployé, l'artefact est reprojeté sans lui (runtime rechargé).
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour gérer les flows"));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let was_deployed = flow.deployed_version_id.is_some();
    flows::Entity::delete_by_id(flow.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if was_deployed {
        reproject_and_signal(&ctx.db).await?;
    }
    tracing::info!(flow_id = flow.id, "flow supprimé");
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── GET /flows/{id}/versions ───────────────────────────

/// `GET /api/v1/flows/{id}/versions` — historique append-only, paginé (D14),
/// du plus récent au plus ancien.
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Query(q): Query<VersionsQuery>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .order_by_desc(flow_versions::Column::VersionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    let results: Vec<pnex_core::FlowVersionSummary> = rows
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|v| {
            let deployed = flow.deployed_version_id == Some(v.id);
            pnex_core::FlowVersionSummary {
                id: v.id,
                version_number: v.version_number,
                author: v.author,
                note: v.note,
                deployed,
                created_at: v.created_at.to_rfc3339(),
            }
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/flows/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

#[derive(Debug, Default, Deserialize)]
struct VersionsQuery {
    limit: Option<String>,
    offset: Option<String>,
}

// ─────────────────────────── GET /flows/{id}/versions/{n} ───────────────────────────

/// `GET /api/v1/flows/{id}/versions/{n}` — graphe d'une version précise
/// (rollback ou audit).
async fn version_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(i64, i64)>,
) -> Result<Response> {
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .filter(flow_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let graph: FlowGraph =
        serde_json::from_value(version.graph.clone()).map_err(|_| Error::InternalServerError)?;
    Ok(format::json(pnex_core::FlowVersionDetail {
        id: version.id,
        version_number: version.version_number,
        author: version.author,
        note: version.note,
        deployed: flow.deployed_version_id == Some(version.id),
        created_at: version.created_at.to_rfc3339(),
        graph,
    })
    .into_response())
}

// ─────────────────────────── Stop automatique (dépendance pin ↔ flows) ───────────────────────────

/// Dépendances pin ↔ flows (Phase 6) : un changement de mode d'un pin
/// (in↔out) invalide les lectures device des flows déployés — la série O2
/// n'est plus alimentée, le pipeline ETL tournerait à vide ou sur des
/// valeurs figées. Pour chaque flow **déployé** de l'org dont un nœud
/// `device` lit ce (device_id, pin) : dé-déploiement immédiat (status →
/// draft, `deployed_version_id` → NULL — la version publiée reste
/// enregistrée, un redéploiement manuel est possible une fois la
/// configuration cohérente) puis reprojection unique de l'artefact.
///
/// Retourne les impacts `(flow_id, nom)` pour l'UI ; appelée par le
/// contrôleur pins (`set_mode`) **avant** le push device (la base est la
/// source de vérité, le stop doit refléter la base même si le device est
/// hors ligne).
pub(crate) async fn stop_flows_reading_pin(
    ctx: &AppContext,
    org_id: i64,
    device_id: &str,
    pin_label: &str,
) -> Result<Vec<(i64, String)>> {
    let deployed = flows::Entity::find()
        .filter(flows::Column::OrgId.eq(org_id))
        .filter(flows::Column::Status.eq(pnex_core::FLOW_STATUS_DEPLOYED))
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    let mut impacts: Vec<(i64, String)> = Vec::new();
    for flow in deployed {
        let Some(version_id) = flow.deployed_version_id else {
            continue;
        };
        let Some(version) = flow_versions::Entity::find_by_id(version_id)
            .one(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
        else {
            continue;
        };
        let Ok(graph) = serde_json::from_value::<FlowGraph>(version.graph) else {
            continue;
        };
        let touches = graph.nodes.iter().any(|n| matches!(
            &n.kind,
            pnex_core::FlowNodeKind::Device { config } if config.reads.iter().any(|r| {
                r.device_id == device_id && pnex_core::normalize_measurement_name(&r.pin) == pnex_core::normalize_measurement_name(pin_label)
            })
        ));
        if !touches {
            continue;
        }
        let mut active: flows::ActiveModel = flow.into();
        active.status = Set(pnex_core::FLOW_STATUS_DRAFT.to_string());
        active.deployed_version_id = Set(None);
        let stopped = active
            .update(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        tracing::warn!(
            flow_id = stopped.id,
            device_id,
            pin = pin_label,
            "flow dé-déployé automatiquement : le pin vient de changer de mode (in↔out)"
        );
        impacts.push((stopped.id, stopped.name));
    }

    // Un seul rechargement du runtime même si plusieurs flows ont été
    // arrêtés — l'artefact est reprojeté sans eux.
    if !impacts.is_empty() {
        reproject_and_signal(&ctx.db).await?;
    }
    Ok(impacts)
}

// ─────────────────────────── POST /flows/{id}/deploy | /rollback ───────────────────────────

/// `POST /flows/{id}/deploy` — publie une version (`version_number` absent =
/// dernière) : reprojection de l'ensemble des flows déployés → SIGUSR1 →
/// `deployed_version_id` mis à jour.
async fn deploy(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    deploy_version(ctx, org, id, body).await
}

/// `POST /flows/{id}/rollback` — alias explicite du deploy d'une version
/// antérieure (même mécanique, intention produit distincte).
async fn rollback(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    deploy_version(ctx, org, id, body).await
}

async fn deploy_version(
    ctx: AppContext,
    org: OrgContext,
    id: i64,
    body: Option<Json<pnex_core::DeployFlow>>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour déployer les flows"));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let version_number = match body.and_then(|Json(p)| p.version_number) {
        Some(n) => n,
        // Version absente → dernière version déployée par défaut.
        None => {
            let Some(latest) = latest_version(&ctx.db, flow.id).await? else {
                return Err(Error::NotFound);
            };
            latest.version_number
        }
    };
    let Some(version) = flow_versions::Entity::find()
        .filter(flow_versions::Column::FlowId.eq(flow.id))
        .filter(flow_versions::Column::VersionNumber.eq(version_number))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };

    let mut active: flows::ActiveModel = flow.clone().into();
    active.deployed_version_id = Set(Some(version.id));
    active.status = Set(pnex_core::FLOW_STATUS_DEPLOYED.to_string());
    let flow = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Reprojection de TOUS les flows déployés + rechargement (503 explicite
    // si settings.flow.enabled=false ou acquittement absent).
    reproject_and_signal(&ctx.db).await?;

    let latest_number = latest_version(&ctx.db, flow.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    let graph: FlowGraph =
        serde_json::from_value(version.graph).map_err(|_| Error::InternalServerError)?;
    tracing::info!(flow_id = flow.id, version = version_number, "flow déployé");
    Ok(format::json(flow_dto(flow, graph, latest_number, Some(version_number))).into_response())
}

// ─────────────────────────── GET /flows/{id}/runtime ───────────────────────────

/// `GET /flows/{id}/runtime` — état du runtime vu par le superviseur
/// (pid, version déployée, rechargements). 404 si le flow n'est pas de l'org.
async fn runtime(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(_) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let settings = FlowSettings::from_config(&ctx.config);
    let mut status = flow_supervisor::runtime_status(&settings);
    // Point de vue du flow : ce que le runtime exécute pour lui.
    status.deployed_flow_id = Some(id);
    // Porte l'activation des outils de debug (mode dev/debug uniquement) :
    // l'éditeur masque panneau et run-once sans second endpoint.
    status.debug_tools = settings.debug_tools;
    Ok(format::json(status).into_response())
}

// ─────────────────────────── Debug (panneau) + run-once ───────────────────────────

/// `GET /flows/{id}/debug` — feed du panneau (100 dernières entrées du flow).
/// Lecture seule (même niveau que `runtime`), 200 avec feed vide si le
/// moteur est arrêté (un 503 rendrait le drawer inutilisable). Garde-fou :
/// 403 hors mode dev/debug (`settings.flow.debug_tools`).
async fn debug(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.debug_tools {
        return Err(forbidden("outils de debug désactivés (mode run)"));
    }
    let Some(_) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(pnex_core::FlowDebugFeed {
        flow_id: id,
        entries: flow_supervisor::debug_entries(id, 100),
    })
    .into_response())
}

/// `POST /flows/{id}/run-once` — exécute une fois le flow déployé :
/// `cmd.json` + SIGUSR2 vers le runtime, acquittement stdout corrélé.
/// Garde-fous : 403 hors mode dev/debug, `can_write`, 409 si non déployé,
/// 503 `flow_runtime` si le runtime n'acquitte pas.
async fn run_once(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.debug_tools {
        return Err(forbidden("outils de debug désactivés (mode run)"));
    }
    if !org.can_write() {
        return Err(forbidden("owner ou admin requis pour exécuter les flows"));
    }
    let Some(flow) = find_flow(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    if flow.status != pnex_core::FLOW_STATUS_DEPLOYED {
        return Err(conflict("le flow doit être déployé pour être exécuté"));
    }
    let result = flow_supervisor::run_once(id).await.map_err(|e| {
        Error::CustomError(
            StatusCode::SERVICE_UNAVAILABLE,
            loco_rs::controller::ErrorDetail::new("flow_runtime", e),
        )
    })?;
    tracing::info!(
        flow_id = id,
        injected = result.injected,
        "flow exécuté (run-once)"
    );
    Ok(format::json(result).into_response())
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/flows")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/versions/{version_number}", get(version_detail))
        .add("/{id}/deploy", post(deploy))
        .add("/{id}/rollback", post(rollback))
        .add("/{id}/runtime", get(runtime))
        .add("/{id}/debug", get(debug))
        .add("/{id}/run-once", post(run_once))
}
