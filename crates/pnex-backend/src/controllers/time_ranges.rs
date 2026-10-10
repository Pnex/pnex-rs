//! Time ranges API (media-ingest.md D169, §6): list by scope and time
//! window (D14), create, update, delete, CSV/ICS import. Org from the
//! principal (R1), `can_write` at the head of every write handler (R2);
//! a scope that is not the org's answers a `scope_id` field error.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
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
        .add("/time-ranges/{id}", patch(update).delete(remove))
}

fn detail(status: StatusCode, code: &str, msg: &str) -> Error {
    Error::CustomError(status, ErrorDetail::new(code, msg.to_string()))
}

fn require_write(org: &OrgContext) -> Result<()> {
    if org.can_write() {
        return Ok(());
    }
    Err(detail(
        StatusCode::FORBIDDEN,
        err_codes::TIME_RANGE_WRITE_FORBIDDEN,
        "Viewer role: time ranges are read-only.",
    ))
}

fn field(field: &str, token: &str) -> Result<Response> {
    Ok((
        StatusCode::BAD_REQUEST,
        format::json(serde_json::json!({ field: token })),
    )
        .into_response())
}

/// Service error → HTTP answer (field errors as `{field: token}`).
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

#[derive(Debug, Deserialize)]
struct ListQuery {
    scope_kind: Option<String>,
    scope_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<String>,
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

/// `POST /api/v1/time-ranges` — `can_write`; origin `manual`. With an
/// `external_id` already used in the scope, the range is updated (upsert).
async fn create(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Json(input): Json<TimeRangeInput>,
) -> Result<Response> {
    require_write(&org)?;
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

/// `PATCH /api/v1/time-ranges/{id}` — `can_write`. Absent fields keep their
/// value, an empty string clears; scope, origin and provenance are kept.
async fn update(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
    Json(input): Json<TimeRangeInput>,
) -> Result<Response> {
    require_write(&org)?;
    match time_ranges::update(&ctx.db, org.org.id, id, &input).await {
        Ok(row) => format::json(time_ranges::view(&row)),
        Err(e) => range_error(e),
    }
}

/// `DELETE /api/v1/time-ranges/{id}` — `can_write`.
async fn remove(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    require_write(&org)?;
    match time_ranges::delete(&ctx.db, org.org.id, id).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => range_error(e),
    }
}

#[derive(Debug, Deserialize)]
struct ImportQuery {
    format: Option<String>,
    scope_kind: Option<String>,
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
/// — `can_write`; raw body (1 MiB, 5000 rows at most). Upserts by
/// `external_id` with origin `import`; answers the counts and the reasons
/// of the rejected rows.
async fn import_ranges(
    State(ctx): State<AppContext>,
    org: OrgContext,
    Query(q): Query<ImportQuery>,
    body: axum::body::Bytes,
) -> Result<Response> {
    require_write(&org)?;
    let Some(format) = q.format.as_deref().and_then(Format::from_wire) else {
        return field("format", err_codes::FIELD_INVALID);
    };
    if body.len() > IMPORT_MAX_BYTES {
        return Err(too_large());
    }
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
