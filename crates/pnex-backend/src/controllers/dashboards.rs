//! Dashboards du studio SCADA (D24/D40/D41) — CRUD versionné, scoping
//! org (D2), pluriel (`/api/v1/dashboards`) pour ne pas confondre avec
//! `/api/v1/dashboard` (singulier = résumé télémétrie existant).
//!
//! Contrat versioning (école `flows.rs`, adapté D24) :
//! - `PATCH /{id}` = **save** : insère une version (append-only) **et**
//!   suit le pointeur `current_version_number` — save = live, pas de
//!   publish séparé en V1 ;
//! - concurrence optimiste : `expected_version_number` vise la version
//!   **courante** (le pointeur), un save/restauration concurrent est
//!   rejeté **409** (écart assumé vs la convention 400 du repo) ;
//! - `POST /{id}/versions/{n}/restore` = re-positionne le pointeur
//!   (école media) — n'insère rien ;
//! - zéro runtime à acquitter : le « deploy » d'un dashboard est un
//!   swap de pointeur en base, les viewers le lisent au prochain polling.
//!
//! Erreurs : 400 champ-par-champ `{"<champ>": ...}` ; violations de
//! layout en 400 `{"violations": [...]}` ; 409 via `Error::CustomError`
//! (patron `orgs.rs::conflict`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::format;
use loco_rs::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{dashboard_versions, dashboards};
use crate::services::dashboards::DashboardWriteError;
use pnex_core::err_codes;
use pnex_core::{DashboardLayout, VizDashboard, VizDashboardSummary, VizDashboardVersion};

// ─────────────────────────── Aides ───────────────────────────

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.

/// Dashboard de l'org courante, sinon None (→ 404 masqué cross-org).
async fn find_dashboard(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<dashboards::Model>> {
    dashboards::Entity::find_by_id(id)
        .filter(dashboards::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Layout d'une version donnée (None si la version n'existe pas).
async fn layout_of(
    db: &DatabaseConnection,
    dashboard_id: Uuid,
    version_number: i64,
) -> Result<Option<DashboardLayout>> {
    Ok(dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard_id))
        .filter(dashboard_versions::Column::VersionNumber.eq(version_number))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .and_then(|v| serde_json::from_value(v.layout).ok()))
}

fn summary_dto(d: dashboards::Model, format: pnex_core::DashboardFormat) -> VizDashboardSummary {
    VizDashboardSummary {
        id: d.id.to_string(),
        org_id: d.org_id,
        name: d.name,
        description: d.description,
        current_version_number: d.current_version_number,
        format,
        created_at: d.created_at.to_rfc3339(),
        updated_at: d.updated_at.to_rfc3339(),
    }
}

