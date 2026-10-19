//! Annotations sur médias (D55–D60) — CRUD versionné des couches
//! (`/api/v1/annotation-layers`, école `controllers/tours.rs`) + read model
//! viewers `GET /api/v1/media/{asset_id}/annotations` : items fusionnés des
//! couches publiées ancrées sur l'asset (D55), résolution device en batch
//! (D57, école `link_target_labels`), 404 si asset inconnu. Share public
//! exclu V1 (D58) : le read model est auth-scoped.
//!
//! Deux `Routes` (même prefix que `media.rs` : chemin `/{id}/annotations`
//! disjoint — aucune collision axum).

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
use crate::models::_entities::{
    annotation_layer_versions, annotation_layers, device_registries, media_assets,
};
use pnex_core::err_codes;

// Error helpers carry a machine code (`pnex_core::err_codes` registry) so
// the frontend can resolve `err-<kebab>` at render time; the English text is
// the verbatim fallback for unregistered codes.

/// 400 per-field error shape.
fn field_status(field: &str, msg: &str) -> Response {
    super::field_error(field, msg)
}

/// Couche de l'org courante, sinon None (404 masqué cross-org).
async fn find_layer(
    db: &DatabaseConnection,
    org: &OrgContext,
    id: Uuid,
) -> Result<Option<annotation_layers::Model>> {
    annotation_layers::Entity::find_by_id(id)
        .filter(annotation_layers::Column::OrgId.eq(org.org.id))
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)
}

/// Dernière version d'une couche (numéro le plus élevé).
async fn latest_version(
    db: &DatabaseConnection,
    layer_id: Uuid,
) -> Result<Option<annotation_layer_versions::Model>> {
    annotation_layer_versions::Entity::find()
        .filter(annotation_layer_versions::Column::LayerId.eq(layer_id))
        .order_by_desc(annotation_layer_versions::Column::VersionNumber)
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
    Ok(annotation_layer_versions::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|_| Error::InternalServerError)?
        .map(|v| v.version_number))
}

fn summary_dto(
    l: annotation_layers::Model,
    latest: i64,
    published_number: Option<i64>,
) -> pnex_core::AnnotationLayerSummary {
    pnex_core::AnnotationLayerSummary {
        id: l.id.to_string(),
        org_id: l.org_id,
        name: l.name,
        description: l.description,
        media_asset_id: l.media_asset_id.map(|m| m.to_string()),
        tour_id: l.tour_id.map(|t| t.to_string()),
        latest_version_number: latest,
        published_version_number: published_number,
        created_at: l.created_at.to_rfc3339(),
        updated_at: l.updated_at.to_rfc3339(),
    }
}

fn detail_dto(
    l: annotation_layers::Model,
    doc: pnex_core::AnnotationDoc,
    doc_version_number: i64,
    latest: i64,
    published_number: Option<i64>,
) -> pnex_core::AnnotationLayerDetail {
    pnex_core::AnnotationLayerDetail {
        id: l.id.to_string(),
        org_id: l.org_id,
        name: l.name,
        description: l.description,
        media_asset_id: l.media_asset_id.map(|m| m.to_string()),
        tour_id: l.tour_id.map(|t| t.to_string()),
        doc,
        doc_version_number,
        latest_version_number: latest,
        published_version_number: published_number,
        created_at: l.created_at.to_rfc3339(),
        updated_at: l.updated_at.to_rfc3339(),
    }
}

