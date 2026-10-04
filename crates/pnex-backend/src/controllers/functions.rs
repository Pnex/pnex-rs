//! Registre « Fonctions » — CRUD org-scoped + versioning append-only +
//! test en live. École `controllers/edge_refs.rs` : scoping org (404 masqué
//! cross-org), écriture gated `can_write()`, 400 champ-par-champ. Le test
//! spawn le binaire runtime (`--test-function`) — viewer autorisé (lecture).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use loco_rs::prelude::*;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde::Deserialize;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{function_versions, functions};
use crate::services::flow::FlowSettings;
use crate::services::functions::{
    create_function, latest_version, referencing_deployed_flows, save_new_version,
    spawn_function_check, spawn_function_test, FunctionWriteError,
};
use pnex_core::{
    CreateFunction, FunctionDetail, FunctionSummary, FunctionTestRequest, FunctionValidateRequest,
    FunctionVersionSummary, SaveFunctionVersion,
};

/// 400 champ-par-champ (école edge_refs).
fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

/// Writes: owner/admin/member (`OrgContext::can_write`).
// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 503 `flow_runtime` — panne d'infra du test en live (binaire absent,
/// délai dépassé, sortie illisible).
fn flow_runtime_503(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::SERVICE_UNAVAILABLE,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/functions")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(save_version).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/test", post(test))
        .add("/validate", post(validate))
}

// ───────────────────────────── DTO mappers ─────────────────────────────