/// Format of the current version of each listed dashboard (one query for
/// the page; an unreadable layout falls back to desktop).
async fn current_formats(
    db: &DatabaseConnection,
    rows: &[dashboards::Model],
) -> Result<std::collections::HashMap<Uuid, pnex_core::DashboardFormat>> {
    if rows.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let current: std::collections::HashMap<Uuid, i64> = rows
        .iter()
        .map(|d| (d.id, d.current_version_number))
        .collect();
    // Only the current version of each row (a dashboard keeps every saved
    // version: never load the whole history for a badge).
    let mut cond = sea_orm::Condition::any();
    for (id, n) in &current {
        cond = cond.add(
            sea_orm::Condition::all()
                .add(dashboard_versions::Column::DashboardId.eq(*id))
                .add(dashboard_versions::Column::VersionNumber.eq(*n)),
        );
    }
    let versions = dashboard_versions::Entity::find()
        .filter(cond)
        .all(db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(versions
        .into_iter()
        .filter(|v| current.get(&v.dashboard_id) == Some(&v.version_number))
        .map(|v| {
            let format = v
                .layout
                .get("format")
                .cloned()
                .and_then(|f| serde_json::from_value(f).ok())
                .unwrap_or_default();
            (v.dashboard_id, format)
        })
        .collect())
}

async fn detail_dto(db: &DatabaseConnection, d: dashboards::Model) -> Result<VizDashboard> {
    let layout = layout_of(db, d.id, d.current_version_number)
        .await?
        .unwrap_or_default();
    Ok(VizDashboard {
        id: d.id.to_string(),
        org_id: d.org_id,
        name: d.name,
        description: d.description,
        current_version_number: d.current_version_number,
        layout,
        created_at: d.created_at.to_rfc3339(),
        updated_at: d.updated_at.to_rfc3339(),
    })
}

/// Service write error to HTTP: `field-invalid` / `dashboard-invalid` 400,
/// 409 version conflict, 404, 500.
fn write_error_response(e: DashboardWriteError) -> Response {
    match e {
        DashboardWriteError::NameRequired => super::field_error("name", err_codes::FIELD_REQUIRED),
        DashboardWriteError::NameTooLong => {
            super::field_error("name", &format!("{}:255", err_codes::FIELD_MAX_LENGTH))
        }
        DashboardWriteError::Violations(v) => {
            super::violations_error(pnex_core::err_codes::DASHBOARD_INVALID, v)
        }
        // Optimistic concurrency: the expected/current numbers travel in
        // `errors.args` (never pre-rendered into the description).
        DashboardWriteError::Conflict { expected, current } => Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail {
                error: Some(err_codes::DASHBOARD_VERSION_CONFLICT.to_string()),
                description: Some(
                    "Stale version — reload the latest version before saving".to_string(),
                ),
                errors: Some(serde_json::json!({
                    "args": {
                        "expected": expected.to_string(),
                        "current": current.to_string(),
                    }
                })),
            },
        )
        .into_response(),
        DashboardWriteError::VersionUnknown => Error::NotFound.into_response(),
        DashboardWriteError::Db => Error::InternalServerError.into_response(),
    }
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/dashboards")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/versions/{version_number}", get(version_detail))
        .add("/{id}/versions/{version_number}/restore", post(restore))
}