fn layer_write_error_response(
    e: crate::services::annotation_layer::AnnotationLayerWriteError,
) -> Response {
    use crate::services::annotation_layer::AnnotationLayerWriteError as E;
    match e {
        E::NameRequired => field_status("name", err_codes::FIELD_REQUIRED),
        E::NameTooLong => field_status(
            "name",
            &format!("{}:200", err_codes::FIELD_MAX_LENGTH),
        ),
        E::Doc(v) => super::violations_error(pnex_core::err_codes::ANNOTATION_LAYER_INVALID, v),
        // Database-checked references of the document travel as violations
        // (the offending id is the `subject`), like the structural ones.
        E::UnknownAsset { id } => doc_violation(&id, "unknown_asset", format!("asset {id} is unknown in this organization")),
        E::BadAssetKind { id, expected } => doc_violation(
            &id,
            "asset_kind_mismatch",
            format!("asset {id}: kind {} required", expected.join(" or ")),
        ),
        E::UnknownDevice { device_id } => doc_violation(
            &device_id,
            "unknown_device",
            format!("device \"{device_id}\" is unknown in this organization"),
        ),
        E::UnknownControl { id } => doc_violation(
            &id.to_string(),
            "unknown_control",
            format!("control {id} is unknown in this organization"),
        ),
        E::AnchorMismatch { id } => doc_violation(
            &id,
            "anchor_mismatch",
            format!("item anchored on asset {id}: a layer only anchors on its declared media (or the scenes of its tour)"),
        ),
        E::MediaAndTour => field_status("tour_id", err_codes::FIELD_INVALID),
        E::UnknownTour { .. } => field_status("tour_id", err_codes::FIELD_NOT_FOUND),
        // Optimistic concurrency: the expected/current numbers travel in
        // `errors.args` (never pre-rendered into the description).
        E::Conflict { expected, current } => Error::CustomError(
            StatusCode::CONFLICT,
            loco_rs::controller::ErrorDetail {
                error: Some("annot-version-conflict".to_string()),
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

/// One-violation `annotation-layer-invalid` answer.
fn doc_violation(subject: &str, code: &str, message: String) -> Response {
    super::violations_error(
        err_codes::ANNOTATION_LAYER_INVALID,
        vec![pnex_core::AnnotationViolation {
            subject: Some(subject.to_string()),
            code: code.to_string(),
            message,
        }],
    )
}

/// Extracteur du slug device d'une cible (None pour note).
fn device_slug(target: &pnex_core::AnnotationTarget) -> Option<&String> {
    match target {
        pnex_core::AnnotationTarget::Device { device_id } => Some(device_id),
        pnex_core::AnnotationTarget::Pin { device_id, .. } => Some(device_id),
        pnex_core::AnnotationTarget::Status { device_id } => Some(device_id),
        pnex_core::AnnotationTarget::Note { .. }
        | pnex_core::AnnotationTarget::Control { .. }
        | pnex_core::AnnotationTarget::Reading { .. } => None,
    }
}

// ─────────────────────────── POST /annotation-layers ───────────────────────────

/// `POST /api/v1/annotation-layers` — crée la couche et sa version 1
/// (une transaction, document vide). Non publiée.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-layers",
    tag = "annotation-layers",
    operation_id = "annotation_layers_create",
    summary = "Create an annotation layer",
    description = "Creates the layer and its version 1 with an empty document, attached to a media OR a tour (not both). Not published. Requires owner, admin or member role.",
    request_body = pnex_core::CreateAnnotationLayer,
    responses(
        (status = 201, body = pnex_core::AnnotationLayerDetail),
        (status = 400, description = "`field-invalid` (`name`, `media_asset_id`, `tour_id`) or `annotation-layer-invalid` with `errors.violations` = `[{subject, code, message}]`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(params): Json<pnex_core::CreateAnnotationLayer>,
) -> Result<Response> {
    let media_asset_id = match params
        .media_asset_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => {
                return Ok(field_status(
                    "media_asset_id",
                    err_codes::FIELD_INVALID_UUID,
                ))
            }
        },
    };
    let tour_id = match params
        .tour_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => None,
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(field_status("tour_id", err_codes::FIELD_INVALID_UUID)),
        },
    };
    let (layer, version) = match crate::services::annotation_layer::create_annotation_layer(
        &ctx.db,
        org.org.id,
        &params.name,
        media_asset_id,
        tour_id,
        params.description,
        params.author,
        params.note,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return Ok(layer_write_error_response(e)),
    };
    Ok((
        StatusCode::CREATED,
        format::json(detail_dto(
            layer,
            pnex_core::AnnotationDoc::default(),
            version,
            version,
            None,
        )),
    )
        .into_response())
}

// ─────────────────────────── GET /annotation-layers ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListLayersQuery {
    /// Case-insensitive substring of the name.
    search: Option<String>,
    /// Exact filter on the attached media (UUID).
    media: Option<String>,
    /// Exact filter on the attached tour ("the sets of this tour").
    tour: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
    /// Label filter `name` / `name:value` (D218).
    label: Option<String>,
}

