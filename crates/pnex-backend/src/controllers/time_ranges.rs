//! Time ranges API (media-ingest.md D169, §6): list by scope and time
//! window (D14), create, update, delete, CSV/ICS import. Org from the
//! principal (R1), `time-ranges:write` checked by the IAM extractor (R2);
//! a scope that is not the org's answers a `scope_id` field error.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;
use loco_rs::controller::{format, ErrorDetail};
use loco_rs::prelude::*;
use pnex_core::err_codes;
use pnex_core::time_range::{
    RangeOrigin, TimeRange, TimeRangeInput, IMPORT_MAX_BYTES, IMPORT_MAX_ROWS,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::OrgContext;
use crate::controllers::pagination;
use crate::services::time_ranges::import::{self, Format, ParseError};
use crate::services::time_ranges::{self, ListFilter, RangeError, Writer};

pub fn routes() -> Routes {
    Routes::new()
        .prefix("/api/v1")
        .add("/time-ranges", get(list).post(create))
        .add(
            "/time-ranges/import",
            // Room above the 1 MiB cap so the handler answers the coded error.
            post(import_ranges).layer(axum::extract::DefaultBodyLimit::max(IMPORT_MAX_BYTES * 2)),
        )
        .add(
            "/time-ranges/{id}",
            get(get_one).patch(update).delete(remove),
        )
}

fn detail(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(status, ErrorDetail::new(code, msg.to_string()))
}

/// The object-type write role on top of `time-ranges:write` (shared rule:
/// [`time_ranges::scope_writable`]).
async fn require_object_role(
    ctx: &AppContext,
    org: &OrgContext,
    scope_kind: &str,
    scope_id: &str,
) -> Result<()> {
    match time_ranges::scope_writable(&ctx.db, org.org.id, &org.role, scope_kind, scope_id).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(detail(
            StatusCode::FORBIDDEN,
            err_codes::ONTOLOGY_WRITE_FORBIDDEN,
            "This object type needs a higher role to write.",
        )),
        Err(e) => {
            tracing::error!(error = %e, "time range role check");
            Err(Error::InternalServerError)
        }
    }
}

/// [`require_object_role`] for an existing range (unknown id: the service
/// answers the 404).
async fn require_range_role(ctx: &AppContext, org: &OrgContext, id: Uuid) -> Result<()> {
    match time_ranges::find(&ctx.db, org.org.id, id).await {
        Ok(r) => require_object_role(ctx, org, &r.scope_kind, &r.scope_id).await,
        Err(_) => Ok(()),
    }
}

fn field(field: &str, token: &str) -> Result<Response> {
    Ok(super::field_error(field, token))
}

/// Service error → HTTP answer (field errors as `field-invalid`).
fn range_error(e: RangeError) -> Result<Response> {
    match e {
        RangeError::Invalid { field: f, token } => field(&f, &token),
        RangeError::ScopeUnknown => field("scope_id", err_codes::FIELD_INVALID),
        RangeError::NotFound => Err(detail(
            StatusCode::NOT_FOUND,
            err_codes::TIME_RANGE_NOT_FOUND,
            "Time range not found.",
        )),
        RangeError::Db(e) => {
            tracing::error!(error = %e, "time range database error");
            Err(Error::InternalServerError)
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Scope filter: `stream`, `org` or `object` (with `scope_id`).
    scope_kind: Option<String>,
    /// Id of the scope (stream or object UUID; ignored for `org`).
    scope_id: Option<String>,
    /// Window start, RFC 3339.
    from: Option<String>,
    /// Window end, RFC 3339.
    to: Option<String>,
    #[param(value_type = Option<i64>)]
    limit: Option<String>,
    #[param(value_type = Option<i64>)]
    offset: Option<String>,
}

fn parse_time(
    name: &'static str,
    raw: Option<&str>,
) -> std::result::Result<Option<chrono::DateTime<chrono::FixedOffset>>, &'static str> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(Some)
            .map_err(|_| name),
    }
}

