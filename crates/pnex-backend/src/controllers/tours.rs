//! Studio (mode panorama V1) — CRUD versionné des parcours de visite 3D,
//! scoping org (D2). École `controllers/flows.rs` : save append-only avec
//! concurrence optimiste (409), publication = pointeur `published_version_id`
//! (pas de superviseur ici : publier un tour est un simple marquage DB).
//!
//! Le **share** génère un token (32 hex) qui expose la version publiée sur
//! l'endpoint public sans auth (`controllers/public_tours.rs`, phase 2) —
//! le token ne quitte l'API que pour les writers de l'org.
//!
//! Errors: `ErrorDetail` everywhere (`field-invalid`, `tour-invalid` with
//! `errors.violations`, coded 403/409); 404 hides other orgs.

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
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::models::_entities::{tour_versions, tours};
use pnex_core::err_codes;

// ─────────────────────────── Aides ───────────────────────────

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.
fn conflict(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::CONFLICT,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

fn forbidden(code: &str, msg: &str) -> Error {
    Error::CustomError(
        StatusCode::FORBIDDEN,
        loco_rs::controller::ErrorDetail::new(code, msg.to_string()),
    )
}

/// `field-invalid` 400 on one field.
fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// Tour de l'org courante, sinon None (→ 404 masqué).
async fn find_tour(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<tours::Model>> {
    tours::Entity::find_by_id(id)
        .filter(tours::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Dernière version d'un tour (numéro le plus élevé).
async fn latest_version(
    db: &DatabaseConnection,
    tour_id: Uuid,
) -> Result<Option<tour_versions::Model>> {
    tour_versions::Entity::find()
        .filter(tour_versions::Column::TourId.eq(tour_id))
        .order_by_desc(tour_versions::Column::VersionNumber)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Numéro de la version publiée (résolu depuis la FK circulaire).
async fn published_number_of(
    db: &DatabaseConnection,
    published_version_id: Option<Uuid>,
) -> Result<Option<i64>> {
    let Some(id) = published_version_id else {
        return Ok(None);
    };
    Ok(tour_versions::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number))
}

fn summary_dto(
    t: tours::Model,
    latest: i64,
    published_number: Option<i64>,
    can_write: bool,
) -> pnex_core::TourSummary {
    pnex_core::TourSummary {
        id: t.id.to_string(),
        org_id: t.org_id,
        name: t.name,
        description: t.description,
        mode: t.mode,
        latest_version_number: latest,
        published_version_number: published_number,
        share_enabled: t.share_token.is_some(),
        // Secret de publication : writers uniquement.
        share_token: can_write.then_some(t.share_token).flatten(),
        created_at: t.created_at.to_rfc3339(),
        updated_at: t.updated_at.to_rfc3339(),
    }
}

#[allow(clippy::too_many_arguments)]
fn detail_dto(
    t: tours::Model,
    doc: pnex_core::TourDoc,
    doc_version_number: i64,
    latest: i64,
    published_number: Option<i64>,
    can_write: bool,
) -> pnex_core::TourDetail {
    pnex_core::TourDetail {
        id: t.id.to_string(),
        org_id: t.org_id,
        name: t.name,
        description: t.description,
        mode: t.mode,
        doc,
        doc_version_number,
        latest_version_number: latest,
        published_version_number: published_number,
        share_enabled: t.share_token.is_some(),
        share_token: can_write.then_some(t.share_token).flatten(),
        created_at: t.created_at.to_rfc3339(),
        updated_at: t.updated_at.to_rfc3339(),
    }
}

fn tour_write_error_response(e: crate::services::tour::TourWriteError) -> Response {
    use crate::services::tour::TourWriteError as E;
    match e {
        E::NameRequired => field_status("name", err_codes::FIELD_REQUIRED),
        E::NameTooLong => field_status("name", &format!("{}:200", err_codes::FIELD_MAX_LENGTH)),
        E::Doc(v) => super::violations_error(pnex_core::err_codes::TOUR_INVALID, v),
        // Database-checked asset references travel as violations (the
        // offending asset id is the `subject`), like the structural ones.
        E::UnknownAsset { id } => super::violations_error(
            err_codes::TOUR_INVALID,
            vec![pnex_core::TourViolation {
                subject: Some(id.clone()),
                code: "unknown_asset".to_string(),
                message: format!("asset {id} is unknown in this organization"),
            }],
        ),
        E::BadAssetKind { id, expected } => super::violations_error(
            err_codes::TOUR_INVALID,
            vec![pnex_core::TourViolation {
                subject: Some(id.clone()),
                code: "asset_kind_mismatch".to_string(),
                message: format!("asset {id}: kind {} required", expected.join(" or ")),
            }],
        ),
        // Optimistic concurrency: the expected/current numbers travel in
        // `errors.args` (never pre-rendered into the description).
        E::Conflict { expected, current } => Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail {
                error: Some("tour-version-conflict".to_string()),
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
        E::Db => Error::InternalServerError.into_response(),
    }
}

// ─────────────────────────── POST /tours ───────────────────────────

/// `POST /api/v1/tours` — crée le tour **et sa version 1** (une
/// transaction), document minimal (un étage « RDC »). Non publié.
#[utoipa::path(
    post,
    path = "/api/v1/tours",
    tag = "tours",
    operation_id = "tours_create",
    summary = "Create a tour",
    description = "Creates the tour and its version 1 with a minimal document (one floor). Not published. Requires owner, admin or member role.",
    request_body = pnex_core::CreateTour,
    responses(
        (status = 201, body = pnex_core::TourDetail),
        (status = 400, description = "`field-invalid` (`name`) or `tour-invalid` with `errors.violations` = `[{subject, code, message}]`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`tour-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateTour>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-write-forbidden",
            "Owner, admin or member role required to manage tours",
        ));
    }
    let (tour, version) = match crate::services::tour::create_tour(
        &ctx.db,
        org.org.id,
        &params.name,
        params.description,
        params.author,
        params.note,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return Ok(tour_write_error_response(e)),
    };
    Ok((
        StatusCode::CREATED,
        format::json(detail_dto(
            tour,
            pnex_core::TourDoc::minimal(),
            version,
            version,
            None,
            org.can_write(),
        )),
    )
        .into_response())
}

// ─────────────────────────── GET /tours ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListToursQuery {
    /// Case-insensitive substring of the name.
    search: Option<String>,
    /// D42: effective label (`name` or `name:value`, inherited included).
    label: Option<String>,
    /// Exact tour mode (e.g. `panorama`).
    mode: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /api/v1/tours` — tours de l'org, paginés (D14), filtres `search`
/// (nom) et `mode`.
#[utoipa::path(
    get,
    path = "/api/v1/tours",
    tag = "tours",
    operation_id = "tours_list",
    summary = "List the tours of the organisation",
    description = "Studio 3D tours, paginated (`limit`/`offset`), newest first, with latest and published version numbers. Filters: `search`, `mode`, `label`. `share_token` is only returned to writers.",
    params(ListToursQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::TourSummary>),
        (status = 400, description = "Invalid `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListToursQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let mut query = tours::Entity::find()
        .filter(tours::Column::OrgId.eq(org.org.id))
        .order_by_desc(tours::Column::Id);
    if let Some(mode) = q.mode.as_deref().filter(|m| !m.is_empty()) {
        query = query.filter(tours::Column::Mode.eq(mode));
    }
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        query = query.filter(pagination::sql_contains(
            (tours::Entity, tours::Column::Name),
            &pat,
        ));
    }
    // D42: effective label filter.
    match crate::services::resources::labels::list_filter(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_TOUR,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            let ids: Vec<Uuid> = ids.iter().filter_map(|id| id.parse().ok()).collect();
            query = query.filter(tours::Column::Id.is_in(ids));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Invalid(reason)) => {
            return Ok(field_status("label", &reason));
        }
        Err(crate::services::resources::labels::ListLabelFilterError::Db(_)) => {
            return Err(Error::InternalServerError);
        }
    }
    let (count, page_rows) = pagination::sql_page(&ctx.db, query, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydratation en vrac (pas de N+1) : latest + numéro publié par tour.
    let ids: Vec<Uuid> = page_rows.iter().map(|t| t.id).collect();
    let latest: std::collections::HashMap<Uuid, i64> = if ids.is_empty() {
        Default::default()
    } else {
        tour_versions::Entity::find()
            .select_only()
            .column(tour_versions::Column::TourId)
            .column_as(tour_versions::Column::VersionNumber.max(), "latest")
            .filter(tour_versions::Column::TourId.is_in(ids.clone()))
            .group_by(tour_versions::Column::TourId)
            .into_tuple::<(Uuid, i64)>()
            .all(&ctx.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let published_ids: Vec<Uuid> = page_rows
        .iter()
        .filter_map(|t| t.published_version_id)
        .collect();
    let published_numbers: std::collections::HashMap<Uuid, i64> = if published_ids.is_empty() {
        Default::default()
    } else {
        tour_versions::Entity::find()
            .filter(tour_versions::Column::Id.is_in(published_ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|v| (v.id, v.version_number))
            .collect()
    };
    let results: Vec<pnex_core::TourSummary> = page_rows
        .into_iter()
        .map(|t| {
            let latest_n = latest.get(&t.id).copied().unwrap_or(0);
            let published_n = t
                .published_version_id
                .and_then(|id| published_numbers.get(&id).copied());
            summary_dto(t, latest_n, published_n, org.can_write())
        })
        .collect();

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    if let Some(l) = q.label.as_deref().filter(|l| !l.is_empty()) {
        filters.push(("label".to_string(), l.to_string()));
    }
    if let Some(s) = q.mode.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("mode".to_string(), s.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/tours",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /tours/{id} ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DetailQuery {
    /// Version number of the returned document (default: latest).
    version: Option<i64>,
}

/// `GET /api/v1/tours/{id}[?version=n]` — détail + document (dernière
/// version par défaut, ou la version demandée).
#[utoipa::path(
    get,
    path = "/api/v1/tours/{id}",
    tag = "tours",
    operation_id = "tours_get",
    summary = "Get a tour",
    description = "The tour with the document of its latest version, or of `?version=n`. `share_token` is only returned to writers.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)"), DetailQuery),
    responses(
        (status = 200, body = pnex_core::TourDetail),
        (status = 404, description = "Unknown tour/version or tour of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DetailQuery>,
) -> Result<Response> {
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let (version_row, doc_number) = match q.version {
        Some(n) => {
            let row = tour_versions::Entity::find()
                .filter(tour_versions::Column::TourId.eq(tour.id))
                .filter(tour_versions::Column::VersionNumber.eq(n))
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            match row {
                Some(v) => (Some(v), n),
                None => return Err(Error::NotFound),
            }
        }
        None => {
            let latest = latest_version(&ctx.db, tour.id).await?;
            let n = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
            (latest, n)
        }
    };
    let doc: pnex_core::TourDoc = version_row
        .map(|v| serde_json::from_value(v.doc).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    let latest_number = latest_version(&ctx.db, tour.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    let published_number = published_number_of(&ctx.db, tour.published_version_id).await?;
    Ok(format::json(detail_dto(
        tour,
        doc,
        doc_number,
        latest_number,
        published_number,
        org.can_write(),
    ))
    .into_response())
}

// ─────────────────────────── PATCH /tours/{id} ───────────────────────────

/// `PATCH /api/v1/tours/{id}` — enregistre une **nouvelle version**
/// (append-only). 409 si `expected_version_number` ≠ version courante.
#[utoipa::path(
    patch,
    path = "/api/v1/tours/{id}",
    tag = "tours",
    operation_id = "tours_update",
    summary = "Save a new version of a tour",
    description = "Appends a new version with the given document (append-only history), only if `expected_version_number` is the current latest version (optimistic concurrency). Referenced media must belong to the org. Does not change what is published. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    request_body = pnex_core::UpdateTour,
    responses(
        (status = 200, body = pnex_core::TourDetail),
        (status = 400, description = "`field-invalid` (`name`) or `tour-invalid` with `errors.violations` = `[{subject, code, message}]`", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`tour-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`tour-version-conflict` (`errors.args.expected` / `errors.args.current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<pnex_core::UpdateTour>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-write-forbidden",
            "Owner, admin or member role required to manage tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let (tour, version) = match crate::services::tour::append_tour_version(
        &ctx.db,
        &tour,
        params.expected_version_number,
        &params.doc,
        params.name,
        params.author,
        params.note,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return Ok(tour_write_error_response(e)),
    };
    let published_number = published_number_of(&ctx.db, tour.published_version_id).await?;
    Ok(format::json(detail_dto(
        tour,
        params.doc,
        version,
        version,
        published_number,
        org.can_write(),
    ))
    .into_response())
}

// ─────────────────────────── DELETE /tours/{id} ───────────────────────────

/// `DELETE /api/v1/tours/{id}` — 204 ; versions supprimées en cascade. Aucun
/// octet à purger : les assets média sont référencés, jamais possédés (D21).
#[utoipa::path(
    delete,
    path = "/api/v1/tours/{id}",
    tag = "tours",
    operation_id = "tours_delete",
    summary = "Delete a tour",
    description = "Deletes the tour, its versions, its labels/edges and its public link. Referenced media are kept. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`tour-write-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-write-forbidden",
            "Owner, admin or member role required to manage tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // D42 : purge symétrique de la couche d'organisation avant le delete.
    crate::services::resources::purge_for(
        &ctx.db,
        org.org.id,
        pnex_core::resources::KIND_TOUR,
        &tour.id.to_string(),
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    tours::Entity::delete_by_id(tour.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(tour_id = tour.id.to_string().as_str(), "tour supprimé");
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── GET /tours/{id}/versions ───────────────────────────

/// `GET /api/v1/tours/{id}/versions` — historique append-only, paginé (D14),
/// du plus récent au plus ancien.
#[utoipa::path(
    get,
    path = "/api/v1/tours/{id}/versions",
    tag = "tours",
    operation_id = "tours_versions_list",
    summary = "List the versions of a tour",
    description = "Append-only history, newest first, paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)"), VersionsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::TourVersionSummary>),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionsQuery>,
) -> Result<Response> {
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = tour_versions::Entity::find()
        .filter(tour_versions::Column::TourId.eq(tour.id))
        .order_by_desc(tour_versions::Column::VersionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    let results: Vec<pnex_core::TourVersionSummary> = rows
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|v| {
            let published = tour.published_version_id == Some(v.id);
            pnex_core::TourVersionSummary {
                id: v.id.to_string(),
                version_number: v.version_number,
                author: v.author,
                note: v.note,
                published,
                created_at: v.created_at.to_rfc3339(),
            }
        })
        .collect();
    Ok(format::json(pagination::envelope(
        &format!("/api/v1/tours/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct VersionsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

// ─────────────────────────── GET /tours/{id}/versions/{n} ───────────────────────────

/// `GET /api/v1/tours/{id}/versions/{n}` — document d'une version précise
/// (audit / rechargement éditeur — « restaurer » = la charger puis save).
#[utoipa::path(
    get,
    path = "/api/v1/tours/{id}/versions/{version_number}",
    tag = "tours",
    operation_id = "tours_versions_get",
    summary = "Get one version of a tour",
    description = "The document of a given version (audit, or load it in the editor then save to restore it).",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)"), ("version_number" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::TourVersionDetail),
        (status = 404, description = "Unknown tour or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn version_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(Uuid, i64)>,
) -> Result<Response> {
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = tour_versions::Entity::find()
        .filter(tour_versions::Column::TourId.eq(tour.id))
        .filter(tour_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let doc: pnex_core::TourDoc =
        serde_json::from_value(version.doc.clone()).map_err(|_| Error::InternalServerError)?;
    Ok(format::json(pnex_core::TourVersionDetail {
        id: version.id.to_string(),
        version_number: version.version_number,
        author: version.author,
        note: version.note,
        published: tour.published_version_id == Some(version.id),
        created_at: version.created_at.to_rfc3339(),
        doc,
    })
    .into_response())
}

// ─────────────────────────── POST /tours/{id}/publish | /unpublish ───────────────────────────

/// `POST /tours/{id}/publish` — publie une version (`version_number` absent =
/// dernière) : simple marquage DB (le doc publié sert le viewer in-app et
/// l'endpoint public).
#[utoipa::path(
    post,
    path = "/api/v1/tours/{id}/publish",
    tag = "tours",
    operation_id = "tours_publish",
    summary = "Publish a version of a tour",
    description = "Marks `version_number` (default: latest) as the published version, served to the in-app viewer and, if a share link exists, to the public endpoint. Body optional. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    request_body = Option<pnex_core::PublishTour>,
    responses(
        (status = 200, body = pnex_core::TourDetail),
        (status = 403, description = "`tour-publish-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn publish(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    body: Option<Json<pnex_core::PublishTour>>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-publish-forbidden",
            "Owner, admin or member role required to publish tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let version_number = match body.and_then(|Json(p)| p.version_number) {
        Some(n) => n,
        None => {
            let Some(latest) = latest_version(&ctx.db, tour.id).await? else {
                return Err(Error::NotFound);
            };
            latest.version_number
        }
    };
    let Some(version) = tour_versions::Entity::find()
        .filter(tour_versions::Column::TourId.eq(tour.id))
        .filter(tour_versions::Column::VersionNumber.eq(version_number))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let mut active: tours::ActiveModel = tour.clone().into();
    active.published_version_id = Set(Some(version.id));
    let tour = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(
        tour_id = tour.id.to_string().as_str(),
        version = version_number,
        "tour publié"
    );
    let doc: pnex_core::TourDoc =
        serde_json::from_value(version.doc).map_err(|_| Error::InternalServerError)?;
    let latest_number = latest_version(&ctx.db, tour.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    Ok(format::json(detail_dto(
        tour,
        doc,
        version_number,
        latest_number,
        Some(version_number),
        org.can_write(),
    ))
    .into_response())
}

/// `POST /tours/{id}/unpublish` — dépublie **et** révoque le lien public
/// (un seul interrupteur « public »).
#[utoipa::path(
    post,
    path = "/api/v1/tours/{id}/unpublish",
    tag = "tours",
    operation_id = "tours_unpublish",
    summary = "Unpublish a tour",
    description = "Clears the published version AND revokes the public share link. Versions are kept. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    responses(
        (status = 200, body = pnex_core::TourDetail),
        (status = 403, description = "`tour-publish-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn unpublish(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-publish-forbidden",
            "Owner, admin or member role required to publish tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let mut active: tours::ActiveModel = tour.clone().into();
    active.published_version_id = Set(None);
    active.share_token = Set(None);
    let tour = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(tour_id = tour.id.to_string().as_str(), "tour dépublié");
    let latest = latest_version(&ctx.db, tour.id).await?;
    let latest_number = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
    let doc: pnex_core::TourDoc = latest
        .map(|v| serde_json::from_value(v.doc).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    Ok(format::json(detail_dto(
        tour,
        doc,
        latest_number,
        latest_number,
        None,
        org.can_write(),
    ))
    .into_response())
}

// ─────────────────────────── POST/DELETE /tours/{id}/share ───────────────────────────

/// `POST /tours/{id}/share` — génère (ou régénère : l'ancien token meurt) le
/// token du lien public. 409 `not_published` si aucune version publiée.
#[utoipa::path(
    post,
    path = "/api/v1/tours/{id}/share",
    tag = "tours",
    operation_id = "tours_share_create",
    summary = "Create the public link of a tour",
    description = "Generates a new public share token (the previous one stops working). Anyone with the token can read the published version without authentication. Requires a published version and owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    responses(
        (status = 200, body = TourShare),
        (status = 403, description = "`tour-share-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`tour-not-published`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create_share(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-share-forbidden",
            "Owner, admin or member role required to share tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    if tour.published_version_id.is_none() {
        return Err(conflict(
            "tour-not-published",
            "Publish a version before creating a public share link",
        ));
    }
    let token = Uuid::new_v4().simple().to_string(); // 32 hex, 122 bits
    let mut active: tours::ActiveModel = tour.clone().into();
    active.share_token = Set(Some(token.clone()));
    active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(tour_id = tour.id.to_string().as_str(), "lien public généré");
    Ok(format::json(TourShare {
        share_token: token,
        share_enabled: true,
    })
    .into_response())
}

/// Public share link of a tour (secret: writers only).
#[derive(serde::Serialize, utoipa::ToSchema)]
struct TourShare {
    /// 32-hex token of `GET /api/v1/public/tours/{token}`.
    share_token: String,
    share_enabled: bool,
}

/// `DELETE /tours/{id}/share` — révoque le lien public en gardant la
/// publication (le viewer in-app reste servi par `published_version_id`).
#[utoipa::path(
    delete,
    path = "/api/v1/tours/{id}/share",
    tag = "tours",
    operation_id = "tours_share_delete",
    summary = "Revoke the public link of a tour",
    description = "Removes the share token; the publication stays visible in the app. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Tour id (UUID)")),
    responses(
        (status = 204, description = "Revoked"),
        (status = 403, description = "`tour-share-forbidden`: viewer role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown tour or tour of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete_share(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    if !org.can_write() {
        return Err(forbidden(
            "tour-share-forbidden",
            "Owner, admin or member role required to share tours",
        ));
    }
    let Some(tour) = find_tour(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let mut active: tours::ActiveModel = tour.into();
    active.share_token = Set(None);
    active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/tours")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/versions/{version_number}", get(version_detail))
        .add("/{id}/publish", post(publish))
        .add("/{id}/unpublish", post(unpublish))
        .add("/{id}/share", post(create_share).delete(delete_share))
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
        publish,
        unpublish,
        create_share,
        delete_share
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
