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
fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// 400 per-field error shape: `{"<field>": "..."}`.
fn field_status(field: &str, msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: msg })),
    )
        .into_response()
}

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

/// Mapping service → HTTP (formes historiques : les tests d'intégration
/// en sont la référence).
fn write_error_response(e: DashboardWriteError) -> Response {
    match e {
        DashboardWriteError::NameRequired => field_status("name", err_codes::FIELD_REQUIRED),
        DashboardWriteError::NameTooLong => {
            field_status("name", &format!("{}:255", err_codes::FIELD_MAX_LENGTH))
        }
        DashboardWriteError::Violations(v) => (
            StatusCode::BAD_REQUEST,
            format::json(serde_json::json!({ "violations": v })),
        )
            .into_response(),
        // Optimistic concurrency: the expected/current numbers travel in
        // `errors.args` (never pre-rendered into the description).
        DashboardWriteError::Conflict { expected, current } => Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail {
                error: Some("dashboard-version-conflict".to_string()),
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

#[derive(Debug, Default, Deserialize)]
struct ListDashboardsQuery {
    search: Option<String>,
    /// D42: effective label (`name` or `name:value`, inherited included).
    label: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `GET /api/v1/dashboards` — dashboards de l'org, paginés (D14),
/// filtre `search` (nom).
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
    match crate::services::resources::labels::list_filter(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_DASHBOARD,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            let ids: Vec<Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
            query = query.filter(dashboards::Column::Id.is_in(ids));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Invalid(reason)) => {
            return Ok(field_status("label", &reason));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Db(_)) => {
            return Err(Error::InternalServerError);
        }
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
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateDashboard>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "dashboard-write-forbidden",
            "Owner, admin or member role required to manage dashboards",
        ));
    }
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
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<pnex_core::UpdateDashboard>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "dashboard-write-forbidden",
            "Owner, admin or member role required to manage dashboards",
        ));
    }
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
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "dashboard-write-forbidden",
            "Owner, admin or member role required to manage dashboards",
        ));
    }
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

/// Max entries of the dashboard version history list.
const VERSIONS_LIST_CAP: u64 = 500;

/// `GET /api/v1/dashboards/{id}/versions` — historique append-only
/// (drapeau `current` = version pointée).
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // History list without the (large) layout column, newest first, bounded
    // (the drawer shows the recent history; older versions stay reachable
    // through GET /versions/{n}).
    let rows: Vec<(i64, Option<String>, sea_orm::prelude::DateTimeWithTimeZone)> =
        dashboard_versions::Entity::find()
            .filter(dashboard_versions::Column::DashboardId.eq(dashboard.id))
            .select_only()
            .column(dashboard_versions::Column::VersionNumber)
            .column(dashboard_versions::Column::Author)
            .column(dashboard_versions::Column::CreatedAt)
            .order_by_desc(dashboard_versions::Column::VersionNumber)
            .limit(VERSIONS_LIST_CAP)
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
    Ok(format::json(results).into_response())
}

// ─────────────────────── GET /dashboards/{id}/versions/{n} ───────────────────────

/// `GET /api/v1/dashboards/{id}/versions/{n}` — layout d'une version
/// (aperçu du drawer historique).
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
async fn restore(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, version_number)): Path<(Uuid, i64)>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "dashboard-write-forbidden",
            "Owner, admin or member role required to manage dashboards",
        ));
    }
    let Some(dashboard) = find_dashboard(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    match crate::services::dashboards::restore_version(&ctx.db, &dashboard, version_number).await {
        Ok(dashboard) => Ok(format::json(detail_dto(&ctx.db, dashboard).await?).into_response()),
        Err(e) => Ok(write_error_response(e)),
    }
}