/// `GET /api/v1/time-ranges` — every member (D14 envelope). Optional
/// `scope_kind` + `scope_id` and `from` / `to` window (RFC 3339).
#[utoipa::path(
    get,
    path = "/api/v1/time-ranges",
    tag = "time-ranges",
    operation_id = "time_ranges_list",
    summary = "List time ranges",
    description = "Paginated (`limit`/`offset`). Optional scope filter (`scope_kind` + `scope_id`) and time window (`from`/`to`, RFC 3339). Any member.",
    params(ListQuery),
    responses(
        (status = 200, body = pnex_core::Paginated<pnex_core::time_range::TimeRange>),
        (status = 400, description = "`field-invalid`, e.g. `errors.fields.from = invalid` or `errors.fields.scope_id = invalid`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn list(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ListQuery>,
) -> Result<Response> {
    let page = pagination::PageParams::from(q.limit.as_deref(), q.offset.as_deref());
    let (from, to) = match (
        parse_time("from", q.from.as_deref()),
        parse_time("to", q.to.as_deref()),
    ) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(f), _) | (_, Err(f)) => return field(f, err_codes::FIELD_INVALID),
    };
    let mut filters = Vec::new();
    let scope = match q.scope_kind.as_deref().filter(|k| !k.trim().is_empty()) {
        None => None,
        Some(kind) => {
            let id = q.scope_id.as_deref().unwrap_or_default();
            match time_ranges::resolve_scope(&ctx.db, org.org.id, kind, id).await {
                Ok(s) => {
                    filters.push(("scope_kind".to_string(), s.kind.wire().to_string()));
                    filters.push(("scope_id".to_string(), s.id.clone()));
                    Some(s)
                }
                Err(e) => return range_error(e),
            }
        }
    };
    for (k, v) in [("from", &q.from), ("to", &q.to)] {
        if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
            filters.push((k.to_string(), v.trim().to_string()));
        }
    }
    let select = time_ranges::select(org.org.id, &ListFilter { scope, from, to });
    let (count, rows) = match pagination::sql_page(&ctx.db, select, page).await {
        Ok(v) => v,
        Err(e) => return range_error(e.into()),
    };
    let results: Vec<TimeRange> = rows.iter().map(time_ranges::view).collect();
    format::json(pagination::envelope(
        "/api/v1/time-ranges",
        &filters,
        page,
        count,
        results,
    ))
}