/// `GET /api/v1/annotation-layers` — couches de l'org, paginées (D14),
/// filtre search (nom) ; hydratation latest + publié en vrac (pas de N+1).
#[utoipa::path(
    get,
    path = "/api/v1/annotation-layers",
    tag = "annotation-layers",
    operation_id = "annotation_layers_list",
    summary = "List the annotation layers",
    description = "Paginated (`limit`/`offset`), newest first, with latest and published version numbers. Filters: `search`, `media`, `tour`, `label`.",
    params(ListLayersQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::AnnotationLayerSummary>),
        (status = 400, description = "Invalid `media`/`tour` UUID or `label` filter", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListLayersQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let media_filter = match q.media.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(field_status("media", err_codes::FIELD_INVALID_UUID)),
        },
    };
    let tour_filter = match q.tour.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => match uuid::Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return Ok(field_status("tour", err_codes::FIELD_INVALID_UUID)),
        },
    };
    let mut select =
        annotation_layers::Entity::find().filter(annotation_layers::Column::OrgId.eq(org.org.id));
    if let Some(media) = media_filter {
        select = select.filter(annotation_layers::Column::MediaAssetId.eq(media));
    }
    if let Some(tour) = tour_filter {
        select = select.filter(annotation_layers::Column::TourId.eq(tour));
    }
    match pagination::label_ids(
        &ctx.db,
        org.org.id,
        q.label.as_deref(),
        pnex_core::resources::KIND_ANNOTATION_LAYER,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(ids)) => {
            select = select.filter(
                annotation_layers::Column::Id.is_in(pagination::parse_ids::<uuid::Uuid>(&ids)),
            )
        }
        Err(resp) => return Ok(resp),
    }
    if let Some(pat) = pagination::sql_search_pattern(q.search.as_deref()) {
        select = select.filter(pagination::sql_contains(
            (annotation_layers::Entity, annotation_layers::Column::Name),
            &pat,
        ));
    }
    let select = select.order_by_desc(annotation_layers::Column::Id);
    let (count, page_rows) = pagination::sql_page(&ctx.db, select, page)
        .await
        .map_err(|_| Error::InternalServerError)?;

    // Hydratation en vrac (pas de N+1) : latest + numéro publié par couche.
    let ids: Vec<Uuid> = page_rows.iter().map(|l| l.id).collect();
    let latest: std::collections::HashMap<Uuid, i64> = if ids.is_empty() {
        Default::default()
    } else {
        annotation_layer_versions::Entity::find()
            .select_only()
            .column(annotation_layer_versions::Column::LayerId)
            .column_as(
                annotation_layer_versions::Column::VersionNumber.max(),
                "latest",
            )
            .filter(annotation_layer_versions::Column::LayerId.is_in(ids.clone()))
            .group_by(annotation_layer_versions::Column::LayerId)
            .into_tuple::<(Uuid, i64)>()
            .all(&ctx.db)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect()
    };
    let published_ids: Vec<Uuid> = page_rows
        .iter()
        .filter_map(|l| l.published_version_id)
        .collect();
    let published_numbers: std::collections::HashMap<Uuid, i64> = if published_ids.is_empty() {
        Default::default()
    } else {
        annotation_layer_versions::Entity::find()
            .filter(annotation_layer_versions::Column::Id.is_in(published_ids))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?
            .into_iter()
            .map(|v| (v.id, v.version_number))
            .collect()
    };
    let results: Vec<pnex_core::AnnotationLayerSummary> = page_rows
        .into_iter()
        .map(|l| {
            let latest_n = latest.get(&l.id).copied().unwrap_or(0);
            let published_n = l
                .published_version_id
                .and_then(|id| published_numbers.get(&id).copied());
            summary_dto(l, latest_n, published_n)
        })
        .collect();

    let mut filters = Vec::new();
    if let Some(s) = q.search.as_deref().filter(|s| !s.is_empty()) {
        filters.push(("search".to_string(), s.to_string()));
    }
    Ok(format::json(pagination::envelope(
        "/api/v1/annotation-layers",
        &filters,
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /annotation-layers/{id} ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DetailQuery {
    /// Version number of the returned document (default: latest).
    version: Option<i64>,
}

/// `GET /api/v1/annotation-layers/{id}[?version=n]` — détail + document.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-layers/{id}",
    tag = "annotation-layers",
    operation_id = "annotation_layers_get",
    summary = "Get an annotation layer",
    description = "The layer with the document of its latest version, or of `?version=n`.",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)"), DetailQuery),
    responses(
        (status = 200, body = pnex_core::AnnotationLayerDetail),
        (status = 404, description = "Unknown layer/version or layer of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<DetailQuery>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let (version_row, doc_number) = match q.version {
        Some(n) => {
            let row = annotation_layer_versions::Entity::find()
                .filter(annotation_layer_versions::Column::LayerId.eq(layer.id))
                .filter(annotation_layer_versions::Column::VersionNumber.eq(n))
                .one(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?;
            match row {
                Some(v) => (Some(v), n),
                None => return Err(Error::NotFound),
            }
        }
        None => {
            let latest = latest_version(&ctx.db, layer.id).await?;
            let n = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
            (latest, n)
        }
    };
    let doc: pnex_core::AnnotationDoc = version_row
        .map(|v| serde_json::from_value(v.doc).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    let latest_number = latest_version(&ctx.db, layer.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    let published_number = published_number_of(&ctx.db, layer.published_version_id).await?;
    Ok(format::json(detail_dto(
        layer,
        doc,
        doc_number,
        latest_number,
        published_number,
    ))
    .into_response())
}

// ─────────────────────────── PATCH /annotation-layers/{id} ───────────────────────────

/// `PATCH /{id}` — nouvelle version append-only. 409 si
/// `expected_version_number` ≠ version courante.
#[utoipa::path(
    patch,
    path = "/api/v1/annotation-layers/{id}",
    tag = "annotation-layers",
    operation_id = "annotation_layers_update",
    summary = "Save a new version of an annotation layer",
    description = "Appends a new version with the given document (append-only history), only if `expected_version_number` is the current latest version (optimistic concurrency). Controls declared by items are provisioned. Does not change what is published. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)")),
    request_body = pnex_core::UpdateAnnotationLayer,
    responses(
        (status = 200, body = pnex_core::AnnotationLayerDetail),
        (status = 400, description = "`field-invalid` (`name`, `media_asset_id`, `tour_id`) or `annotation-layer-invalid` with `errors.violations` = `[{subject, code, message}]`", body = crate::openapi::ErrorDetail),
        (status = 404, description = "Unknown layer or layer of another org", body = crate::openapi::ErrorDetail),
        (status = 409, description = "`annot-version-conflict` (`errors.args.expected` / `errors.args.current`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(params): Json<pnex_core::UpdateAnnotationLayer>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let (layer, version, doc) =
        match crate::services::annotation_layer::append_annotation_layer_version(
            &ctx.db,
            &layer,
            params.expected_version_number,
            &params.doc,
            params.name,
            params.author,
            params.note,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(layer_write_error_response(e)),
        };
    let published_number = published_number_of(&ctx.db, layer.published_version_id).await?;
    Ok(format::json(detail_dto(layer, doc, version, version, published_number)).into_response())
}

// ─────────────────────────── DELETE /annotation-layers/{id} ───────────────────────────

/// `DELETE /{id}` — 204 ; versions en cascade. Aucun octet à purger (D21).
/// Pas de purge D42 : KIND_ANNOTATION_LAYER = porte ouverte D60(5).
#[utoipa::path(
    delete,
    path = "/api/v1/annotation-layers/{id}",
    tag = "annotation-layers",
    operation_id = "annotation_layers_delete",
    summary = "Delete an annotation layer",
    description = "Deletes the layer and all its versions; controls its items declared are released (deleted unless still used elsewhere). Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Unknown layer or layer of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn delete(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    // D131: the controls its items declared are released (deleted, or kept
    // standalone while a flow or another surface uses them).
    let released = crate::services::surface_controls::release_surface(
        &ctx.db,
        org.org.id,
        pnex_core::ui_control::ORIGIN_ANNOTATION,
        layer.id,
    )
    .await
    .map_err(|_| Error::InternalServerError)?;
    crate::services::surface_controls::forget_deleted(&ctx, org.org.id, &released).await;
    annotation_layers::Entity::delete_by_id(layer.id)
        .exec(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(
        layer_id = layer.id.to_string().as_str(),
        "couche d'annotations supprimée"
    );
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─────────────────────────── GET /{id}/versions ───────────────────────────

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct VersionsQuery {
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

/// `GET /{id}/versions` — historique append-only, paginé (D14), du plus
/// récent au plus ancien.
#[utoipa::path(
    get,
    path = "/api/v1/annotation-layers/{id}/versions",
    tag = "annotation-layers",
    operation_id = "annotation_layers_versions_list",
    summary = "List the versions of an annotation layer",
    description = "Append-only history, newest first, paginated (`limit`/`offset`).",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)"), VersionsQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::AnnotationLayerVersionSummary>),
        (status = 404, description = "Unknown layer or layer of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn versions(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Query(q): Query<VersionsQuery>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let rows = annotation_layer_versions::Entity::find()
        .filter(annotation_layer_versions::Column::LayerId.eq(layer.id))
        .order_by_desc(annotation_layer_versions::Column::VersionNumber)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let count = rows.len() as i64;
    let (skip, take) = page.slice(rows.len());
    let results: Vec<pnex_core::AnnotationLayerVersionSummary> = rows
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|v| {
            let published = layer.published_version_id == Some(v.id);
            pnex_core::AnnotationLayerVersionSummary {
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
        &format!("/api/v1/annotation-layers/{id}/versions"),
        &[],
        page,
        count,
        results,
    ))
    .into_response())
}

// ─────────────────────────── GET /{id}/versions/{n} ───────────────────────────

/// `GET /{id}/versions/{n}` — document d'une version précise (audit /
/// rechargement éditeur — « restaurer » = la charger puis save).
#[utoipa::path(
    get,
    path = "/api/v1/annotation-layers/{id}/versions/{version_number}",
    tag = "annotation-layers",
    operation_id = "annotation_layers_versions_get",
    summary = "Get one version of an annotation layer",
    description = "The document of a given version (audit, or load it in the editor then save to restore it).",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)"), ("version_number" = i64, Path, description = "Version number")),
    responses(
        (status = 200, body = pnex_core::AnnotationLayerVersionDetail),
        (status = 404, description = "Unknown layer or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn version_detail(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path((id, n)): Path<(Uuid, i64)>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let Some(version) = annotation_layer_versions::Entity::find()
        .filter(annotation_layer_versions::Column::LayerId.eq(layer.id))
        .filter(annotation_layer_versions::Column::VersionNumber.eq(n))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let doc: pnex_core::AnnotationDoc =
        serde_json::from_value(version.doc.clone()).map_err(|_| Error::InternalServerError)?;
    Ok(format::json(pnex_core::AnnotationLayerVersionDetail {
        id: version.id.to_string(),
        version_number: version.version_number,
        author: version.author,
        note: version.note,
        published: layer.published_version_id == Some(version.id),
        created_at: version.created_at.to_rfc3339(),
        doc,
    })
    .into_response())
}

// ─────────────────────────── POST /{id}/publish | /unpublish ───────────────────────────

/// `POST /{id}/publish` — publie une version (version_number absent =
/// dernière) : simple pointeur (le doc publié sert le read model).
#[utoipa::path(
    post,
    path = "/api/v1/annotation-layers/{id}/publish",
    tag = "annotation-layers",
    operation_id = "annotation_layers_publish",
    summary = "Publish a version of an annotation layer",
    description = "Points the published version of the layer to `version_number` (default: latest). Its items become visible to every viewer of the anchored media. Body optional. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)")),
    request_body = Option<pnex_core::PublishAnnotationLayer>,
    responses(
        (status = 200, body = pnex_core::AnnotationLayerDetail),
        (status = 404, description = "Unknown layer or version", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn publish(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    body: Option<Json<pnex_core::PublishAnnotationLayer>>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let version_number = match body.and_then(|Json(p)| p.version_number) {
        Some(n) => n,
        None => {
            let Some(latest) = latest_version(&ctx.db, layer.id).await? else {
                return Err(Error::NotFound);
            };
            latest.version_number
        }
    };
    let Some(version) = annotation_layer_versions::Entity::find()
        .filter(annotation_layer_versions::Column::LayerId.eq(layer.id))
        .filter(annotation_layer_versions::Column::VersionNumber.eq(version_number))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let mut active: annotation_layers::ActiveModel = layer.clone().into();
    active.published_version_id = Set(Some(version.id));
    let layer = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(
        layer_id = layer.id.to_string().as_str(),
        version = version_number,
        "couche d'annotations publiée"
    );
    let doc: pnex_core::AnnotationDoc =
        serde_json::from_value(version.doc).map_err(|_| Error::InternalServerError)?;
    let latest_number = latest_version(&ctx.db, layer.id)
        .await?
        .map(|v| v.version_number)
        .unwrap_or(0);
    Ok(format::json(detail_dto(
        layer,
        doc,
        version_number,
        latest_number,
        Some(version_number),
    ))
    .into_response())
}

/// `POST /{id}/unpublish` — pointeur → NULL ; les items disparaissent de
/// tous les viewers, aucun octet déplacé.
#[utoipa::path(
    post,
    path = "/api/v1/annotation-layers/{id}/unpublish",
    tag = "annotation-layers",
    operation_id = "annotation_layers_unpublish",
    summary = "Unpublish an annotation layer",
    description = "Clears the published version: the layer items disappear from all viewers. Versions are kept. Requires owner, admin or member role.",
    params(("id" = String, Path, format = "uuid", description = "Annotation layer id (UUID)")),
    responses(
        (status = 200, body = pnex_core::AnnotationLayerDetail),
        (status = 404, description = "Unknown layer or layer of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn unpublish(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let Some(layer) = find_layer(&ctx.db, &org, id).await? else {
        return Err(Error::NotFound);
    };
    let mut active: annotation_layers::ActiveModel = layer.clone().into();
    active.published_version_id = Set(None);
    let layer = active
        .update(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    tracing::info!(
        layer_id = layer.id.to_string().as_str(),
        "couche d'annotations dépubliée"
    );
    let latest = latest_version(&ctx.db, layer.id).await?;
    let latest_number = latest.as_ref().map(|v| v.version_number).unwrap_or(0);
    let doc: pnex_core::AnnotationDoc = latest
        .map(|v| serde_json::from_value(v.doc).map_err(|_| Error::InternalServerError))
        .transpose()?
        .unwrap_or_default();
    Ok(format::json(detail_dto(layer, doc, latest_number, latest_number, None)).into_response())
}

// ─────────────────────────── Read model viewers ───────────────────────────

/// `GET /api/v1/media/{asset_id}/annotations[?tour=]` — items fusionnés des couches
/// publiées ancrées sur l'asset, in the read context (see `ReadScope`) (union, ordre déterministe : couches par id
/// asc, items dans l'ordre du doc), résolution device en batch (D57) —
/// référence morte tolérée (dead: true, jamais 500, école S7). 404 si asset
/// inconnu dans l'org (masqué). Auth-scoped : lecture membre (share public
/// exclu V1, D58).
/// Context of a read: a standalone media shows only the sets attached to
/// that media; a tour (`?tour=`) shows only that tour's sets — the two never
/// mix (D147).
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ReadScope {
    /// Tour id (UUID): read the sets of this tour instead of the media's own sets.
    #[param(value_type = Option<String>)]
    tour: Option<Uuid>,
}

#[utoipa::path(
    get,
    path = "/api/v1/media/{id}/annotations",
    tag = "media",
    operation_id = "media_annotations",
    summary = "Annotations shown on a media",
    description = "Merged items of the published annotation layers anchored on this media, with batch-resolved device targets (`resolved.dead` = device gone). Without `tour`: the media own sets plus free sets; with `tour`: only that tour sets. Read-only.",
    params(("id" = String, Path, format = "uuid", description = "Media asset id (UUID)"), ReadScope),
    responses(
        (status = 200, body = pnex_core::MediaAnnotations),
        (status = 404, description = "Unknown media or media of another org", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn media_annotations(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(asset_id): Path<Uuid>,
    Query(scope): Query<ReadScope>,
) -> Result<Response> {
    let Some(asset) = media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org.org.id))
        .one(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?
    else {
        return Err(Error::NotFound);
    };
    let asset_str = asset.id.to_string();
    // Couches publiées de l'org, ordre id asc = déterministe — restricted to
    // the read context (the tour's sets, or the sets of this media).
    // Standalone media: the sets of this media + free sets (attached to
    // nothing); never a tour's sets.
    let context = match scope.tour {
        Some(tour_id) => {
            sea_orm::Condition::all().add(annotation_layers::Column::TourId.eq(tour_id))
        }
        None => sea_orm::Condition::any()
            .add(annotation_layers::Column::MediaAssetId.eq(asset.id))
            .add(
                sea_orm::Condition::all()
                    .add(annotation_layers::Column::MediaAssetId.is_null())
                    .add(annotation_layers::Column::TourId.is_null()),
            ),
    };
    let layers = annotation_layers::Entity::find()
        .filter(annotation_layers::Column::OrgId.eq(org.org.id))
        .filter(annotation_layers::Column::PublishedVersionId.is_not_null())
        .filter(context)
        .order_by_asc(annotation_layers::Column::Id)
        .all(&ctx.db)
        .await
        .map_err(|_| Error::InternalServerError)?;
    let pub_ids: Vec<Uuid> = layers
        .iter()
        .filter_map(|l| l.published_version_id)
        .collect();
    let versions_map: std::collections::HashMap<Uuid, annotation_layer_versions::Model> =
        if pub_ids.is_empty() {
            Default::default()
        } else {
            // Only the published docs that anchor an item on this asset
            // (jsonb containment) instead of loading every published doc of
            // the org. The Rust filter below stays the source of truth.
            let probe = serde_json::json!({ "items": [{ "media_asset_id": asset_str }] });
            let q = annotation_layer_versions::Entity::find()
                .filter(annotation_layer_versions::Column::Id.is_in(pub_ids))
                .filter(sea_orm::sea_query::Expr::cust_with_values(
                    r#""annotation_layer_versions"."doc" @> $1"#,
                    [sea_orm::Value::Json(Some(Box::new(probe)))],
                ));
            q.all(&ctx.db)
                .await
                .map_err(|_| Error::InternalServerError)?
                .into_iter()
                .map(|v| (v.id, v))
                .collect()
        };
    let mut items: Vec<pnex_core::ResolvedAnnotationItem> = Vec::new();
    for l in &layers {
        let Some(published) = l.published_version_id.and_then(|id| versions_map.get(&id)) else {
            continue; // dangling pointer: tolerated
        };
        let Ok(doc) = serde_json::from_value::<pnex_core::AnnotationDoc>(published.doc.clone())
        else {
            continue; // doc illisible : toléré (jamais 500)
        };
        for item in doc
            .items
            .into_iter()
            .filter(|i| i.media_asset_id == asset_str)
        {
            items.push(pnex_core::ResolvedAnnotationItem {
                id: item.id,
                media_asset_id: item.media_asset_id,
                kind: item.kind,
                geometry: item.geometry,
                color: item.color,
                label: item.label,
                target: item.target,
                layer_id: l.id.to_string(),
                layer_name: l.name.clone(),
                resolved: None,
            });
        }
    }

    // Résolution device en batch (école link_target_labels, anti N+1).
    let mut slugs: Vec<String> = items
        .iter()
        .filter_map(|i| device_slug(&i.target).cloned())
        .collect();
    slugs.sort();
    slugs.dedup();
    let mut by_slug: std::collections::HashMap<String, i64> = Default::default();
    if !slugs.is_empty() {
        let rows = device_registries::Entity::find()
            .filter(device_registries::Column::OrgId.eq(org.org.id))
            .filter(device_registries::Column::DeviceId.is_in(slugs.clone()))
            .all(&ctx.db)
            .await
            .map_err(|_| Error::InternalServerError)?;
        for r in rows {
            by_slug.insert(r.device_id, r.id);
        }
    }
    for item in &mut items {
        let Some(slug) = device_slug(&item.target).cloned() else {
            continue;
        };
        item.resolved = Some(match by_slug.get(&slug) {
            Some(pk) => pnex_core::ResolvedTargetInfo {
                device_pk: Some(*pk),
                device_label: slug,
                dead: false,
            },
            None => pnex_core::ResolvedTargetInfo {
                device_pk: None,
                device_label: slug,
                dead: true,
            },
        });
    }
    Ok(format::json(pnex_core::MediaAnnotations {
        media_asset_id: asset_str,
        items,
    })
    .into_response())
}

// ─────────────────────────── Routes ───────────────────────────

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/annotation-layers")
        .add("", get(list).post(create))
        .add("/{id}", get(detail).patch(update).delete(delete))
        .add("/{id}/versions", get(versions))
        .add("/{id}/versions/{version_number}", get(version_detail))
        .add("/{id}/publish", post(publish))
        .add("/{id}/unpublish", post(unpublish))
}

/// Read model viewers — même prefix que `media.rs`, chemin `/{id}/annotations`
/// disjoint (aucune collision axum).
pub fn media_routes() -> Routes {
    Routes::new()
        .prefix("/api/v1/media")
        .add("/{id}/annotations", get(media_annotations))
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
        media_annotations
    ))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