// ─────────────────────────── GET /dashboards ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListDashboardsQuery {
    /// Case-insensitive search on the name.
    search: Option<String>,
    /// D42: effective label (`name` or `name:value`, inherited included).
    label: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/dashboards` — dashboards de l'org, paginés (D14),
/// filtre `search` (nom).
#[utoipa::path(
    get,
    path = "/api/v1/dashboards",
    tag = "dashboards",
    operation_id = "dashboards_list",
    summary = "List the dashboards of the organisation",
    description = "Newest first, paginated (`limit`/`offset`). Filters: `search` (name) and `label` (`name` or `name:value`, inherited labels included). Each row carries the format (desktop/mobile) of its current version. Any member.",
    params(ListDashboardsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::VizDashboardSummary>),
        (status = 400, description = "`field-invalid`: invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListDashboardsQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    // Search, COUNT and LIMIT/OFFSET in SQL (org-scoped).
    let mut query = dashboards::Entity::find()
        .filter(dashboards::Column::OrgId.eq(org.org.id))
        .order_by_desc(dashboards::Column::Id);
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (dashboards::Entity, dashboards::Column::Name),
            &pat,
        ));
    }
    // D42: effective label filter.
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_DASHBOARD,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            query = query.filter(dashboards::Column::Id.is_in(pagination::parse_ids::<Uuid>(&ids)));
        }
        Err(resp) => return Ok(resp),
    }
    let (count, rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let formats = current_formats(&ctx.db, &rows).await?;
    let results: Vec<VizDashboardSummary> = rows
        .into_iter()
        .map(|d| {
            let format = formats.get(&d.id).copied().unwrap_or_default();
            summary_dto(d, format)
        })
        .collect();

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(l) = q.label.as_deref().filter(|l| !l.is_empty()) {
        filters.push(("label".to_string(), l.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/dashboards",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── POST /dashboards ───────────────────────────

/// `POST /api/v1/dashboards` — crée le dashboard **et sa version 1**.
#[utoipa::path(
    post,
    path = "/api/v1/dashboards",
    tag = "dashboards",
    operation_id = "dashboards_create",
    summary = "Create a dashboard",
    description = "Creates the dashboard and its version 1 (empty layout when none is given). Controls declared by control widgets are provisioned. Requires a writer role.",
    request_body = pnex_core::CreateDashboard,
    responses(
        (status = 201, body = pnex_core::VizDashboard),
        (status = 400, description = "`field-invalid` (`errors.fields`: `name`) or `dashboard-invalid` (`errors.violations`: layout violations)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateDashboard>,
) -> Result<Response> {
    match crate::services::dashboards::create_dashboard(
        &ctx.db,
        org.org.id,
        &params.name,
        params.description,
        params.layout.as_ref(),
        Some(org.auth.user.email.clone()),
    )
    .await
    {
        Ok((dashboard, version)) => {
            let mut dto = detail_dto(&ctx.db, dashboard).await?;
            dto.current_version_number = version;
            Ok((StatusCode::CREATED, format::json(dto)).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

// ─────────────────────────── GET /dashboards/{id} ───────────────────────────

/// `GET /api/v1/dashboards/{id}` — détail + layout de la version
/// **courante** (celle que les viewers rendent).
#[utoipa::path(
    get,
    path = "/api/v1/dashboards/{id}",
    tag = "dashboards",
    operation_id = "dashboards_get",
    summary = "Get a dashboard with its current layout",
    description = "Returns the layout of the current version (the one viewers render). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)")),
    responses(
        (status = 200, body = pnex_core::VizDashboard),
        (status = 404, description = "Unknown dashboard (or another org's)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    Ok(format::json(detail_dto(&ctx.db, dashboard).await?).into_response())
}

// ─────────────────────────── PATCH /dashboards/{id} ───────────────────────────

/// `PATCH /api/v1/dashboards/{id}` = save — nouvelle version + pointeur
/// (save = live). 409 si `expected_version_number` ≠ version courante.
#[utoipa::path(
    patch,
    path = "/api/v1/dashboards/{id}",
    tag = "dashboards",
    operation_id = "dashboards_update",
    summary = "Save a dashboard (new version)",
    description = "Appends a new version with the given layout (and optional new name) and makes it current immediately: saving is publishing, viewers see it on their next poll. Optimistic concurrency: `expected_version_number` must be the current version, else 409. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)")),
    request_body = pnex_core::UpdateDashboard,
    responses(
        (status = 200, body = pnex_core::VizDashboard),
        (status = 400, description = "`field-invalid` (`errors.fields`: `name`) or `dashboard-invalid` (`errors.violations`: layout violations)", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown dashboard (or another org's)", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`dashboard-version-conflict` (args `expected`, `current`): another save happened, reload first", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<pnex_core::UpdateDashboard>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    match crate::services::dashboards::append_version(
        &ctx.db,
        &dashboard,
        params.expected_version_number,
        &params.layout,
        params.name,
        Some(org.auth.user.email.clone()),
    )
    .await
    {
        Ok(_) => {
            // Re-lecture : le pointeur a suivi la nouvelle version, le
            // détail renvoyé est donc déjà à jour.
            let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
                return Err(Error::NotFound);
            };
            Ok(format::json(detail_dto(&ctx.db, dashboard).await?).into_response())
        }
        Err(e) => Ok(write_error_response(e)),
    }
}

// ─────────────────────────── DELETE /dashboards/{id} ───────────────────────────

/// `DELETE /api/v1/dashboards/{id}` — 204 ; les versions suivent (FK
/// CASCADE).
#[utoipa::path(
    delete,
    path = "/api/v1/dashboards/{id}",
    tag = "dashboards",
    operation_id = "dashboards_delete",
    summary = "Delete a dashboard",
    description = "Permanently deletes the dashboard and all its versions, its labels and links, and releases the controls its widgets declared (deleted unless a flow or another surface still uses them). Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown dashboard (or another org's)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // D42 : purge symétrique de la couche d'organisation avant le delete.
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_DASHBOARD,
        &dashboard.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    // D131: the controls its widgets declared are released (deleted, or
    // kept standalone while a flow or another surface uses them).
    let released = crate::services::surface_controls::release_surface(
        &ctx.db,
        org.org.id,
        pnex_core::ui_control::ORIGIN_DASHBOARD,
        dashboard.id,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    crate::services::surface_controls::forget_deleted(&ctx, org.org.id, &released).await;
    dashboard
        .into_active_model()
        .delete(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── GET /dashboards/{id}/versions ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct VersionsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/dashboards/{id}/versions` — historique append-only