/// `POST /api/v1/time-ranges` — origin `manual`. With an
/// `external_id` already used in the scope, the range is updated (upsert).
#[utoipa::path(
    post,
    path = "/api/v1/time-ranges",
    tag = "time-ranges",
    operation_id = "time_ranges_create",
    summary = "Create a time range",
    description = "Creates a manual range in a scope (`scope_kind` + `scope_id` required). Upsert: when `external_id` already exists in the scope, that range is updated and 200 is returned instead of 201. Requires a writer role, plus the object type's write role for an `object` scope.",
    request_body = pnex_core::time_range::TimeRangeInput,
    responses(
        (status = 201, description = "Created", body = pnex_core::time_range::TimeRange),
        (status = 200, description = "Existing `external_id`: updated", body = pnex_core::time_range::TimeRange),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`: the type of the scope requires a higher role", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<TimeRangeInput>,
) -> Result<Response> {
    require_object_role(
        &ctx,
        &org,
        input.scope_kind.as_deref().unwrap_or_default(),
        input.scope_id.as_deref().unwrap_or_default(),
    )
    .await?;
    let scope = match time_ranges::resolve_scope(
        &ctx.db,
        org.org.id,
        input.scope_kind.as_deref().unwrap_or_default(),
        input.scope_id.as_deref().unwrap_or_default(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return range_error(e),
    };
    let writer = Writer {
        origin: RangeOrigin::Manual,
        source_ref: Some(format!("manual:{}", org.auth.user.id)),
    };
    match time_ranges::upsert(&ctx.db, org.org.id, &scope, &input, &writer).await {
        Ok((row, created)) => {
            let status = if created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            Ok((status, format::json(time_ranges::view(&row))).into_response())
        }
        Err(e) => range_error(e),
    }
}

/// `GET /api/v1/time-ranges/{id}` — every member.
#[utoipa::path(
    get,
    path = "/api/v1/time-ranges/{id}",
    tag = "time-ranges",
    operation_id = "time_ranges_get",
    summary = "Get a time range",
    description = "Any member.",
    params(("id" = String, Path, format = "uuid", description = "Time range id (UUID)")),
    responses(
        (status = 200, body = pnex_core::time_range::TimeRange),
        (status = 404, description = "`time-range-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn get_one(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    match time_ranges::find(&ctx.db, org.org.id, id).await {
        Ok(row) => format::json(time_ranges::view(&row)),
        Err(e) => range_error(e),
    }
}

/// `PATCH /api/v1/time-ranges/{id}` — absent fields keep their
/// value, an empty string clears; scope, origin and provenance are kept.
#[utoipa::path(
    patch,
    path = "/api/v1/time-ranges/{id}",
    tag = "time-ranges",
    operation_id = "time_ranges_update",
    summary = "Update a time range",
    description = "Partial update: absent fields keep their value, an empty string clears it; scope, origin and provenance are kept. Completing an occurrence of a recurring series restarts the series. Requires a writer role (and the object type's write role for an `object` scope).",
    params(("id" = String, Path, format = "uuid", description = "Time range id (UUID)")),
    request_body = pnex_core::time_range::TimeRangeInput,
    responses(
        (status = 200, body = pnex_core::time_range::TimeRange),
        (status = 400, description = "`field-invalid`: `errors.fields` maps the field to a token", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`: the type of the scope requires a higher role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`time-range-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TimeRangeInput>,
) -> Result<Response> {
    require_range_role(&ctx, &org, id).await?;
    match time_ranges::update(&ctx.db, org.org.id, id, &input).await {
        Ok(row) => {
            // A completed occurrence restarts a floating series.
            if row.scope_kind == "object"
                && row
                    .external_id
                    .as_deref()
                    .is_some_and(|e| e.starts_with("rec:"))
            {
                if let Ok(obj) = Uuid::parse_str(&row.scope_id) {
                    crate::services::ontology::recurrence::refresh_logged(&ctx.db, org.org.id, obj)
                        .await;
                }
            }
            format::json(time_ranges::view(&row))
        }
        Err(e) => range_error(e),
    }
}

/// `DELETE /api/v1/time-ranges/{id}`.
#[utoipa::path(
    delete,
    path = "/api/v1/time-ranges/{id}",
    tag = "time-ranges",
    operation_id = "time_ranges_delete",
    summary = "Delete a time range",
    description = "Permanently deletes the range. Requires a writer role (and the object type's write role for an `object` scope).",
    params(("id" = String, Path, format = "uuid", description = "Time range id (UUID)")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 403, description = "`ontology-write-forbidden`: the type of the scope requires a higher role", body = crate::openapi::ErrorDetail),
        (status = 404, description = "`time-range-not-found`", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_range_role(&ctx, &org, id).await?;
    match time_ranges::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => range_error(e),
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ImportQuery {
    /// `csv` (with a label column) or `ics` (iCalendar).
    format: Option<String>,
    /// Target scope: `stream`, `org` or `object`.
    scope_kind: Option<String>,
    /// Id of the target scope.
    scope_id: Option<String>,
}

fn too_large() -> Error {
    super::coded_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        err_codes::TIME_RANGE_IMPORT_TOO_LARGE,
        "Import over 1 MiB or 5000 rows.",
        Some(serde_json::json!({
            "max_kib": (IMPORT_MAX_BYTES / 1024).to_string(),
            "max_rows": IMPORT_MAX_ROWS.to_string(),
        })),
    )
}

/// `POST /api/v1/time-ranges/import?format=csv|ics&scope_kind=&scope_id=`
/// — raw body (1 MiB, 5000 rows at most). Upserts by
/// `external_id` with origin `import`; answers the counts and the reasons
/// of the rejected rows.
#[utoipa::path(
    post,
    path = "/api/v1/time-ranges/import",
    tag = "time-ranges",
    operation_id = "time_ranges_import",
    summary = "Import time ranges from a CSV or iCalendar file",
    description = "Raw file body (1 MiB and 5000 rows at most) imported into one scope. Rows are upserted by `external_id` with origin `import`; the answer gives the created/updated/rejected counts and the reasons of the first rejected rows. Requires a writer role (and the object type's write role for an `object` scope).",
    params(ImportQuery),
    request_body(content = Vec<u8>, content_type = "text/plain", description = "CSV (with a label column) or iCalendar file content"),
    responses(
        (status = 200, body = pnex_core::time_range::RangeImportResult),
        (status = 400, description = "`time-range-import-invalid` (unreadable file), or `field-invalid` (bad `format` or scope)", body = crate::openapi::ErrorDetail),
        (status = 403, description = "`ontology-write-forbidden`: the type of the scope requires a higher role", body = crate::openapi::ErrorDetail),
        (status = 413, description = "`time-range-import-too-large` (args `max_kib`, `max_rows`)", body = crate::openapi::ErrorDetail)
    ),
    security(("bearer" = [], "org" = []))
)]
async fn import_ranges(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ImportQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    let Some(format) = q.format.as_deref().and_then(Format::from_wire) else {
        return field("format", err_codes::FIELD_INVALID);
    };
    if body.len() > IMPORT_MAX_BYTES {
        return Err(too_large());
    }
    require_object_role(
        &ctx,
        &org,
        q.scope_kind.as_deref().unwrap_or_default(),
        q.scope_id.as_deref().unwrap_or_default(),
    )
    .await?;
    let scope = match time_ranges::resolve_scope(
        &ctx.db,
        org.org.id,
        q.scope_kind.as_deref().unwrap_or_default(),
        q.scope_id.as_deref().unwrap_or_default(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return range_error(e),
    };
    let rows = match import::parse(format, &body) {
        Ok(rows) => rows,
        Err(ParseError::TooManyRows) => return Err(too_large()),
        Err(ParseError::Invalid) => {
            return Err(detail(
                StatusCode::BAD_REQUEST,
                err_codes::TIME_RANGE_IMPORT_INVALID,
                "Not a readable CSV (with a label column) or iCalendar file.",
            ))
        }
    };
    match import::run(&ctx.db, org.org.id, &scope, rows).await {
        Ok(result) => format::json(result),
        Err(e) => range_error(e.into()),
    }
}

/// OpenAPI paths of this controller (merged by `crate::openapi::spec`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    #[derive(utoipa::OpenApi)]
    #[openapi(paths(list, create, get_one, update, remove, import_ranges))]
    struct Doc;
    <Doc as utoipa::OpenApi>::openapi()
}