/// FunctionDetail — le save_version renvoie la version fraîche (l'éditeur
/// recharge sans second GET).
fn detail_dto(f: &functions::Model, version: &function_versions::Model) -> FunctionDetail {
    FunctionDetail {
        id: f.id,
        org_id: f.org_id,
        name: f.name.clone(),
        language: crate::services::functions::parse_language(&f.language).unwrap_or_default(),
        description: f.description.clone(),
        current_version_number: version.version_number,
        code: version.code.clone(),
        inputs: serde_json::from_value(version.inputs.clone()).unwrap_or_default(),
        outputs: serde_json::from_value(version.outputs.clone()).unwrap_or_default(),
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

/// Résumé (liste, sans code).
fn summary_dto(f: &functions::Model, current: i64) -> FunctionSummary {
    FunctionSummary {
        id: f.id,
        org_id: f.org_id,
        name: f.name.clone(),
        language: crate::services::functions::parse_language(&f.language).unwrap_or_default(),
        description: f.description.clone(),
        current_version_number: current,
        created_at: f.created_at.to_rfc3339(),
        updated_at: f.updated_at.to_rfc3339(),
    }
}

// ─────────────────────────────── Handlers ───────────────────────────────

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/functions` — registre de l'org (name ASC). Le numéro de
/// version courante est calculé batch (une requête versions pour le lot).
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = functions::Entity::find()
        .filter(functions::Column::OrgId.eq(org.org.id))
        .order_by_asc(functions::Column::Name)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    // Numéros de version courants du lot : une seule requête versions.
    let ids: Vec<i64> = rows.iter().map(|f| f.id).collect();
    let all_versions = if ids.is_empty() {
        Vec::new()
    } else {
        function_versions::Entity::find()
            .filter(function_versions::Column::FunctionId.is_in(ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
    };
    let page_slice: Vec<FunctionSummary> = rows
        .iter()
        .map(|f| {
            let current = all_versions
                .iter()
                .filter(|v| v.function_id == f.id)
                .map(|v| v.version_number)
                .max()
                .unwrap_or(0);
            summary_dto(f, current)
        })
        .skip(page.slice(count as usize).0)
        .take(page.slice(count as usize).1)
        .collect();
    Ok(format::json(pagination::envelope(
        "/api/v1/functions",
        &[],
        page,
        count,
        page_slice,
    ))
    .into_response())
}

/// `POST /api/v1/functions` — fonction + version 1. 201 = FunctionDetail
/// (l'éditeur enchaîne sans second GET) ; 400 champ-par-champ.
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<CreateFunction>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "functions-write-forbidden",
            "Owner, admin or member role required to manage functions",
        ));
    }
    let (f, version) = match create_function(&ctx.db, org.org.id, &input).await {
        Ok(pair) => pair,
        Err(e) => return Ok(write_error_response(e)),
    };
    Ok((StatusCode::CREATED, format::json(detail_dto(&f, &version))).into_response())
}

/// 400 par champ / directive (école edge_refs : Field → clé du corps JSON).
fn write_error_response(e: FunctionWriteError) -> Response {
    match e {
        FunctionWriteError::Field(field, msg) => field_status(&field, &msg),
        FunctionWriteError::Directive(e) => {
            field_status("code", &format!("ligne {} : {}", e.line, e.message))
        }
        FunctionWriteError::Db(_) => Error::InternalServerError.into_response(),
        FunctionWriteError::Conflict { .. } => (
            StatusCode::CONFLICT,
            format::json(serde_json::json!({"error": "version_conflict"})),
        )
            .into_response(),
    }
}

/// Fonction de l'org demandée, sinon 404 (cross-org masqué).
async fn find_function(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: i64,
) -> Result<Option<functions::Model>> {
    let row = functions::Entity::find_by_id(id)
        .filter(functions::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(row)
}

/// `GET /api/v1/functions/{id}` — fonction + version courante (code +
/// interface extraite).
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = latest_version(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    Ok(format::json(detail_dto(&f, &version)).into_response())
}

/// `PATCH /api/v1/functions/{id}` — nouvelle version (append-only) et/ou
/// métadonnées. 409 si `expected_version_number` ≠ version courante.
async fn save_version(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(input): Json<SaveFunctionVersion>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "functions-write-forbidden",
            "Owner, admin or member role required to manage functions",
        ));
    }
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // Concurrence optimiste (école flows) : la comparaison porte sur la plus
    // grande version enregistrée (append-only).
    let current = latest_version(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number)
        .unwrap_or(0);
    if input.expected_version_number != current {
        return Ok((
            StatusCode::CONFLICT,
            format::json(serde_json::json!({"error": "version_conflict"})),
        )
            .into_response());
    }
    let (f, new_version) = match save_new_version(&ctx.db, f, &input).await {
        Ok(pair) => pair,
        Err(e) => return Ok(write_error_response(e)),
    };
    let version = match new_version {
        Some(v) => v,
        None => latest_version(&ctx.db, f.id)
            .await
            .map_err(|_| Error::InternalServerError)?
            .ok_or(Error::NotFound)?,
    };
    Ok(format::json(detail_dto(&f, &version)).into_response())
}

/// `GET /api/v1/functions/{id}/versions` — historique (version DESC).
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let rows = function_versions::Entity::find()
        .filter(function_versions::Column::FunctionId.eq(f.id))
        .order_by_desc(function_versions::Column::VersionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<FunctionVersionSummary> = rows
        .iter()
        .map(|v| FunctionVersionSummary {
            id: v.id,
            version_number: v.version_number,
            note: v.note.clone(),
            inputs: serde_json::from_value(v.inputs.clone()).unwrap_or_default(),
            outputs: serde_json::from_value(v.outputs.clone()).unwrap_or_default(),
            created_at: v.created_at.to_rfc3339(),
            code: v.code.clone(),
        })
        .collect();
    Ok(
        format::json(serde_json::json!({ "count": results.len(), "results": results }))
            .into_response(),
    )
}

/// `DELETE /api/v1/functions/{id}` — 409 `function_in_use` si un flow
/// déployé référence la fonction (le graphe déployé doit rester exécutable)
/// ; sinon 204 (cascade versions via FK).
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "functions-write-forbidden",
            "Owner, admin or member role required to manage functions",
        ));
    }
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let referencing = referencing_deployed_flows(&ctx.db, f.id)
        .await
        .map_err(|_| Error::InternalServerError)?;
    if !referencing.is_empty() {
        let flows_json: Vec<serde_json::Value> = referencing
            .iter()
            .map(|(flow, version_number)| {
                serde_json::json!({
                    "flow_id": flow.id,
                    "flow_name": flow.name,
                    "version_number": version_number,
                })
            })
            .collect();
        return Ok((
            StatusCode::CONFLICT,
            format::json(serde_json::json!({
                "error": "function_in_use",
                "referencing_flows": flows_json,
            })),
        )
            .into_response());
    }
    f.delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/v1/functions/{id}/test` — viewer autorisé (outil de mise au
/// point en lecture). 503 `flow_runtime` si le runtime n'est pas activé ou
/// injoignable ; le résultat script voyage en 200 (`ok:false` + `error`).
async fn test(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<i64>,
    Json(req): Json<FunctionTestRequest>,
) -> Result<Response> {
    let Some(f) = find_function(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.enabled {
        return Err(flow_runtime_503(
            "functions-runtime-disabled",
            "The flow runtime is not enabled (settings.flow.enabled)",
        ));
    }
    // Per-pod cap on short-lived runtime children (shared with the deploy
    // pre-flight check): 503 server-busy past the wait budget.
    let _permit = crate::services::compute_limits::runtime_check()
        .acquire()
        .await?;
    match spawn_function_test(&settings, &ctx.db, org.org.id, f.id, &req).await {
        Ok(resp) => Ok(format::json(resp).into_response()),
        // Runtime/engine diagnostic: the raw message embeds arbitrary data —
        // verbatim display is the documented exception (err_codes doctrine).
        Err(msg) => Err(flow_runtime_503("flow_runtime", &msg)),
    }
}

/// `POST /api/v1/functions/validate` — validation compile-only du code
/// (barre d'erreurs de l'éditeur, aucune exécution). Viewer autorisé,
/// aucun accès DB : le corps porte langage + code. Même garde 503 que le
/// test (le binaire runtime est requis pour compiler).
async fn validate(
    State(ctx): State<AppContext>,
    _org: OrgContext,
    Json(req): Json<FunctionValidateRequest>,
) -> Result<Response> {
    let settings = FlowSettings::from_config(&ctx.config);
    if !settings.enabled {
        return Err(flow_runtime_503(
            "functions-runtime-disabled",
            "The flow runtime is not enabled (settings.flow.enabled)",
        ));
    }
    // Editor validation is debounced and retried on the next keystroke:
    // never queue it, reject at once when the runtime-check pool is full.
    let _permit = crate::services::compute_limits::runtime_check().try_acquire()?;
    match spawn_function_check(&settings, &req).await {
        Ok(resp) => Ok(format::json(resp).into_response()),
        // Runtime/engine diagnostic: verbatim display is the documented
        // exception (err_codes doctrine).
        Err(msg) => Err(flow_runtime_503("flow_runtime", &msg)),
    }
}