/// (drapeau `current` = version pointée).
#[utoipa::path(
    get,
    path = "/api/v1/dashboards/{id}/versions",
    tag = "dashboards",
    operation_id = "dashboards_versions",
    summary = "List the versions of a dashboard",
    description = "Version history without layouts, newest first, paginated (`limit`/`offset`); `current` flags the version viewers render. Any member.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)"), VersionsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::VizDashboardVersion>),
        (status = 404, description = "Unknown dashboard (or another org's)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionsQuery>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let base = dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard.id));
    let count = sea_orm::PaginatorTrait::count(base.clone(), &ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)? as i64;
    // One page of the history, without the (large) layout column.
    let rows: Vec<(i64, Option<String>, sea_orm::prelude::DateTimeWithTimeZone)> = base
        .select_only()
        .column(dashboard_versions::Column::VersionNumber)
        .column(dashboard_versions::Column::Author)
        .column(dashboard_versions::Column::CreatedAt)
        .order_by_desc(dashboard_versions::Column::VersionNumber)
        .offset(page.offset as u64)
        .limit(page.limit as u64)
        .into_tuple()
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let results: Vec<VizDashboardVersion> = rows
        .into_iter()
        .map(|(version_number, author, created_at)| VizDashboardVersion {
            version_number,
            current: version_number == dashboard.current_version_number,
            author,
            created_at: created_at.to_rfc3339(),
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/dashboards/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────── GET /dashboards/{id}/versions/{n} ───────────────────────

/// `GET /api/v1/dashboards/{id}/versions/{n}` — layout d'une version
/// (aperçu du drawer historique).
#[utoipa::path(
    get,
    path = "/api/v1/dashboards/{id}/versions/{version_number}",
    tag = "dashboards",
    operation_id = "dashboards_version_get",
    summary = "Get one version of a dashboard",
    description = "Layout of a past or current version (history preview). Any member.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)"), ("version_number" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::VizDashboardVersionDetail),
        (status = 404, description = "Unknown dashboard or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn version_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, version_number)): Path<(Uuid, i64)>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = dashboard_versions::Entity::find()
        .filter(dashboard_versions::Column::DashboardId.eq(dashboard.id))
        .filter(dashboard_versions::Column::VersionNumber.eq(version_number))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let Ok(layout) = serde_json::from_value::<DashboardLayout>(version.layout) else {
        return Err(Error::InternalServerError);
    };
    Ok(format::json(pnex_core::VizDashboardVersionDetail {
        version_number,
        current: version_number == dashboard.current_version_number,
        layout,
        author: version.author,
        created_at: version.created_at.to_rfc3339(),
    })
    .into_response())
}

// ─────────────────────── POST /dashboards/{id}/versions/{n}/restore ───────────────────────

/// `POST /api/v1/dashboards/{id}/versions/{n}/restore` — re-positionne
/// le pointeur courant (école media) : les viewers retombent sur cette
/// version au prochain polling, aucune version n'est créée.
#[utoipa::path(
    post,
    path = "/api/v1/dashboards/{id}/versions/{version_number}/restore",
    tag = "dashboards",
    operation_id = "dashboards_version_restore",
    summary = "Make an older version of a dashboard current",
    description = "Moves the current-version pointer to that version (no new version is created); viewers switch on their next poll. Requires a writer role.",
    params(("id" = String, Path, format = "uuid", description = "Dashboard id (UUID)"), ("version_number" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::VizDashboard),
        (status = 404, description = "Unknown dashboard or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn restore(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, version_number)): Path<(Uuid, i64)>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    match crate::services::dashboards::restore_version(&ctx.db, &dashboard, version_number).await {
        Ok(dashboard) => Ok(format::json(detail_dto(&ctx.db, dashboard).await?).into_response()),
        Err(e) => Ok(write_error_response(e)),
    }
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(
        list,
        create,
        detail,
        update,
        delete,
        versions,
        version_detail,
        restore
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
